//! Remote discovery for the inventory: which providers are loading, ready or failed, what they
//! last returned, and which response still belongs to a current request. Also the actions
//! lager takes on a provider's repositories, which `remote_actions` must allow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use crate::application::git_dir;
use crate::application::ports::{
    ConfigStore, GitRemoval, GitState, HookRunner, Interaction, InteractionError, Mutation,
    PathKind, ProviderAnswer, ProviderFailure, ProviderSource, RemoteRepositories, RemoteState,
    RemovalFilesystem, Visibility, WorktreeFilesystem, WorktreeGit,
};
use crate::application::warehouse::{self, ExactRemovalError};
use crate::domain::config::{Config, RemoteAction};
use crate::domain::repository::{RepositoryRef, RepositorySummary, validate_reference_safety};
use crate::domain::state::LocalState;

/// How long a provider request may run, and how long ending the requests may hold the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteBounds {
    pub request: Duration,
    pub cancellation: Duration,
}

impl Default for RemoteBounds {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(30),
            cancellation: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderPhase {
    /// No request ran in this session, or it was cancelled before it answered.
    Idle,
    Loading {
        since: Instant,
    },
    Ready,
    Failed(String),
}

/// What the provider last returned, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCache {
    pub members: Vec<RepositorySummary>,
    pub fetched: SystemTime,
}

#[derive(Debug)]
pub struct ProviderState {
    pub phase: ProviderPhase,
    /// The last successful answer of this session. It outlives failures, refreshes and
    /// disabling remote discovery.
    pub cached: Option<ProviderCache>,
    /// A reloaded configuration changed what this provider's last request was built from.
    pub config_changed: bool,
    pending: Option<Pending>,
}

impl ProviderState {
    /// The cached members are older than the request that is running or failed.
    pub fn stale(&self) -> bool {
        self.cached.is_some() && self.phase != ProviderPhase::Ready
    }
}

#[derive(Debug)]
struct Pending {
    request: u64,
    cancelled: Arc<AtomicBool>,
}

struct Response {
    host: String,
    request: u64,
    answer: ProviderAnswer,
}

/// The wildcard declarations of the configuration; their members join the provider catalogs.
pub fn wildcards(config: &Config) -> Vec<RepositoryRef> {
    config
        .repositories
        .iter()
        .filter_map(|declaration| RepositoryRef::parse(&declaration.url).ok())
        .filter(RepositoryRef::is_wildcard)
        .collect()
}

fn host_wildcards(wildcards: &[RepositoryRef], host: &str) -> Vec<RepositoryRef> {
    wildcards
        .iter()
        .filter(|wildcard| wildcard.host.eq_ignore_ascii_case(host))
        .cloned()
        .collect()
}

/// Fetches provider catalogs on background threads and keeps the session's answers. Fetching
/// only reads from providers: it never registers, clones or runs hooks.
pub struct RemoteDiscovery<S: ProviderSource> {
    source: Arc<S>,
    wildcards: Vec<RepositoryRef>,
    bounds: RemoteBounds,
    enabled: bool,
    include_archived: bool,
    providers: BTreeMap<String, ProviderState>,
    next_request: u64,
    /// Requests whose thread has not answered yet, current or not.
    running: usize,
    sender: mpsc::Sender<Response>,
    receiver: mpsc::Receiver<Response>,
}

impl<S: ProviderSource> RemoteDiscovery<S> {
    pub fn new(source: S, wildcards: Vec<RepositoryRef>, bounds: RemoteBounds) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut discovery = Self {
            source: Arc::new(source),
            wildcards: Vec::new(),
            bounds,
            enabled: false,
            include_archived: false,
            providers: BTreeMap::new(),
            next_request: 0,
            running: 0,
            sender,
            receiver,
        };
        discovery.install(wildcards);
        discovery
    }

    /// Takes the providers of a reloaded configuration without a request. Answers of providers
    /// that are still configured stay. A provider whose settings or wildcards changed drops its
    /// running request, and its cached answer turns stale.
    pub fn reconfigure(&mut self, source: S, wildcards: Vec<RepositoryRef>) {
        for (host, provider) in &mut self.providers {
            if source.settings(host) != self.source.settings(host)
                || host_wildcards(&wildcards, host) != host_wildcards(&self.wildcards, host)
            {
                if let Some(pending) = provider.pending.take() {
                    pending.cancelled.store(true, Ordering::Relaxed);
                }
                provider.phase = ProviderPhase::Idle;
                provider.config_changed = true;
            }
        }
        self.source = Arc::new(source);
        self.install(wildcards);
    }

    fn install(&mut self, wildcards: Vec<RepositoryRef>) {
        self.wildcards = wildcards;
        let hosts = self.source.hosts();
        self.providers.retain(|host, provider| {
            let kept = hosts.contains(host);
            if !kept && let Some(pending) = provider.pending.take() {
                pending.cancelled.store(true, Ordering::Relaxed);
            }
            kept
        });
        for host in hosts {
            self.providers.entry(host).or_insert(ProviderState {
                phase: ProviderPhase::Idle,
                cached: None,
                config_changed: false,
                pending: None,
            });
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn include_archived(&self) -> bool {
        self.include_archived
    }

    pub fn providers(&self) -> &BTreeMap<String, ProviderState> {
        &self.providers
    }

    pub fn loading(&self) -> bool {
        self.providers
            .values()
            .any(|provider| provider.pending.is_some())
    }

    /// The providers whose last request failed.
    pub fn failed(&self) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|(_, provider)| matches!(provider.phase, ProviderPhase::Failed(_)))
            .map(|(host, _)| host.as_str())
            .collect()
    }

    /// Every cached member while remote discovery is enabled, stale ones included.
    pub fn members(&self) -> Vec<RepositorySummary> {
        if !self.enabled {
            return Vec::new();
        }
        self.providers
            .values()
            .filter_map(|provider| provider.cached.as_ref())
            .flat_map(|cache| cache.members.iter().cloned())
            .collect()
    }

    /// Fetches every provider. Remote discovery stays enabled until `disable`.
    pub fn enable(&mut self) {
        self.enabled = true;
        self.fetch_all();
    }

    /// Invalidates every running request. The cache and the archive preference stay.
    pub fn disable(&mut self) {
        self.enabled = false;
        for provider in self.providers.values_mut() {
            if let Some(pending) = provider.pending.take() {
                pending.cancelled.store(true, Ordering::Relaxed);
                provider.phase = ProviderPhase::Idle;
            }
        }
    }

    /// Fetches every provider again; cached members stay, stale, until the answers arrive.
    pub fn refresh(&mut self) {
        if self.enabled {
            self.fetch_all();
        }
    }

    /// Fetches the failed providers again and leaves every other request alone.
    pub fn retry(&mut self) {
        if !self.enabled {
            return;
        }
        let failed = self
            .failed()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for host in failed {
            self.fetch(&host);
        }
    }

    /// Stores the archive preference. While enabled, a change invalidates the running requests
    /// and fetches every provider again.
    pub fn set_include_archived(&mut self, include_archived: bool) {
        let changed = self.include_archived != include_archived;
        self.include_archived = include_archived;
        if changed && self.enabled {
            self.fetch_all();
        }
    }

    fn fetch_all(&mut self) {
        let hosts = self.providers.keys().cloned().collect::<Vec<_>>();
        for host in hosts {
            self.fetch(&host);
        }
    }

    fn fetch(&mut self, host: &str) {
        let Some(provider) = self.providers.get_mut(host) else {
            return;
        };
        if let Some(previous) = provider.pending.take() {
            previous.cancelled.store(true, Ordering::Relaxed);
        }
        let request = self.next_request;
        self.next_request += 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let since = Instant::now();
        provider.phase = ProviderPhase::Loading { since };
        provider.config_changed = false;
        provider.pending = Some(Pending {
            request,
            cancelled: Arc::clone(&cancelled),
        });
        let source = Arc::clone(&self.source);
        let sender = self.sender.clone();
        let wildcards = host_wildcards(&self.wildcards, host);
        let include_archived = self.include_archived;
        let host = host.to_owned();
        self.running += 1;
        let bound = self.bounds.request;
        // The request runs on a thread of its own, so its bound holds whether or not anything
        // polls: at the bound it is cancelled and answered as failed, and what it returns
        // later has no receiver.
        std::thread::spawn(move || {
            let (finished, answered) = mpsc::channel();
            let (request_host, flag) = (host.clone(), Arc::clone(&cancelled));
            std::thread::spawn(move || {
                let _ =
                    finished.send(source.fetch(&request_host, &wildcards, include_archived, &flag));
            });
            let answer = answered.recv_timeout(bound).unwrap_or_else(|_| {
                cancelled.store(true, Ordering::Relaxed);
                ProviderAnswer {
                    members: Vec::new(),
                    error: Some(format!("no response within {} s", bound.as_secs())),
                }
            });
            let _ = sender.send(Response {
                host,
                request,
                answer,
            });
        });
    }

    /// Applies the answers that arrived, a request's failure at its bound among them. An answer
    /// to a request that was invalidated since is dropped. Returns whether any provider changed,
    /// and the failures to report.
    pub fn poll(&mut self) -> (bool, Vec<ProviderFailure>) {
        let mut changed = false;
        let mut failures = Vec::new();
        while let Ok(response) = self.receiver.try_recv() {
            self.running -= 1;
            changed |= self.accept(response, &mut failures);
        }
        (changed, failures)
    }

    fn accept(&mut self, response: Response, failures: &mut Vec<ProviderFailure>) -> bool {
        let Some(provider) = self.providers.get_mut(&response.host) else {
            return false;
        };
        if provider
            .pending
            .take_if(|pending| pending.request == response.request)
            .is_none()
        {
            return false;
        }
        let mut fail = |error: String| {
            failures.push(ProviderFailure {
                provider: response.host.clone(),
                error,
            })
        };
        let ProviderAnswer { mut members, error } = response.answer;
        let listed = members.len();
        // A clone URL carrying credentials, a query or a fragment is never shown or used.
        members.retain(|member| validate_reference_safety(&member.reference.clone_url).is_ok());
        match listed - members.len() {
            0 => {}
            1 => fail("1 repository skipped: unsafe clone URL".to_owned()),
            skipped => fail(format!("{skipped} repositories skipped: unsafe clone URL")),
        }
        // What a partly failed request did return replaces the cache; a request that returned
        // nothing keeps the members it could not replace.
        if error.is_none() || !members.is_empty() {
            provider.cached = Some(ProviderCache {
                members,
                fetched: SystemTime::now(),
            });
        }
        provider.phase = match error {
            Some(error) => {
                fail(error.clone());
                ProviderPhase::Failed(error)
            }
            None => ProviderPhase::Ready,
        };
        true
    }

    /// Whether a request thread has not answered yet, current or not.
    pub fn busy(&mut self) -> bool {
        while self.running > 0 && self.receiver.try_recv().is_ok() {
            self.running -= 1;
        }
        self.running > 0
    }

    /// Cancels every request and waits for the request threads, at most the cancellation bound.
    pub fn shutdown(&mut self) {
        self.shutdown_by(Instant::now() + self.bounds.cancellation);
    }

    /// Cancels every request and waits for the request threads until `deadline`. Threads that
    /// have not answered by then are no longer waited for.
    pub fn shutdown_by(&mut self, deadline: Instant) {
        self.disable();
        while self.running > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if self.receiver.recv_timeout(left).is_err() {
                break;
            }
            self.running -= 1;
        }
        self.running = 0;
    }
}

impl<S: ProviderSource> Drop for RemoteDiscovery<S> {
    fn drop(&mut self) {
        // An interruptible request, such as a provider CLI, is ended before the process exits.
        self.shutdown();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// The user answered no to the prompt.
    Declined,
    Cancelled,
    Failed(String),
}

impl From<String> for ActionError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// Refuses `action` unless the provider for `reference` lists it in `remote_actions`.
pub fn allowed(
    config: &Config,
    reference: &RepositoryRef,
    action: RemoteAction,
) -> Result<(), String> {
    let host = reference.host.to_ascii_lowercase();
    if config
        .providers
        .get(&host)
        .is_some_and(|provider| provider.remote_actions.contains(&action))
    {
        return Ok(());
    }
    Err(format!(
        "`{}` is not enabled for {host}; add it to remote_actions under [providers.\"{host}\"]",
        action.as_str()
    ))
}

/// The checks every action runs before it changes anything on the provider.
fn prepare(
    config: &Config,
    remote: &dyn RemoteRepositories,
    reference: &RepositoryRef,
    action: RemoteAction,
) -> Result<RemoteState, ActionError> {
    ready(config, remote, reference, action)?;
    remote
        .state(reference)?
        .ok_or_else(|| format!("not found on {}", reference.host).into())
}

/// One `owner/name` repository whose provider allows `action` and whose CLI is logged in.
fn ready(
    config: &Config,
    remote: &dyn RemoteRepositories,
    reference: &RepositoryRef,
    action: RemoteAction,
) -> Result<(), ActionError> {
    // A leading dash would reach gh as a flag.
    if reference.is_wildcard()
        || reference.path.split('/').count() != 2
        || reference.path.starts_with('-')
    {
        return Err(format!("{} needs one repository as owner/name", action.as_str()).into());
    }
    allowed(config, reference, action)?;
    remote.check(&reference.host)?;
    Ok(())
}

/// Creates `reference` on its provider once neither its checkout path nor its name is taken,
/// and returns that checkout path.
pub fn create(
    config: &Config,
    remote: &dyn RemoteRepositories,
    git: &dyn GitState,
    reference: &RepositoryRef,
    visibility: Visibility,
    description: Option<&str>,
    home: &Path,
) -> Result<PathBuf, ActionError> {
    ready(config, remote, reference, RemoteAction::Create)?;
    let destination = warehouse::destination(config, reference, home)?;
    if git.classify_destination(&destination, &reference.clone_url) != LocalState::Missing {
        return Err(format!("destination already exists: {}", destination.display()).into());
    }
    if remote.state(reference)?.is_some() {
        return Err(format!("already exists on {}", reference.host).into());
    }
    remote.create(reference, visibility, description)?;
    Ok(destination)
}

/// Publishes the local repository at `source` as `reference`, then moves it to its checkout
/// path and returns that path. Every refusal happens before the provider is touched.
#[allow(clippy::too_many_arguments)]
pub fn publish(
    config: &Config,
    remote: &dyn RemoteRepositories,
    git: &dyn WorktreeGit,
    fs: &dyn WorktreeFilesystem,
    reference: &RepositoryRef,
    visibility: Visibility,
    description: Option<&str>,
    source: &Path,
    home: &Path,
) -> Result<PathBuf, ActionError> {
    ready(config, remote, reference, RemoteAction::Create)?;
    let destination = warehouse::destination(config, reference, home)?;
    if fs.kind(&destination)? != PathKind::Missing {
        return Err(format!("destination already exists: {}", destination.display()).into());
    }
    let shown = source.display();
    let refuse = |reason: String| ActionError::from(format!("cannot publish {shown}: {reason}"));
    let source = fs.canonicalize(source).map_err(refuse)?;
    if source.to_str().is_none() {
        return Err(refuse("its path is not UTF-8".to_owned()));
    }
    let location = git
        .locate(&source)
        .map_err(|_| refuse("not a Git working tree".to_owned()))?;
    if location.top_level != source {
        return Err(refuse("not the top of its repository".to_owned()));
    }
    if location.git_dir != source.join(".git") {
        return Err(refuse(
            "its Git directory lives outside it, as in a submodule or linked worktree".to_owned(),
        ));
    }
    let worktrees = git.entries(&source).map_err(|fault| refuse(fault.error))?;
    if worktrees.len() > 1 {
        let linked = worktrees
            .iter()
            .filter(|entry| entry.path != source)
            .map(|entry| entry.path.display().to_string())
            .collect::<Vec<_>>();
        return Err(refuse(format!(
            "it has linked worktrees ({}); remove them first (lager worktree remove)",
            linked.join(", ")
        )));
    }
    if git.commit(&source, "HEAD").is_none() {
        return Err(refuse("it has no commit yet".to_owned()));
    }
    let remotes = git.remotes(&source).map_err(refuse)?;
    if !remotes.is_empty() {
        return Err(refuse(format!(
            "it already has a remote ({})",
            remotes.join(", ")
        )));
    }
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    if let Ok(root) = fs.canonicalize(&root)
        && source.starts_with(&root)
    {
        return Err(refuse(format!("it is already inside {}", root.display())));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent".to_owned())?;
    let existing = parent
        .ancestors()
        .find(|path| fs.kind(path).is_ok_and(|kind| kind != PathKind::Missing))
        .unwrap_or(parent);
    if fs.canonicalize(existing)?.starts_with(&source) {
        return Err(refuse(format!("{} lies inside it", destination.display())));
    }
    if fs.device(existing)? != fs.device(&source)? {
        return Err(refuse(format!(
            "it is on another file system than {}",
            destination.display()
        )));
    }
    if remote.state(reference)?.is_some() {
        return Err(format!("already exists on {}", reference.host).into());
    }
    let identity = reference.identity();
    let finish = format!(
        "move it to {}, then `lager register {identity}`",
        destination.display()
    );
    if let Err(error) = remote.publish(reference, visibility, description, &source) {
        // gh can create the repository and then fail to push.
        return Err(match remote.state(reference) {
            Ok(Some(_)) => {
                let origin = git
                    .remotes(&source)
                    .is_ok_and(|remotes| remotes.iter().any(|name| name == "origin"));
                let (state, push) = if origin {
                    ("origin is set", "push")
                } else {
                    ("origin is not set", "set origin, push")
                };
                format!(
                    "created {identity}; push failed: {error}; {state} and {shown} was not moved; {push}, {finish}"
                )
            }
            _ => error,
        }
        .into());
    }
    fs.create_dir_all(parent)
        .and_then(|()| fs.rename(&source, &destination))
        .map_err(|error| {
            format!(
                "published {identity} from {shown}; move failed: {error}; {shown} is pushed with origin set; {finish}"
            )
        })?;
    Ok(destination)
}

/// Declares a repository `publish` moved into root and runs `post_clone` in it. Errors say
/// that the publish and the move already happened.
#[allow(clippy::too_many_arguments)]
pub fn settle_published<S: ConfigStore, H: HookRunner>(
    store: &S,
    hooks: &H,
    config_path: &Path,
    repository: &str,
    register: bool,
    post_clone: Option<&str>,
    source: &Path,
    destination: &Path,
) -> Result<(), ActionError> {
    let fail = |error: String| {
        ActionError::from(format!(
            "published {repository} from {}; moved to {}; {error}",
            source.display(),
            destination.display()
        ))
    };
    if register {
        store
            .register(config_path, &[repository.to_owned()], post_clone)
            .map_err(|error| fail(format!("could not persist declaration: {error}")))?;
    }
    if let Some(command) = post_clone
        && let Err(error) = hooks.run_hook(command, destination)
    {
        if hooks.is_cancelled(&error) {
            return Err(ActionError::Cancelled);
        }
        return Err(fail(format!("post_clone failed: {error}")));
    }
    Ok(())
}

pub fn archive(
    config: &Config,
    remote: &dyn RemoteRepositories,
    reference: &RepositoryRef,
    archived: bool,
) -> Result<Mutation, ActionError> {
    let state = prepare(config, remote, reference, RemoteAction::Archive)?;
    if state.archived == archived {
        return Ok(Mutation::Noop);
    }
    remote.archive(reference, archived)?;
    Ok(Mutation::Changed)
}

/// Making a repository public exposes its code and history, so it asks first unless `yes`.
pub fn visibility(
    config: &Config,
    remote: &dyn RemoteRepositories,
    interaction: &mut dyn Interaction,
    reference: &RepositoryRef,
    visibility: Visibility,
    yes: bool,
) -> Result<Mutation, ActionError> {
    let state = prepare(config, remote, reference, RemoteAction::Visibility)?;
    if state.visibility == visibility {
        return Ok(Mutation::Noop);
    }
    if visibility == Visibility::Public && !yes {
        let question = format!(
            "Make {} public? Its code and history become visible to everyone.",
            reference.identity()
        );
        match interaction.confirm(&question, false) {
            Ok(true) => {}
            Ok(false) => return Err(ActionError::Declined),
            Err(InteractionError::Cancelled) => return Err(ActionError::Cancelled),
            Err(InteractionError::Failed(error)) => return Err(ActionError::Failed(error)),
        }
    }
    remote.set_visibility(reference, visibility)?;
    Ok(Mutation::Changed)
}

/// What `delete` did after deleting the repository on its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    /// A checkout existed and was removed.
    pub removed: bool,
    /// A declaration changed.
    pub unregistered: bool,
}

/// Deletes `reference` on its provider, then its checkout and its declaration. The guards of
/// `remove` run before the provider is touched, and local losses refuse unless `force`. Without
/// `yes` the user types `owner/name` to confirm.
#[allow(clippy::too_many_arguments)]
pub fn delete<S: ConfigStore, G: GitRemoval, F: RemovalFilesystem>(
    config: &Config,
    remote: &dyn RemoteRepositories,
    store: &S,
    git: &G,
    filesystem: &F,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    reference: &RepositoryRef,
    yes: bool,
    force: bool,
    home: &Path,
) -> Result<Deleted, ActionError> {
    ready(config, remote, reference, RemoteAction::Delete)?;
    remote.check_delete_scope(&reference.host)?;
    let identity = reference.identity();
    let finish = format!("`lager remove {identity} --unregister`");
    if remote.state(reference)?.is_none() {
        return Err(format!(
            "not found on {}; run {finish} for the local side",
            reference.host
        )
        .into());
    }
    let untouched = |error: String| ActionError::from(format!("{error}; GitHub was not touched"));
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let destination = warehouse::destination(config, reference, home)?;
    let inspect = || {
        warehouse::inspect_exact_removal(git, filesystem, &root, &destination, &reference.clone_url)
    };
    let exists = filesystem
        .metadata_exists(&destination)
        .map_err(|error| error.to_string())?;
    let checkout = if exists {
        let confirmed = inspect().map_err(untouched)?;
        if !confirmed.losses.is_empty() && !force {
            return Err(format!(
                "local work would be lost ({}); GitHub was not touched; rerun with --force to accept these losses",
                confirmed.losses.join("; ")
            )
            .into());
        }
        Some(confirmed)
    } else {
        None
    };
    if !yes {
        confirm_name(interaction, reference)?;
        // The prompt can stay open for a while; what it confirmed must still hold.
        if let Some(confirmed) = &checkout
            && inspect().map_err(untouched)? != *confirmed
        {
            return Err(untouched(
                "the checkout changed while confirming".to_owned(),
            ));
        }
    }
    if let Some(confirmed) = checkout
        .as_ref()
        .filter(|checkout| !checkout.losses.is_empty())
    {
        // The hints name commands that stop applying once the checkout is gone.
        let lost = confirmed
            .losses
            .iter()
            .map(|loss| git_dir::without_hint(loss))
            .collect::<Vec<_>>();
        interaction.notice(&format!(
            "{}: deleting with --force; this is lost: {}",
            reference.path,
            lost.join("; ")
        ));
    }
    remote.delete(reference)?;
    let shown = destination.display();
    if let Some(confirmed) = &checkout {
        warehouse::remove_exact(git, filesystem, &root, &destination, &reference.clone_url, confirmed)
            .map_err(|error| match error {
                ExactRemovalError::Stale(error) => format!(
                    "deleted {identity}; local removal failed: {error}; {shown} and its declaration remain; finish with {finish}"
                ),
                ExactRemovalError::Failed(error) => format!(
                    "deleted {identity}; local removal failed: {error}; {shown} may be partly removed and its declaration remains; finish with {finish}"
                ),
            })?;
    }
    let mutations = store
        .remove(config_path, std::slice::from_ref(&identity))
        .map_err(|error| {
            let removed = if checkout.is_some() {
                format!("removed {shown}; ")
            } else {
                String::new()
            };
            format!("deleted {identity}; {removed}could not remove the declaration: {error}; finish with {finish}")
        })?;
    Ok(Deleted {
        removed: checkout.is_some(),
        unregistered: mutations.contains(&Mutation::Changed),
    })
}

/// Asks the user to type `owner/name` of `reference`; any other answer declines.
pub fn confirm_name(
    interaction: &mut dyn Interaction,
    reference: &RepositoryRef,
) -> Result<(), ActionError> {
    let question = format!(
        "Delete {} on GitHub for good, with its checkout and declaration? Type {} to confirm",
        reference.identity(),
        reference.path
    );
    match interaction.input(&question, &reference.path, Some("")) {
        Ok(answer) if answer == reference.path => Ok(()),
        Ok(_) => Err(ActionError::Declined),
        Err(InteractionError::Cancelled) => Err(ActionError::Cancelled),
        Err(InteractionError::Failed(error)) => Err(ActionError::Failed(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex};

    use super::*;

    /// A provider source whose answers the test releases one by one.
    #[derive(Default)]
    struct Gated {
        hosts: Vec<String>,
        /// Requests in arrival order: host and archive preference.
        requests: Mutex<Vec<(String, bool)>>,
        /// Answers waiting per request index.
        answers: Mutex<BTreeMap<usize, ProviderAnswer>>,
        released: Condvar,
        cancellations: Mutex<Vec<usize>>,
        settings: String,
    }

    #[derive(Clone)]
    struct Stub(Arc<Gated>);

    impl Stub {
        fn new(hosts: &[&str]) -> Self {
            Self(Arc::new(Gated {
                hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                ..Gated::default()
            }))
        }

        fn requests(&self) -> Vec<(String, bool)> {
            self.0.requests.lock().unwrap().clone()
        }

        fn answer(&self, request: usize, result: Result<Vec<RepositorySummary>, String>) {
            let answer = match result {
                Ok(members) => ProviderAnswer {
                    members,
                    error: None,
                },
                Err(error) => ProviderAnswer {
                    members: Vec::new(),
                    error: Some(error),
                },
            };
            self.answer_with(request, answer);
        }

        fn answer_with(&self, request: usize, answer: ProviderAnswer) {
            self.0.answers.lock().unwrap().insert(request, answer);
            self.0.released.notify_all();
        }

        /// Answers the provider's request number `nth`; requests of two providers that start
        /// together arrive in either order.
        fn answer_host(
            &self,
            host: &str,
            nth: usize,
            result: Result<Vec<RepositorySummary>, String>,
        ) {
            let request = self
                .requests()
                .iter()
                .enumerate()
                .filter(|(_, (requested, _))| requested == host)
                .nth(nth)
                .map(|(index, _)| index)
                .expect("the request was made");
            self.answer(request, result);
        }

        fn wait_for_requests(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.requests().len() < count {
                assert!(Instant::now() < deadline, "{:?}", self.requests());
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl ProviderSource for Stub {
        fn hosts(&self) -> Vec<String> {
            self.0.hosts.clone()
        }

        fn settings(&self, _host: &str) -> String {
            self.0.settings.clone()
        }

        fn fetch(
            &self,
            host: &str,
            _: &[RepositoryRef],
            include_archived: bool,
            cancelled: &AtomicBool,
        ) -> ProviderAnswer {
            let index = {
                let mut requests = self.0.requests.lock().unwrap();
                requests.push((host.to_owned(), include_archived));
                requests.len() - 1
            };
            let mut answers = self.0.answers.lock().unwrap();
            loop {
                if let Some(answer) = answers.remove(&index) {
                    return answer;
                }
                if cancelled.load(Ordering::Relaxed) {
                    let mut cancellations = self.0.cancellations.lock().unwrap();
                    if !cancellations.contains(&index) {
                        cancellations.push(index);
                    }
                }
                answers = self
                    .0
                    .released
                    .wait_timeout(answers, Duration::from_millis(2))
                    .unwrap()
                    .0;
            }
        }
    }

    fn member(identity: &str) -> RepositorySummary {
        RepositorySummary {
            reference: RepositoryRef::parse(identity).unwrap(),
            archived: false,
        }
    }

    fn identities(discovery: &RemoteDiscovery<Stub>) -> Vec<String> {
        discovery
            .members()
            .iter()
            .map(|member| member.reference.identity())
            .collect()
    }

    /// Polls until the provider left `Loading`, and returns every failure reported on the way.
    fn settle(discovery: &mut RemoteDiscovery<Stub>, hosts: &[&str]) -> Vec<ProviderFailure> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut failures = Vec::new();
        loop {
            failures.extend(discovery.poll().1);
            if hosts.iter().all(|host| {
                !matches!(
                    discovery.providers()[*host].phase,
                    ProviderPhase::Loading { .. }
                )
            }) {
                return failures;
            }
            assert!(Instant::now() < deadline, "providers did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The stub never ends a request on its own, so dropping waits out the cancellation bound.
    fn discovery(stub: &Stub) -> RemoteDiscovery<Stub> {
        RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_secs(30),
                cancellation: Duration::from_millis(20),
            },
        )
    }

    #[test]
    fn enable_fetches_every_provider_and_keeps_partial_results_beside_a_failure() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        assert!(!remote.enabled() && remote.members().is_empty());
        remote.enable();
        stub.wait_for_requests(2);
        assert!(remote.loading());
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert!(matches!(
            remote.providers()["b.example"].phase,
            ProviderPhase::Loading { .. }
        ));
        stub.answer_host("b.example", 0, Err("refused".to_owned()));
        let failures = settle(&mut remote, &["b.example"]);
        assert_eq!(
            failures,
            [ProviderFailure {
                provider: "b.example".to_owned(),
                error: "refused".to_owned()
            }]
        );
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert_eq!(remote.failed(), ["b.example"]);
        assert!(remote.poll().1.is_empty(), "a failure is reported once");
    }

    #[test]
    fn retry_requests_only_the_failed_provider_and_refresh_keeps_stale_members() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(2);
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        stub.answer_host("b.example", 0, Err("refused".to_owned()));
        settle(&mut remote, &["a.example", "b.example"]);

        remote.retry();
        stub.wait_for_requests(3);
        assert_eq!(stub.requests()[2].0, "b.example");
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        stub.answer(2, Ok(vec![member("b.example/org/two")]));
        settle(&mut remote, &["b.example"]);
        assert!(remote.failed().is_empty());
        remote.retry();
        assert_eq!(stub.requests().len(), 3, "nothing failed, nothing to retry");

        remote.refresh();
        stub.wait_for_requests(5);
        assert!(remote.providers()["a.example"].stale());
        assert_eq!(
            identities(&remote),
            ["a.example/org/one", "b.example/org/two"]
        );
        // A failed refresh keeps the members it could not replace.
        stub.answer_host("a.example", 1, Err("refused".to_owned()));
        stub.answer_host("b.example", 2, Ok(Vec::new()));
        settle(&mut remote, &["a.example", "b.example"]);
        assert!(remote.providers()["a.example"].stale());
        assert!(!remote.providers()["b.example"].stale());
        assert_eq!(identities(&remote), ["a.example/org/one"]);
    }

    #[test]
    fn answers_to_invalidated_requests_are_dropped() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);

        // Disabling invalidates the request; its answer changes nothing.
        remote.disable();
        stub.answer(0, Ok(vec![member("a.example/org/late")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Idle);
        assert!(remote.providers()["a.example"].cached.is_none());

        // An archive toggle supersedes the request that ran with the other preference.
        remote.enable();
        stub.wait_for_requests(2);
        remote.set_include_archived(true);
        stub.wait_for_requests(3);
        assert_eq!(
            stub.requests()[1..],
            [
                ("a.example".to_owned(), false),
                ("a.example".to_owned(), true)
            ]
        );
        stub.answer(1, Ok(vec![member("a.example/org/without")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert!(remote.loading());
        stub.answer(2, Ok(vec![member("a.example/org/with")]));
        settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/with"]);

        // So does a refresh.
        remote.refresh();
        stub.wait_for_requests(4);
        remote.refresh();
        stub.wait_for_requests(5);
        stub.answer(3, Ok(vec![member("a.example/org/first")]));
        stub.answer(4, Ok(vec![member("a.example/org/second")]));
        settle(&mut remote, &["a.example"]);
        std::thread::sleep(Duration::from_millis(20));
        remote.poll();
        assert_eq!(identities(&remote), ["a.example/org/second"]);
    }

    #[test]
    fn disabling_keeps_the_cache_and_the_archive_preference_and_requests_nothing() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.set_include_archived(true);
        assert!(
            stub.requests().is_empty(),
            "the preference alone fetches nothing"
        );
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer(0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);

        remote.disable();
        assert!(remote.members().is_empty());
        assert!(remote.include_archived());
        assert!(remote.providers()["a.example"].cached.is_some());
        remote.refresh();
        remote.retry();
        remote.set_include_archived(false);
        remote.reconfigure(stub.clone(), Vec::new());
        assert_eq!(stub.requests().len(), 1);

        // Enabling again shows the cached members, stale, while the providers load.
        remote.enable();
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert!(remote.providers()["a.example"].stale());
    }

    #[test]
    fn a_request_past_its_bound_fails_is_cancelled_and_its_late_answer_is_dropped() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_millis(30),
                cancellation: Duration::from_millis(30),
            },
        );
        remote.enable();
        stub.wait_for_requests(1);
        // The bound holds while nothing polls: the request is cancelled on its own.
        let deadline = Instant::now() + Duration::from_secs(10);
        while stub.0.cancellations.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the request was not cancelled");
            std::thread::sleep(Duration::from_millis(1));
        }
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "no response within 0 s");
        stub.answer(0, Ok(vec![member("a.example/org/late")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert_eq!(remote.failed(), ["a.example"]);
    }

    #[test]
    fn shutdown_waits_for_a_stalled_request_no_longer_than_the_cancellation_bound() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_secs(30),
                cancellation: Duration::from_millis(50),
            },
        );
        remote.enable();
        stub.wait_for_requests(1);
        let started = Instant::now();
        drop(remote);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(50), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
        assert_eq!(*stub.0.cancellations.lock().unwrap(), [0]);
        stub.answer(0, Ok(Vec::new()));

        // Without a running request there is nothing to wait for.
        let started = Instant::now();
        drop(RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds::default(),
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn members_with_unsafe_clone_urls_are_skipped_and_reported_without_their_value() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        let mut unsafe_member = member("a.example/org/leak");
        unsafe_member.reference.clone_url = "https://user:secret@a.example/org/leak.git".to_owned();
        stub.answer(0, Ok(vec![member("a.example/org/one"), unsafe_member]));
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "1 repository skipped: unsafe clone URL");
    }

    #[test]
    fn a_partly_failed_answer_keeps_its_members_and_leaves_the_provider_failed_for_retry() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer_with(
            0,
            ProviderAnswer {
                members: vec![member("a.example/me/own")],
                error: Some("a.example/org/*: refused".to_owned()),
            },
        );
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "a.example/org/*: refused");
        assert_eq!(identities(&remote), ["a.example/me/own"]);
        assert_eq!(remote.failed(), ["a.example"]);

        remote.retry();
        stub.wait_for_requests(2);
        stub.answer(
            1,
            Ok(vec![
                member("a.example/me/own"),
                member("a.example/org/one"),
            ]),
        );
        settle(&mut remote, &["a.example"]);
        assert!(remote.failed().is_empty());
        assert_eq!(
            identities(&remote),
            ["a.example/me/own", "a.example/org/one"]
        );
    }

    #[test]
    fn a_reloaded_configuration_keeps_known_providers_and_drops_removed_ones() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(2);
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        stub.answer_host("b.example", 0, Ok(vec![member("b.example/org/two")]));
        settle(&mut remote, &["a.example", "b.example"]);

        let next = Stub::new(&["b.example", "c.example"]);
        remote.reconfigure(next.clone(), Vec::new());
        assert_eq!(identities(&remote), ["b.example/org/two"]);
        assert_eq!(remote.providers()["c.example"].phase, ProviderPhase::Idle);
        assert!(next.requests().is_empty() && stub.requests().len() == 2);
    }

    #[test]
    fn a_reloaded_configuration_turns_an_answer_stale_when_its_provider_settings_changed() {
        let settled = |settings: &str| {
            Stub(Arc::new(Gated {
                hosts: vec!["a.example".to_owned()],
                settings: settings.to_owned(),
                ..Gated::default()
            }))
        };
        let stub = settled("api one");
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer(0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);

        remote.reconfigure(settled("api one"), Vec::new());
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        remote.reconfigure(settled("api two"), Vec::new());
        let provider = &remote.providers()["a.example"];
        assert_eq!(provider.phase, ProviderPhase::Idle);
        assert!(provider.stale() && provider.config_changed);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
    }

    struct PublicRemote {
        changed: Mutex<Vec<Visibility>>,
    }

    impl RemoteRepositories for PublicRemote {
        fn check(&self, _host: &str) -> Result<(), String> {
            Ok(())
        }
        fn state(&self, _reference: &RepositoryRef) -> Result<Option<RemoteState>, String> {
            Ok(Some(RemoteState {
                archived: false,
                visibility: Visibility::Private,
            }))
        }
        fn archive(&self, _reference: &RepositoryRef, _archived: bool) -> Result<(), String> {
            unreachable!()
        }
        fn set_visibility(
            &self,
            _reference: &RepositoryRef,
            visibility: Visibility,
        ) -> Result<(), String> {
            self.changed.lock().unwrap().push(visibility);
            Ok(())
        }
        fn create(&self, _: &RepositoryRef, _: Visibility, _: Option<&str>) -> Result<(), String> {
            unreachable!()
        }
        fn publish(
            &self,
            _: &RepositoryRef,
            _: Visibility,
            _: Option<&str>,
            _: &Path,
        ) -> Result<(), String> {
            unreachable!()
        }
        fn check_delete_scope(&self, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn delete(&self, _: &RepositoryRef) -> Result<(), String> {
            unreachable!()
        }
    }

    struct Answer(Result<bool, InteractionError>, usize);

    impl Interaction for Answer {
        fn confirm(&mut self, _message: &str, default: bool) -> Result<bool, InteractionError> {
            assert!(!default, "making a repository public defaults to no");
            self.1 += 1;
            self.0.clone()
        }
        fn input(&mut self, _: &str, _: &str, _: Option<&str>) -> Result<String, InteractionError> {
            unreachable!()
        }
    }

    #[test]
    fn making_public_asks_unless_yes_and_a_no_changes_nothing() {
        let config: Config = toml_edit::de::from_str(
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"visibility\"]\n",
        )
        .unwrap();
        let reference = RepositoryRef::parse("org/demo").unwrap();
        let remote = PublicRemote {
            changed: Mutex::new(Vec::new()),
        };
        let run = |answer: Result<bool, InteractionError>, yes: bool| {
            let mut interaction = Answer(answer, 0);
            let result = visibility(
                &config,
                &remote,
                &mut interaction,
                &reference,
                Visibility::Public,
                yes,
            );
            (result, interaction.1)
        };

        assert_eq!(run(Ok(false), false), (Err(ActionError::Declined), 1));
        assert_eq!(
            run(Err(InteractionError::Cancelled), false),
            (Err(ActionError::Cancelled), 1)
        );
        assert!(remote.changed.lock().unwrap().is_empty());
        assert_eq!(run(Ok(true), false), (Ok(Mutation::Changed), 1));
        assert_eq!(run(Ok(false), true), (Ok(Mutation::Changed), 0));
        assert_eq!(
            *remote.changed.lock().unwrap(),
            [Visibility::Public, Visibility::Public]
        );
    }

    /// A provider that holds every repository and counts deletions.
    #[derive(Default)]
    struct DeletingRemote(Mutex<usize>);

    impl RemoteRepositories for DeletingRemote {
        fn check(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn state(&self, _: &RepositoryRef) -> Result<Option<RemoteState>, String> {
            Ok(Some(RemoteState {
                archived: false,
                visibility: Visibility::Private,
            }))
        }
        fn archive(&self, _: &RepositoryRef, _: bool) -> Result<(), String> {
            unreachable!()
        }
        fn set_visibility(&self, _: &RepositoryRef, _: Visibility) -> Result<(), String> {
            unreachable!()
        }
        fn create(&self, _: &RepositoryRef, _: Visibility, _: Option<&str>) -> Result<(), String> {
            unreachable!()
        }
        fn publish(
            &self,
            _: &RepositoryRef,
            _: Visibility,
            _: Option<&str>,
            _: &Path,
        ) -> Result<(), String> {
            unreachable!()
        }
        fn check_delete_scope(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &RepositoryRef) -> Result<(), String> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    /// One existing checkout whose losses the test changes, and a config store behind it.
    #[derive(Default)]
    struct Checkout {
        losses: Arc<Mutex<Vec<String>>>,
        removed: Mutex<bool>,
    }

    impl GitRemoval for Checkout {
        fn inspect_removal(&self, _: &Path, _: &str) -> Result<Vec<String>, String> {
            Ok(self.losses.lock().unwrap().clone())
        }
        fn origin_identity(&self, _: &Path) -> Result<String, String> {
            unreachable!()
        }
    }

    impl RemovalFilesystem for Checkout {
        type Error = String;
        fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
            Ok(path.to_path_buf())
        }
        fn is_symlink(&self, _: &Path) -> Result<bool, String> {
            Ok(false)
        }
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn contains_symlink(&self, _: &Path) -> Result<bool, String> {
            Ok(false)
        }
        fn remove_dir_all(&self, _: &Path) -> Result<(), String> {
            *self.removed.lock().unwrap() = true;
            Ok(())
        }
        fn discover_checkouts(&self, _: &Path) -> Result<Vec<PathBuf>, String> {
            unreachable!()
        }
    }

    impl ConfigStore for Checkout {
        type Error = String;
        fn exists(&self, _: &Path) -> bool {
            unreachable!()
        }
        fn load(&self, _: &Path) -> Result<Config, String> {
            unreachable!()
        }
        fn init(&self, _: &Path, _: &str, _: bool) -> Result<(), String> {
            unreachable!()
        }
        fn register(
            &self,
            _: &Path,
            _: &[String],
            _: Option<&str>,
        ) -> Result<Vec<Mutation>, String> {
            unreachable!()
        }
        fn remove(&self, _: &Path, _: &[String]) -> Result<Vec<Mutation>, String> {
            Ok(vec![Mutation::Changed])
        }
    }

    /// Types `typed` at the prompt, after adding `appears` to the checkout's losses.
    struct Typing {
        typed: Result<String, InteractionError>,
        losses: Arc<Mutex<Vec<String>>>,
        appears: Option<&'static str>,
    }

    impl Typing {
        fn new(typed: &str) -> Self {
            Self {
                typed: Ok(typed.to_owned()),
                losses: Arc::default(),
                appears: None,
            }
        }
    }

    impl Interaction for Typing {
        fn confirm(&mut self, _: &str, _: bool) -> Result<bool, InteractionError> {
            unreachable!()
        }
        fn input(&mut self, _: &str, _: &str, _: Option<&str>) -> Result<String, InteractionError> {
            if let Some(loss) = self.appears {
                self.losses.lock().unwrap().push(loss.to_owned());
            }
            self.typed.clone()
        }
    }

    #[test]
    fn deleting_on_github_needs_the_exact_owner_and_name() {
        let reference = RepositoryRef::parse("github.com/org/demo").unwrap();
        let answer = |typed: Result<String, InteractionError>| {
            let mut interaction = Typing {
                typed,
                ..Typing::new("")
            };
            confirm_name(&mut interaction, &reference)
        };
        assert_eq!(answer(Ok("org/demo".to_owned())), Ok(()));
        for typed in ["", "demo", "github.com/org/demo", "org/demo ", "Org/Demo"] {
            assert_eq!(answer(Ok(typed.to_owned())), Err(ActionError::Declined));
        }
        assert_eq!(
            answer(Err(InteractionError::Cancelled)),
            Err(ActionError::Cancelled)
        );
    }

    #[test]
    fn delete_refuses_when_the_checkout_changes_during_the_prompt() {
        let config: Config = toml_edit::de::from_str(
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"delete\"]\n",
        )
        .unwrap();
        let reference = RepositoryRef::parse("org/demo").unwrap();
        let run = |mut interaction: Typing| {
            let remote = DeletingRemote::default();
            let checkout = Checkout {
                losses: Arc::clone(&interaction.losses),
                ..Checkout::default()
            };
            let result = delete(
                &config,
                &remote,
                &checkout,
                &checkout,
                &checkout,
                &mut interaction,
                Path::new("/config.toml"),
                &reference,
                false,
                false,
                Path::new("/home"),
            );
            let deleted = *remote.0.lock().unwrap();
            let removed = *checkout.removed.lock().unwrap();
            (result, deleted, removed)
        };

        assert_eq!(
            run(Typing::new("org/demo")),
            (
                Ok(Deleted {
                    removed: true,
                    unregistered: true
                }),
                1,
                true
            )
        );
        assert_eq!(
            run(Typing::new("org/other")),
            (Err(ActionError::Declined), 0, false)
        );
        let (result, deleted, removed) = run(Typing {
            appears: Some("untracked files"),
            ..Typing::new("org/demo")
        });
        assert_eq!(
            result,
            Err(ActionError::Failed(
                "the checkout changed while confirming; GitHub was not touched".to_owned()
            ))
        );
        assert_eq!((deleted, removed), (0, false));
    }
}
