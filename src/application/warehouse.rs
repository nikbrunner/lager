use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::ports::{
    ConfigStore, EnsureEvent, EnsureReporter, GitClient, GitRemoval, GitState, HookRunner,
    Interaction, InteractionError, ProviderCatalog, RemovalFilesystem,
};
use crate::domain::config::Config;
use crate::domain::repository::{RepositoryRef, RepositorySummary};
use crate::domain::state::LocalState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderError {
    pub provider: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListRow {
    pub identity: String,
    pub clone_url: String,
    pub source: String,
    pub destination: Option<String>,
    pub hook: bool,
    pub archived: bool,
    pub state: Option<LocalState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListReport {
    pub schema_version: u32,
    pub repositories: Vec<ListRow>,
    pub provider_errors: Vec<ProviderError>,
}

impl ListReport {
    pub fn failed(&self) -> bool {
        !self.provider_errors.is_empty()
    }
}

pub use crate::application::ports::{RepositorySelector, SelectionCandidate, SelectionError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerContext {
    Register,
    Add,
    Unregister,
    Hook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateReport {
    pub candidates: Vec<SelectionCandidate>,
    pub provider_errors: Vec<ProviderError>,
}

impl CandidateReport {
    pub fn failed(&self) -> bool {
        !self.provider_errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationStatus {
    Cloned,
    Noop,
    Hooked,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryOutcome {
    pub reference: String,
    pub status: OperationStatus,
}

// Keep typed cancellation separate from the public, display-only outcome.
struct CloneOutcome {
    outcome: RepositoryOutcome,
    cancelled: bool,
}

impl From<RepositoryOutcome> for CloneOutcome {
    fn from(outcome: RepositoryOutcome) -> Self {
        Self {
            outcome,
            cancelled: false,
        }
    }
}

impl CloneOutcome {
    fn with_cancelled(mut self, cancelled: bool) -> Self {
        self.cancelled = cancelled;
        self
    }
}

fn record_clone(result: &mut BatchOutcome, outcome: CloneOutcome) -> bool {
    result.cancelled |= outcome.cancelled;
    result.outcomes.push(outcome.outcome);
    result.cancelled
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalStatus {
    Removed,
    Absent,
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalOutcome {
    pub reference: String,
    pub status: RemovalStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalBatchOutcome {
    pub outcomes: Vec<RemovalOutcome>,
    pub cancelled: bool,
}

impl RemovalBatchOutcome {
    pub fn failed(&self) -> bool {
        self.cancelled
            || self
                .outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, RemovalStatus::Failed(_)))
    }
}

type EffectiveCandidates = (Vec<(RepositoryRef, Option<String>)>, Vec<ProviderError>);
type ParsedCloneInput = Result<(RepositoryRef, String), (String, String)>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub outcomes: Vec<RepositoryOutcome>,
    pub provider_errors: Vec<ProviderError>,
    pub cancelled: bool,
}

impl BatchOutcome {
    pub fn failed(&self) -> bool {
        self.cancelled
            || !self.provider_errors.is_empty()
            || self
                .outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, OperationStatus::Failed(_)))
    }
}

pub fn remove_candidates<S, G, F>(
    store: &S,
    git: &G,
    filesystem: &F,
    config_path: &Path,
    home: &Path,
) -> Result<CandidateReport, String>
where
    S: ConfigStore,
    G: GitRemoval,
    F: RemovalFilesystem,
{
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    if !filesystem
        .metadata_exists(&root)
        .map_err(|error| error.to_string())?
    {
        return Ok(CandidateReport {
            candidates: Vec::new(),
            provider_errors: Vec::new(),
        });
    }
    let root = filesystem
        .canonicalize(&root)
        .map_err(|error| error.to_string())?;
    let directories = filesystem
        .discover_directories(&root)
        .map_err(|error| error.to_string())?;
    let mut candidates = Vec::new();
    for directory in directories {
        if filesystem.is_symlink(&directory).unwrap_or(true)
            || filesystem.contains_symlink(&directory).unwrap_or(true)
        {
            continue;
        }
        let Ok(canonical) = filesystem.canonicalize(&directory) else {
            continue;
        };
        if canonical == root || !canonical.starts_with(&root) {
            continue;
        }
        let Ok(identity) = git.origin_identity(&canonical) else {
            continue;
        };
        if git.inspect_removal(&canonical, &identity).is_err() {
            continue;
        }
        let Ok(reference) = RepositoryRef::parse(&identity) else {
            continue;
        };
        candidates.push(SelectionCandidate {
            reference,
            display: format!("{}\t{}", identity, canonical.display()),
            archived: false,
            exact_path: Some(canonical),
        });
    }
    candidates.sort_by_key(|candidate| candidate.reference.identity());
    Ok(CandidateReport {
        candidates,
        provider_errors: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn remove_many<S, G, F>(
    store: &S,
    git: &G,
    filesystem: &F,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    references: &[String],
    unregister: Option<bool>,
    yes: bool,
    force: bool,
    interactive: bool,
    home: &Path,
) -> Result<RemovalBatchOutcome, String>
where
    S: ConfigStore,
    G: GitRemoval,
    F: RemovalFilesystem,
{
    validate_references(references.iter().map(String::as_str))?;
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let root = filesystem.canonicalize(&root).ok();
    let mut result = RemovalBatchOutcome::default();
    for input in references {
        let outcome = remove_one(
            store,
            git,
            filesystem,
            interaction,
            config_path,
            &config,
            root.as_deref(),
            input,
            None,
            unregister,
            yes,
            force,
            interactive,
            home,
        );
        let cancelled =
            matches!(outcome.status, RemovalStatus::Failed(ref error) if error == "cancelled");
        result.outcomes.push(outcome);
        if cancelled {
            result.cancelled = true;
            break;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn remove_selected_many<S, G, F>(
    store: &S,
    git: &G,
    filesystem: &F,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    selections: &[SelectionCandidate],
    unregister: Option<bool>,
    yes: bool,
    force: bool,
    interactive: bool,
    home: &Path,
) -> Result<RemovalBatchOutcome, String>
where
    S: ConfigStore,
    G: GitRemoval,
    F: RemovalFilesystem,
{
    validate_references(
        selections
            .iter()
            .map(|selection| selection.reference.clone_url.as_str()),
    )?;
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let configured_root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let root = filesystem.canonicalize(&configured_root).ok();
    let mut result = RemovalBatchOutcome::default();
    for selection in selections {
        let input = selection.reference.clone_url.as_str();
        let Some(exact_path) = selection.exact_path.as_deref() else {
            result.outcomes.push(removal_failed(
                input,
                "selected clone omitted its exact path",
            ));
            continue;
        };
        let outcome = remove_one(
            store,
            git,
            filesystem,
            interaction,
            config_path,
            &config,
            root.as_deref(),
            input,
            Some(exact_path),
            unregister,
            yes,
            force,
            interactive,
            home,
        );
        let cancelled =
            matches!(outcome.status, RemovalStatus::Failed(ref error) if error == "cancelled");
        result.outcomes.push(outcome);
        if cancelled {
            result.cancelled = true;
            break;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn remove_one<S, G, F>(
    store: &S,
    git: &G,
    filesystem: &F,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    config: &Config,
    root: Option<&Path>,
    input: &str,
    exact_destination: Option<&Path>,
    unregister: Option<bool>,
    yes: bool,
    force: bool,
    interactive: bool,
    home: &Path,
) -> RemovalOutcome
where
    S: ConfigStore,
    G: GitRemoval,
    F: RemovalFilesystem,
{
    let reference = match RepositoryRef::parse(input) {
        Ok(reference) if !reference.is_wildcard() => reference,
        Ok(_) => return removal_failed(input, "wildcard references cannot be removed"),
        Err(error) => return removal_failed(input, &error.to_string()),
    };
    let destination = match exact_destination {
        Some(path) => path.to_path_buf(),
        None => match destination(config, &reference, home) {
            Ok(path) => path,
            Err(error) => return removal_failed(input, &error),
        },
    };
    let exists = match filesystem.metadata_exists(&destination) {
        Ok(exists) => exists,
        Err(error) => return removal_failed(input, &error.to_string()),
    };
    if !exists {
        if exact_destination.is_some() {
            return removal_failed(input, "selected clone path no longer exists");
        }
        let unregister = match unregister_after_handling(interaction, input, unregister) {
            Ok(unregister) => unregister,
            Err(error) => return removal_failed(input, &error),
        };
        if unregister && let Err(error) = store.remove(config_path, &[input.to_owned()]) {
            return removal_failed(input, &error.to_string());
        }
        return RemovalOutcome {
            reference: input.to_owned(),
            status: RemovalStatus::Absent,
        };
    }
    let Some(root) = root else {
        return removal_failed(input, "configured root is unreadable");
    };
    let unsafe_path = match validate_removal_path(filesystem, root, &destination) {
        Ok(path) if exact_destination.is_none_or(|selected| path == selected) => path,
        Ok(_) => return removal_failed(input, "selected clone path changed after discovery"),
        Err(error) => return removal_failed(input, &error),
    };
    let warnings = match git.inspect_removal(&unsafe_path, &reference.clone_url) {
        Ok(warnings) => warnings,
        Err(error) => return removal_failed(input, &error),
    };
    if !warnings.is_empty() && !force {
        let prompt = format!(
            "Unsafe local state at {} ({}). Force removal?",
            destination.display(),
            warnings.join(", ")
        );
        match interaction.confirm(&prompt, false) {
            Ok(true) => {}
            Ok(false) => return removal_skipped(input),
            Err(InteractionError::Cancelled) => return removal_failed(input, "cancelled"),
            Err(InteractionError::Failed(error)) => return removal_failed(input, &error),
        }
    }
    if interactive && !yes {
        let prompt = format!("Remove {} permanently?", destination.display());
        match interaction.confirm(&prompt, false) {
            Ok(true) => {}
            Ok(false) => return removal_skipped(input),
            Err(InteractionError::Cancelled) => return removal_failed(input, "cancelled"),
            Err(InteractionError::Failed(error)) => return removal_failed(input, &error),
        }
    }
    let unsafe_path = match validate_removal_path(filesystem, root, &destination) {
        Ok(path) if exact_destination.is_none_or(|selected| path == selected) => path,
        Ok(_) => return removal_failed(input, "selected clone path changed before deletion"),
        Err(error) => return removal_failed(input, &error),
    };
    let final_warnings = match git.inspect_removal(&unsafe_path, &reference.clone_url) {
        Ok(warnings) => warnings,
        Err(error) => return removal_failed(input, &error),
    };
    if !force && final_warnings != warnings {
        return removal_failed(input, "local state changed before deletion");
    }
    if let Err(error) = filesystem.remove_dir_all(&unsafe_path) {
        return removal_failed(input, &error.to_string());
    }
    let unregister = match unregister_after_handling(interaction, input, unregister) {
        Ok(unregister) => unregister,
        Err(error) => return removal_failed(input, &error),
    };
    if unregister && let Err(error) = store.remove(config_path, &[input.to_owned()]) {
        return removal_failed(input, &error.to_string());
    }
    RemovalOutcome {
        reference: input.to_owned(),
        status: RemovalStatus::Removed,
    }
}

fn unregister_after_handling(
    interaction: &mut dyn Interaction,
    input: &str,
    unregister: Option<bool>,
) -> Result<bool, String> {
    match unregister {
        Some(unregister) => Ok(unregister),
        None => interaction
            .confirm(&format!("Unregister {input}?"), true)
            .map_err(|error| match error {
                InteractionError::Cancelled => "cancelled".to_owned(),
                InteractionError::Failed(message) => message,
            }),
    }
}

fn validate_removal_path<F: RemovalFilesystem>(
    filesystem: &F,
    root: &Path,
    destination: &Path,
) -> Result<PathBuf, String> {
    if filesystem
        .is_symlink(destination)
        .map_err(|error| error.to_string())?
    {
        return Err("destination is a symlink".to_owned());
    }
    let canonical = filesystem
        .canonicalize(destination)
        .map_err(|error| error.to_string())?;
    if canonical == root {
        return Err("refusing to remove configured root".to_owned());
    }
    if !canonical.starts_with(root) {
        return Err("destination escapes configured root".to_owned());
    }
    if filesystem
        .contains_symlink(destination)
        .map_err(|error| error.to_string())?
    {
        return Err("destination contains a symlink".to_owned());
    }
    Ok(canonical)
}

fn removal_failed(input: &str, error: &str) -> RemovalOutcome {
    RemovalOutcome {
        reference: input.to_owned(),
        status: RemovalStatus::Failed(error.to_owned()),
    }
}

fn removal_skipped(input: &str) -> RemovalOutcome {
    RemovalOutcome {
        reference: input.to_owned(),
        status: RemovalStatus::Skipped,
    }
}

pub fn clone_one<S: ConfigStore, G: GitClient>(
    store: &S,
    git: &G,
    config_path: &Path,
    reference: &str,
) -> Result<(), String> {
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let parsed = RepositoryRef::parse(reference).map_err(|error| error.to_string())?;
    if parsed.is_wildcard() {
        return Err("wildcard references cannot be cloned directly".to_owned());
    }
    let home = home_dir();
    let root = config
        .resolve_root(&home)
        .map_err(|error| error.to_string())?;
    let destination = destination(&config, &parsed, &home)?;
    if destination.exists() {
        return Err(format!(
            "destination already exists: {}",
            destination.display()
        ));
    }
    git.clone_repository(&parsed.clone_url, &home, &root, &destination)
        .map_err(|error| error.to_string())
}

pub fn clone_many_with_interaction<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    references: &[String],
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    validate_references(references.iter().map(String::as_str))?;
    let parsed = references
        .iter()
        .map(|input| {
            RepositoryRef::parse(input)
                .map(|reference| (reference, input.clone()))
                .map_err(|error| ("invalid repository reference".to_owned(), error.to_string()))
        })
        .collect::<Vec<_>>();
    clone_parsed_with_interaction(store, git, hooks, interaction, config_path, &parsed, home)
}

pub fn clone_references_with_interaction<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    references: &[RepositoryRef],
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    validate_references(
        references
            .iter()
            .map(|reference| reference.clone_url.as_str()),
    )?;
    let parsed = references
        .iter()
        .cloned()
        .map(|reference| Ok((reference.clone(), reference.clone_url)))
        .collect::<Vec<_>>();
    clone_parsed_with_interaction(store, git, hooks, interaction, config_path, &parsed, home)
}

fn clone_parsed_with_interaction<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    interaction: &mut dyn Interaction,
    config_path: &Path,
    references: &[ParsedCloneInput],
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let mut result = BatchOutcome::default();
    for parsed in references {
        let (reference, input) = match parsed {
            Ok(parsed) => parsed,
            Err((input, error)) => {
                result.outcomes.push(failed(input, error));
                continue;
            }
        };
        let outcome = clone_reference(
            store,
            git,
            hooks,
            config_path,
            &config,
            reference,
            input,
            false,
            None,
            home,
            None,
        );
        let succeeded = !matches!(outcome.outcome.status, OperationStatus::Failed(_));
        let fresh = matches!(outcome.outcome.status, OperationStatus::Cloned);
        if record_clone(&mut result, outcome) {
            break;
        }
        if !succeeded {
            continue;
        }
        let register = match interaction.confirm(&format!("Register {input}?"), true) {
            Ok(register) => register,
            Err(InteractionError::Cancelled) => {
                result.cancelled = true;
                break;
            }
            Err(InteractionError::Failed(error)) => {
                result.outcomes.last_mut().expect("outcome").status =
                    OperationStatus::Failed(error);
                continue;
            }
        };
        if !register {
            continue;
        }
        let post_clone =
            match crate::application::registry::registration_choice(input, None, true, interaction)
            {
                Ok(post_clone) => post_clone,
                Err(InteractionError::Cancelled) => {
                    result.cancelled = true;
                    break;
                }
                Err(InteractionError::Failed(error)) => {
                    result.outcomes.last_mut().expect("outcome").status =
                        OperationStatus::Failed(error);
                    continue;
                }
            };
        if let Err(error) = store.register(
            config_path,
            std::slice::from_ref(input),
            post_clone.as_deref(),
        ) {
            result.outcomes.last_mut().expect("outcome").status =
                OperationStatus::Failed(format!("could not persist declaration: {error}"));
            continue;
        }
        if fresh && let Some(command) = post_clone {
            let path = match destination(&config, reference, home) {
                Ok(path) => path,
                Err(error) => {
                    result.outcomes.last_mut().expect("outcome").status =
                        OperationStatus::Failed(error);
                    continue;
                }
            };
            if let Err(error) = hooks.run_hook(&command, &path) {
                result.cancelled = hooks.is_cancelled(&error);
                result.outcomes.last_mut().expect("outcome").status =
                    OperationStatus::Failed(error.to_string());
                if result.cancelled {
                    break;
                }
            }
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn clone_many<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    config_path: &Path,
    references: &[String],
    add: bool,
    post_clone: Option<&str>,
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    validate_references(references.iter().map(String::as_str))?;
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let mut result = BatchOutcome::default();
    for reference in references {
        let outcome = clone_candidate(
            store,
            git,
            hooks,
            config_path,
            &config,
            reference,
            add || post_clone.is_some(),
            post_clone,
            home,
        );
        if record_clone(&mut result, outcome) {
            break;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn clone_references<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    config_path: &Path,
    references: &[RepositoryRef],
    add: bool,
    post_clone: Option<&str>,
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    validate_references(
        references
            .iter()
            .map(|reference| reference.clone_url.as_str()),
    )?;
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let mut result = BatchOutcome::default();
    for reference in references {
        let outcome = clone_reference(
            store,
            git,
            hooks,
            config_path,
            &config,
            reference,
            &reference.clone_url,
            add || post_clone.is_some(),
            post_clone,
            home,
            None,
        );
        if record_clone(&mut result, outcome) {
            break;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn ensure<S, G, H, P>(
    store: &S,
    git: &G,
    hooks: &H,
    providers: &P,
    config_path: &Path,
    include_archived: bool,
    home: &Path,
    reporter: &mut dyn EnsureReporter,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
    P: ProviderCatalog,
{
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let (candidates, provider_errors) =
        effective_declarations(&config, providers, include_archived)?;
    let mut result = BatchOutcome {
        outcomes: Vec::new(),
        provider_errors,
        cancelled: false,
    };
    for (reference, hook) in candidates {
        let outcome = clone_reference(
            store,
            git,
            hooks,
            config_path,
            &config,
            &reference,
            &reference.clone_url,
            false,
            hook.as_deref(),
            home,
            Some(reporter),
        );
        if record_clone(&mut result, outcome) {
            break;
        }
    }
    Ok(result)
}

pub fn hook<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    config_path: &Path,
    references: &[String],
    home: &Path,
) -> Result<BatchOutcome, String>
where
    S: ConfigStore,
    G: GitState,
    H: HookRunner,
{
    validate_references(references.iter().map(String::as_str))?;
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let mut result = BatchOutcome::default();
    for input in references {
        let outcome = match RepositoryRef::parse(input) {
            Ok(reference) if !reference.is_wildcard() => {
                let declaration = config.repositories.iter().find(|declaration| {
                    RepositoryRef::parse(&declaration.url)
                        .ok()
                        .filter(|candidate| !candidate.is_wildcard())
                        .is_some_and(|candidate| candidate.identity() == reference.identity())
                });
                match declaration {
                    None => OperationStatus::Failed(
                        "repository is not an explicit declaration".to_owned(),
                    ),
                    Some(declaration) => match destination(&config, &reference, home) {
                        Ok(path)
                            if matches!(
                                git.classify_destination(&path, &reference.clone_url),
                                LocalState::Cloned
                            ) =>
                        {
                            match declaration.post_clone.as_deref() {
                                Some(command) => hooks
                                    .run_hook(command, &path)
                                    .map(|()| OperationStatus::Hooked)
                                    .unwrap_or_else(|error| {
                                        result.cancelled = hooks.is_cancelled(&error);
                                        OperationStatus::Failed(error.to_string())
                                    }),
                                None => OperationStatus::Noop,
                            }
                        }
                        Ok(_) => OperationStatus::Failed(
                            "destination is not a matching clone".to_owned(),
                        ),
                        Err(error) => OperationStatus::Failed(error),
                    },
                }
            }
            Ok(_) => {
                OperationStatus::Failed("wildcard references cannot be hooked directly".to_owned())
            }
            Err(error) => OperationStatus::Failed(error.to_string()),
        };
        result.outcomes.push(RepositoryOutcome {
            reference: input.clone(),
            status: outcome,
        });
        if result.cancelled {
            break;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn clone_candidate<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    config_path: &Path,
    config: &Config,
    input: &str,
    add: bool,
    requested_hook: Option<&str>,
    home: &Path,
) -> CloneOutcome
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    let parsed = match RepositoryRef::parse(input) {
        Ok(reference) if !reference.is_wildcard() => reference,
        Ok(_) => return failed(input, "wildcard references cannot be cloned directly").into(),
        Err(error) => return failed(input, &error.to_string()).into(),
    };
    clone_reference(
        store,
        git,
        hooks,
        config_path,
        config,
        &parsed,
        input,
        add,
        requested_hook,
        home,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn clone_reference<S, G, H>(
    store: &S,
    git: &G,
    hooks: &H,
    config_path: &Path,
    config: &Config,
    parsed: &RepositoryRef,
    input: &str,
    add: bool,
    requested_hook: Option<&str>,
    home: &Path,
    mut reporter: Option<&mut dyn EnsureReporter>,
) -> CloneOutcome
where
    S: ConfigStore,
    G: GitClient + GitState,
    H: HookRunner,
{
    if parsed.is_wildcard() {
        return reported_failure(
            &mut reporter,
            input,
            "wildcard references cannot be cloned directly",
        );
    }
    let root = match config.resolve_root(home) {
        Ok(root) => root,
        Err(error) => return reported_failure(&mut reporter, input, &error.to_string()),
    };
    let destination = match destination(config, parsed, home) {
        Ok(destination) => destination,
        Err(error) => return reported_failure(&mut reporter, input, &error),
    };
    let state = git.classify_destination(&destination, &parsed.clone_url);
    let fresh = match state {
        LocalState::Missing => {
            if let Some(reporter) = reporter.as_deref_mut() {
                reporter.report(EnsureEvent::CloneStarted {
                    reference: input,
                    destination: &destination,
                });
            }
            if let Err(error) = git.clone_repository(&parsed.clone_url, home, &root, &destination) {
                let cancelled = git.is_cancelled(&error);
                return reported_failure(&mut reporter, input, &error.to_string())
                    .with_cancelled(cancelled);
            }
            true
        }
        LocalState::Cloned => false,
        LocalState::Conflict => {
            return reported_failure(
                &mut reporter,
                input,
                "destination conflicts with repository",
            );
        }
        LocalState::Unreadable => {
            return reported_failure(&mut reporter, input, "destination is unreadable");
        }
    };

    if add {
        let command = requested_hook;
        if let Err(error) = store.register(config_path, &[input.to_owned()], command) {
            return reported_failure(
                &mut reporter,
                input,
                &format!("could not persist declaration: {error}"),
            );
        }
    }

    let configured_hook = config.repositories.iter().find_map(|declaration| {
        RepositoryRef::parse(&declaration.url)
            .ok()
            .filter(|reference| {
                !reference.is_wildcard() && reference.identity() == parsed.identity()
            })
            .and(declaration.post_clone.as_deref())
    });
    if fresh {
        if let Some(command) = requested_hook.or(configured_hook)
            && let Err(error) = hooks.run_hook(command, &destination)
        {
            let cancelled = hooks.is_cancelled(&error);
            return reported_failure(&mut reporter, input, &error.to_string())
                .with_cancelled(cancelled);
        }
        if let Some(reporter) = reporter {
            reporter.report(EnsureEvent::CloneSucceeded { reference: input });
        }
        RepositoryOutcome {
            reference: input.to_owned(),
            status: OperationStatus::Cloned,
        }
        .into()
    } else {
        RepositoryOutcome {
            reference: input.to_owned(),
            status: OperationStatus::Noop,
        }
        .into()
    }
}

fn reported_failure(
    reporter: &mut Option<&mut dyn EnsureReporter>,
    reference: &str,
    error: &str,
) -> CloneOutcome {
    if let Some(reporter) = reporter.as_deref_mut() {
        reporter.report(EnsureEvent::CloneFailed { reference, error });
    }
    failed(reference, error).into()
}

fn validate_references<'a>(references: impl Iterator<Item = &'a str>) -> Result<(), String> {
    for reference in references {
        crate::domain::repository::validate_reference_safety(reference)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn effective_declarations<P: ProviderCatalog>(
    config: &Config,
    providers: &P,
    include_archived: bool,
) -> Result<EffectiveCandidates, String> {
    let explicit: HashSet<String> = config
        .repositories
        .iter()
        .filter_map(|declaration| RepositoryRef::parse(&declaration.url).ok())
        .filter(|reference| !reference.is_wildcard())
        .map(|reference| reference.identity())
        .collect();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let mut provider_errors = Vec::new();
    for declaration in &config.repositories {
        let pattern = match RepositoryRef::parse(&declaration.url) {
            Ok(pattern) => pattern,
            Err(error) => return Err(error.to_string()),
        };
        if !pattern.is_wildcard() {
            if seen.insert(pattern.identity()) {
                candidates.push((pattern, declaration.post_clone.clone()));
            }
            continue;
        }
        let mut expanded = match providers.expand(&pattern, include_archived) {
            Ok(expanded) => expanded,
            Err(error) => {
                provider_errors.push(ProviderError {
                    provider: pattern.host.clone(),
                    error,
                });
                continue;
            }
        };
        validate_references(
            expanded
                .iter()
                .map(|summary| summary.reference.clone_url.as_str()),
        )?;
        expanded.sort_by_key(|summary| summary.reference.identity());
        for summary in expanded {
            let identity = summary.reference.identity();
            let relative = summary
                .reference
                .path
                .strip_prefix(&(pattern.path.clone() + "/"))
                .unwrap_or(&summary.reference.path);
            if declaration
                .exclude
                .iter()
                .any(|excluded| excluded == relative)
                || explicit.contains(&identity)
                || !seen.insert(identity)
            {
                continue;
            }
            candidates.push((summary.reference, None));
        }
    }
    Ok((candidates, provider_errors))
}

pub fn list<S, G, P>(
    store: &S,
    git: &G,
    providers: &P,
    config_path: &Path,
    remote: bool,
    include_archived: bool,
    home: &Path,
) -> Result<ListReport, String>
where
    S: ConfigStore,
    G: GitState,
    P: ProviderCatalog,
{
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    if include_archived && !remote {
        return Err("--include-archived requires --remote".to_owned());
    }

    let mut rows = Vec::new();
    let mut provider_errors = Vec::new();
    let explicit: HashSet<String> = config
        .repositories
        .iter()
        .filter_map(|declaration| RepositoryRef::parse(&declaration.url).ok())
        .filter(|reference| !reference.is_wildcard())
        .map(|reference| reference.identity())
        .collect();
    let mut seen = HashSet::new();

    for declaration in &config.repositories {
        let reference = match RepositoryRef::parse(&declaration.url) {
            Ok(reference) => reference,
            Err(error) => {
                provider_errors.push(ProviderError {
                    provider: "config".to_owned(),
                    error: error.to_string(),
                });
                continue;
            }
        };
        if !reference.is_wildcard() {
            if !seen.insert(reference.identity()) {
                continue;
            }
            rows.push(concrete_row(
                &config,
                git,
                &reference,
                "explicit",
                false,
                declaration.post_clone.is_some(),
                home,
            )?);
            continue;
        }

        if !remote {
            rows.push(ListRow {
                identity: format!("{}/*", reference.identity()),
                clone_url: reference.clone_url.clone(),
                source: "wildcard".to_owned(),
                destination: Some(
                    destination(&config, &reference, home)?
                        .display()
                        .to_string(),
                ),
                hook: declaration.post_clone.is_some(),
                archived: false,
                state: None,
                pattern: Some(declaration.url.clone()),
                exclusions: declaration.exclude.clone(),
            });
            continue;
        }

        let mut expanded = match providers.expand(&reference, include_archived) {
            Ok(expanded) => expanded,
            Err(error) => {
                provider_errors.push(ProviderError {
                    provider: reference.host.clone(),
                    error,
                });
                continue;
            }
        };
        validate_references(
            expanded
                .iter()
                .map(|summary| summary.reference.clone_url.as_str()),
        )?;
        expanded.sort_by_key(|summary| summary.reference.identity());
        for summary in expanded {
            if summary.archived && !include_archived {
                continue;
            }
            let identity = summary.reference.identity();
            let relative = summary
                .reference
                .path
                .strip_prefix(&(reference.path.clone() + "/"))
                .unwrap_or(&summary.reference.path);
            if declaration
                .exclude
                .iter()
                .any(|excluded| excluded == relative)
                || explicit.contains(&identity)
                || !seen.insert(identity)
            {
                continue;
            }
            rows.push(concrete_row(
                &config,
                git,
                &summary.reference,
                "wildcard",
                summary.archived,
                declaration.post_clone.is_some(),
                home,
            )?);
        }
    }

    Ok(ListReport {
        schema_version: 1,
        repositories: rows,
        provider_errors,
    })
}

fn concrete_row<G: GitState>(
    config: &Config,
    git: &G,
    reference: &RepositoryRef,
    source: &str,
    archived: bool,
    hook: bool,
    home: &Path,
) -> Result<ListRow, String> {
    let path = destination(config, reference, home)?;
    Ok(ListRow {
        identity: reference.identity(),
        clone_url: reference.clone_url.clone(),
        source: source.to_owned(),
        destination: Some(path.display().to_string()),
        hook,
        archived,
        state: Some(git.classify_destination(&path, &reference.clone_url)),
        pattern: None,
        exclusions: Vec::new(),
    })
}

pub fn picker_candidates<S, G, P>(
    store: &S,
    git: &G,
    providers: &P,
    config_path: &Path,
    context: PickerContext,
    include_archived: bool,
    home: &Path,
) -> Result<CandidateReport, String>
where
    S: ConfigStore,
    G: GitState,
    P: ProviderCatalog,
{
    let config = store.load(config_path).map_err(|error| error.to_string())?;
    let mut report = CandidateReport {
        candidates: Vec::new(),
        provider_errors: Vec::new(),
    };
    let explicit: HashSet<String> = config
        .repositories
        .iter()
        .filter_map(|declaration| RepositoryRef::parse(&declaration.url).ok())
        .filter(|reference| !reference.is_wildcard())
        .map(|reference| reference.identity())
        .collect();
    let mut effective = explicit.clone();
    let mut summaries = Vec::new();

    if !matches!(context, PickerContext::Unregister | PickerContext::Hook) {
        let (catalog, failures) = providers.catalog_with_failures(include_archived);
        summaries.extend(catalog);
        report
            .provider_errors
            .extend(failures.into_iter().map(|failure| ProviderError {
                provider: failure.provider,
                error: failure.error,
            }));
    }
    for declaration in &config.repositories {
        let Ok(pattern) = RepositoryRef::parse(&declaration.url) else {
            continue;
        };
        if !pattern.is_wildcard() {
            if matches!(context, PickerContext::Unregister | PickerContext::Hook) {
                summaries.push(RepositorySummary {
                    reference: pattern,
                    archived: false,
                });
            }
            continue;
        }
        if matches!(context, PickerContext::Hook) {
            continue;
        }
        match providers.expand(&pattern, include_archived) {
            Ok(expanded) => {
                for summary in expanded {
                    if summary.archived && !include_archived {
                        continue;
                    }
                    let relative = summary
                        .reference
                        .path
                        .strip_prefix(&(pattern.path.clone() + "/"))
                        .unwrap_or(&summary.reference.path);
                    if !declaration.exclude.iter().any(|item| item == relative) {
                        if matches!(context, PickerContext::Unregister) {
                            summaries.push(summary);
                        } else if effective.insert(summary.reference.identity()) {
                            // This identity is effectively managed by a wildcard.
                        }
                    }
                }
            }
            Err(error) => report.provider_errors.push(ProviderError {
                provider: pattern.host,
                error,
            }),
        }
    }

    validate_references(
        summaries
            .iter()
            .map(|summary| summary.reference.clone_url.as_str()),
    )?;
    let mut seen = HashSet::new();
    for summary in summaries {
        if summary.archived && !include_archived {
            continue;
        }
        let identity = summary.reference.identity();
        let include = match context {
            PickerContext::Register => !effective.contains(&identity),
            PickerContext::Add => {
                let path = destination(&config, &summary.reference, home)?;
                !matches!(
                    git.classify_destination(&path, &summary.reference.clone_url),
                    LocalState::Cloned
                )
            }
            PickerContext::Unregister | PickerContext::Hook => true,
        };
        if include && seen.insert(identity.clone()) {
            report.candidates.push(SelectionCandidate {
                reference: summary.reference,
                display: format!(
                    "{}{}",
                    identity,
                    if summary.archived { " [archived]" } else { "" }
                ),
                archived: summary.archived,
                exact_path: None,
            });
        }
    }
    Ok(report)
}

fn destination(config: &Config, reference: &RepositoryRef, home: &Path) -> Result<PathBuf, String> {
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let Some(provider) = config.providers.get(&reference.host) else {
        return Ok(reference.destination(&root));
    };
    let mut segments: Vec<&str> = provider
        .prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let mut remote: Vec<&str> = reference.path.split('/').collect();
    if provider.preset == "bitbucket-data-center"
        && let Some(project) = remote.first_mut()
    {
        *project = project.strip_prefix('~').unwrap_or(project);
    }
    segments.extend(remote);
    Ok(segments
        .into_iter()
        .fold(root, |path, segment| path.join(segment)))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn failed(input: &str, error: &str) -> RepositoryOutcome {
    RepositoryOutcome {
        reference: input.to_owned(),
        status: OperationStatus::Failed(error.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::application::ports::{
        EnsureEvent, EnsureReporter, Interaction, InteractionError, Mutation, ProviderCatalog,
    };
    use crate::domain::config::Config;
    use crate::domain::repository::RepositorySummary;

    struct FakeStore {
        config: Config,
    }

    impl ConfigStore for FakeStore {
        type Error = String;

        fn exists(&self, _path: &Path) -> bool {
            true
        }
        fn load(&self, _path: &Path) -> Result<Config, Self::Error> {
            Ok(self.config.clone())
        }
        fn init(&self, _path: &Path, _root: &str, _github: bool) -> Result<(), Self::Error> {
            Ok(())
        }
        fn register(
            &self,
            _path: &Path,
            _references: &[String],
            _post_clone: Option<&str>,
        ) -> Result<Vec<Mutation>, Self::Error> {
            Ok(vec![Mutation::Changed])
        }
        fn remove(
            &self,
            _path: &Path,
            _references: &[String],
        ) -> Result<Vec<Mutation>, Self::Error> {
            Ok(vec![])
        }
    }

    struct FakeGit {
        cloned: RefCell<Vec<String>>,
    }

    impl GitClient for FakeGit {
        type Error = String;
        fn clone_repository(
            &self,
            url: &str,
            _home: &Path,
            _root: &Path,
            _destination: &Path,
        ) -> Result<(), Self::Error> {
            if url.contains("bad") {
                return Err("clone failed".to_owned());
            }
            self.cloned.borrow_mut().push(url.to_owned());
            Ok(())
        }
    }

    impl GitState for FakeGit {
        fn classify_destination(&self, _destination: &Path, _expected_url: &str) -> LocalState {
            LocalState::Missing
        }
    }

    struct EventGit<'a> {
        events: &'a RefCell<Vec<String>>,
    }

    impl GitClient for EventGit<'_> {
        type Error = String;

        fn clone_repository(
            &self,
            url: &str,
            _home: &Path,
            _root: &Path,
            _destination: &Path,
        ) -> Result<(), Self::Error> {
            self.events.borrow_mut().push(format!("git:{url}"));
            Ok(())
        }
    }

    impl GitState for EventGit<'_> {
        fn classify_destination(&self, _destination: &Path, _expected_url: &str) -> LocalState {
            LocalState::Missing
        }
    }

    struct RecordingEnsureReporter<'a> {
        events: &'a RefCell<Vec<String>>,
    }

    impl EnsureReporter for RecordingEnsureReporter<'_> {
        fn report(&mut self, event: EnsureEvent<'_>) {
            let event = match event {
                EnsureEvent::CloneStarted {
                    reference,
                    destination,
                } => format!("start:{reference}:{}", destination.display()),
                EnsureEvent::CloneSucceeded { reference } => format!("success:{reference}"),
                EnsureEvent::CloneFailed { reference, error } => {
                    format!("failure:{reference}:{error}")
                }
            };
            self.events.borrow_mut().push(event);
        }
    }

    impl GitRemoval for FakeGit {
        fn inspect_removal(
            &self,
            _destination: &Path,
            _expected_url: &str,
        ) -> Result<Vec<String>, String> {
            Ok(vec!["stashes".to_owned()])
        }

        fn origin_identity(&self, _destination: &Path) -> Result<String, String> {
            Ok("github.com/org/repo".to_owned())
        }
    }

    struct FakeInteraction {
        answer: Result<bool, InteractionError>,
        prompts: RefCell<Vec<String>>,
    }

    impl Interaction for FakeInteraction {
        fn confirm(&mut self, message: &str, _default: bool) -> Result<bool, InteractionError> {
            self.prompts.borrow_mut().push(message.to_owned());
            self.answer.clone()
        }

        fn input(
            &mut self,
            _message: &str,
            _placeholder: &str,
            _default: Option<&str>,
        ) -> Result<String, InteractionError> {
            unreachable!()
        }
    }

    struct QueueInteraction {
        answers: VecDeque<Result<bool, InteractionError>>,
        inputs: VecDeque<Result<String, InteractionError>>,
        prompts: Vec<(String, bool)>,
    }

    impl Interaction for QueueInteraction {
        fn confirm(&mut self, message: &str, default: bool) -> Result<bool, InteractionError> {
            self.prompts.push((message.to_owned(), default));
            self.answers.pop_front().expect("confirmation answer")
        }

        fn input(
            &mut self,
            message: &str,
            _placeholder: &str,
            _default: Option<&str>,
        ) -> Result<String, InteractionError> {
            self.prompts.push((message.to_owned(), false));
            self.inputs.pop_front().expect("input answer")
        }
    }

    struct RecordingStore {
        config: Config,
        registered: RefCell<Vec<(String, Option<String>)>>,
        removed: RefCell<Vec<String>>,
    }

    impl ConfigStore for RecordingStore {
        type Error = String;

        fn exists(&self, _path: &Path) -> bool {
            true
        }
        fn load(&self, _path: &Path) -> Result<Config, Self::Error> {
            Ok(self.config.clone())
        }
        fn init(&self, _path: &Path, _root: &str, _github: bool) -> Result<(), Self::Error> {
            unreachable!()
        }
        fn register(
            &self,
            _path: &Path,
            references: &[String],
            post_clone: Option<&str>,
        ) -> Result<Vec<Mutation>, Self::Error> {
            self.registered.borrow_mut().extend(
                references
                    .iter()
                    .cloned()
                    .map(|reference| (reference, post_clone.map(str::to_owned))),
            );
            Ok(vec![Mutation::Changed; references.len()])
        }
        fn remove(
            &self,
            _path: &Path,
            references: &[String],
        ) -> Result<Vec<Mutation>, Self::Error> {
            self.removed.borrow_mut().extend_from_slice(references);
            Ok(vec![Mutation::Changed; references.len()])
        }
    }

    struct PermissionDeniedRemovalFilesystem;

    impl RemovalFilesystem for PermissionDeniedRemovalFilesystem {
        type Error = std::io::Error;

        fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error> {
            std::fs::canonicalize(path)
        }
        fn is_symlink(&self, path: &Path) -> Result<bool, Self::Error> {
            Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
        }
        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn metadata_exists(&self, _path: &Path) -> Result<bool, Self::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission denied",
            ))
        }
        fn contains_symlink(&self, _path: &Path) -> Result<bool, Self::Error> {
            Ok(false)
        }
        fn remove_dir_all(&self, _path: &Path) -> Result<(), Self::Error> {
            panic!("unreadable paths must not be removed")
        }
        fn discover_directories(&self, _root: &Path) -> Result<Vec<PathBuf>, Self::Error> {
            Ok(Vec::new())
        }
    }

    struct PartialRemovalFilesystem;

    impl RemovalFilesystem for PartialRemovalFilesystem {
        type Error = std::io::Error;

        fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error> {
            std::fs::canonicalize(path)
        }
        fn is_symlink(&self, path: &Path) -> Result<bool, Self::Error> {
            Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
        }
        fn exists(&self, path: &Path) -> bool {
            std::fs::symlink_metadata(path).is_ok()
        }
        fn contains_symlink(&self, _path: &Path) -> Result<bool, Self::Error> {
            Ok(false)
        }
        fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error> {
            std::fs::remove_file(path.join("sentinel"))?;
            Err(std::io::Error::other("simulated partial removal"))
        }
        fn discover_directories(&self, _root: &Path) -> Result<Vec<PathBuf>, Self::Error> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct FakeHooks {
        commands: RefCell<Vec<String>>,
    }

    impl HookRunner for FakeHooks {
        type Error = String;
        fn run_hook(&self, command: &str, _directory: &Path) -> Result<(), Self::Error> {
            self.commands.borrow_mut().push(command.to_owned());
            Ok(())
        }
    }

    #[derive(Default)]
    struct NoProviders;
    impl ProviderCatalog for NoProviders {
        fn catalog(&self, _include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
            Ok(vec![])
        }
        fn expand(
            &self,
            _reference: &RepositoryRef,
            _include_archived: bool,
        ) -> Result<Vec<RepositorySummary>, String> {
            Ok(vec![])
        }
    }

    fn config() -> Config {
        Config {
            root: "repos".to_owned(),
            providers: Default::default(),
            repositories: vec![],
        }
    }

    struct FakeCatalog {
        expansion: Result<Vec<RepositorySummary>, String>,
    }

    impl ProviderCatalog for FakeCatalog {
        fn catalog(&self, _include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
            Ok(vec![])
        }

        fn expand(
            &self,
            _reference: &RepositoryRef,
            _include_archived: bool,
        ) -> Result<Vec<RepositorySummary>, String> {
            self.expansion.clone()
        }
    }

    fn summary(reference: &str, archived: bool) -> RepositorySummary {
        RepositorySummary {
            reference: RepositoryRef::parse(reference).unwrap(),
            archived,
        }
    }

    #[test]
    fn root_self_and_path_escape_guards_reject_canonical_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let filesystem = crate::infrastructure::filesystem::Filesystem;
        let canonical_root = filesystem.canonicalize(&root).unwrap();
        let canonical_child = filesystem.canonicalize(&child).unwrap();
        let canonical_outside = filesystem.canonicalize(&outside).unwrap();
        assert!(validate_removal_path(&filesystem, &canonical_root, &root).is_err());
        assert!(validate_removal_path(&filesystem, &canonical_root, &outside).is_err());
        assert!(canonical_child.starts_with(&canonical_root));
        assert!(!canonical_outside.starts_with(&canonical_root));
    }

    #[test]
    fn removal_warning_can_be_skipped_without_touching_disk() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let destination = home.join("repos/org/repo");
        std::fs::create_dir_all(destination.join(".git")).unwrap();
        let mut interaction = FakeInteraction {
            answer: Ok(false),
            prompts: RefCell::new(Vec::new()),
        };
        let result = remove_many(
            &FakeStore { config: config() },
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &crate::infrastructure::filesystem::Filesystem,
            &mut interaction,
            Path::new("config"),
            &["github.com/org/repo".to_owned()],
            Some(false),
            false,
            false,
            true,
            &home,
        )
        .unwrap();
        assert_eq!(result.outcomes[0].status, RemovalStatus::Skipped);
        assert!(destination.exists());
        assert!(interaction.prompts.borrow()[0].contains(&destination.display().to_string()));
    }

    #[test]
    fn tty_remove_asks_to_unregister_after_absent_disk_handling_with_yes_default() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let store = RecordingStore {
            config: config(),
            registered: RefCell::new(Vec::new()),
            removed: RefCell::new(Vec::new()),
        };
        let mut interaction = QueueInteraction {
            answers: [Ok(true)].into(),
            inputs: VecDeque::new(),
            prompts: Vec::new(),
        };
        let result = remove_many(
            &store,
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &crate::infrastructure::filesystem::Filesystem,
            &mut interaction,
            Path::new("config"),
            &["github.com/org/repo".to_owned()],
            None,
            false,
            true,
            true,
            &home,
        )
        .unwrap();
        assert_eq!(result.outcomes[0].status, RemovalStatus::Absent);
        assert_eq!(store.removed.borrow().as_slice(), ["github.com/org/repo"]);
        assert_eq!(
            interaction.prompts,
            [("Unregister github.com/org/repo?".to_owned(), true)]
        );
    }

    #[test]
    fn remove_cancellation_stops_the_batch_before_later_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let store = RecordingStore {
            config: config(),
            registered: RefCell::new(Vec::new()),
            removed: RefCell::new(Vec::new()),
        };
        let mut interaction = QueueInteraction {
            answers: [Err(InteractionError::Cancelled), Ok(true)].into(),
            inputs: VecDeque::new(),
            prompts: Vec::new(),
        };
        let result = remove_many(
            &store,
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &crate::infrastructure::filesystem::Filesystem,
            &mut interaction,
            Path::new("config"),
            &[
                "github.com/org/one".to_owned(),
                "github.com/org/two".to_owned(),
            ],
            None,
            false,
            true,
            true,
            &home,
        )
        .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.outcomes.len(), 1);
        assert!(store.removed.borrow().is_empty());
        assert_eq!(interaction.answers.len(), 1);
    }

    #[test]
    fn unreadable_destination_is_a_failure_not_an_absent_unregister() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let mut interaction = FakeInteraction {
            answer: Ok(true),
            prompts: RefCell::new(Vec::new()),
        };
        let result = remove_many(
            &FakeStore { config: config() },
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &PermissionDeniedRemovalFilesystem,
            &mut interaction,
            Path::new("config"),
            &["github.com/org/repo".to_owned()],
            Some(true),
            true,
            true,
            false,
            &home,
        )
        .unwrap();
        assert!(matches!(
            result.outcomes[0].status,
            RemovalStatus::Failed(_)
        ));
        assert!(interaction.prompts.borrow().is_empty());
    }

    #[test]
    fn failed_disk_removal_leaves_destination_and_registration_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let destination = home.join("repos/org/repo");
        let config_path = temp.path().join("config.toml");
        std::fs::create_dir_all(destination.join(".git")).unwrap();
        std::fs::write(destination.join("sentinel"), "partial\n").unwrap();
        std::fs::write(
            &config_path,
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/repo\"\n",
        )
        .unwrap();
        let mut interaction = FakeInteraction {
            answer: Ok(true),
            prompts: RefCell::new(Vec::new()),
        };
        let result = remove_many(
            &crate::infrastructure::config::FileConfigStore,
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &PartialRemovalFilesystem,
            &mut interaction,
            &config_path,
            &["github.com/org/repo".to_owned()],
            Some(true),
            true,
            true,
            false,
            &home,
        )
        .unwrap();
        assert!(matches!(
            result.outcomes[0].status,
            RemovalStatus::Failed(_)
        ));
        assert!(destination.exists());
        assert!(!destination.join("sentinel").exists());
        assert!(
            std::fs::read_to_string(config_path)
                .unwrap()
                .contains("github.com/org/repo")
        );
    }

    #[test]
    fn list_offline_reports_explicit_state_and_wildcard_metadata() {
        let mut config = config();
        config.repositories = vec![
            crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/explicit".to_owned(),
                post_clone: Some("hook".to_owned()),
                exclude: vec![],
            },
            crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/*".to_owned(),
                post_clone: None,
                exclude: vec!["skip".to_owned()],
            },
        ];
        let report = list(
            &FakeStore { config },
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &NoProviders,
            Path::new("config"),
            false,
            false,
            Path::new("/tmp/lager-test-home"),
        )
        .unwrap();

        assert_eq!(report.schema_version, 1);
        assert_eq!(report.repositories.len(), 2);
        assert_eq!(report.repositories[0].source, "explicit");
        assert_eq!(report.repositories[0].state, Some(LocalState::Missing));
        assert!(report.repositories[0].hook);
        assert_eq!(
            report.repositories[1].pattern.as_deref(),
            Some("github.com/org/*")
        );
        assert_eq!(report.repositories[1].exclusions, ["skip"]);
    }

    #[test]
    fn remote_list_deduplicates_explicit_and_excluded_wildcard_members() {
        let mut config = config();
        config.repositories = vec![
            crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/explicit".to_owned(),
                post_clone: None,
                exclude: vec![],
            },
            crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/*".to_owned(),
                post_clone: None,
                exclude: vec!["skip".to_owned()],
            },
        ];
        let report = list(
            &FakeStore { config },
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &FakeCatalog {
                expansion: Ok(vec![
                    summary("github.com/org/skip", false),
                    summary("github.com/org/explicit", false),
                    summary("github.com/org/member", true),
                    summary("github.com/org/member", true),
                ]),
            },
            Path::new("config"),
            true,
            true,
            Path::new("/tmp/lager-test-home"),
        )
        .unwrap();

        assert!(report.provider_errors.is_empty());
        assert_eq!(report.repositories.len(), 2);
        assert_eq!(report.repositories[0].identity, "github.com/org/explicit");
        assert_eq!(report.repositories[1].identity, "github.com/org/member");
        assert!(report.repositories[1].archived);
    }

    #[test]
    fn remote_list_keeps_rows_when_provider_expansion_fails() {
        let mut config = config();
        config
            .repositories
            .push(crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/*".to_owned(),
                post_clone: None,
                exclude: vec![],
            });
        let report = list(
            &FakeStore { config },
            &FakeGit {
                cloned: RefCell::new(vec![]),
            },
            &FakeCatalog {
                expansion: Err("offline".to_owned()),
            },
            Path::new("config"),
            true,
            false,
            Path::new("/tmp/lager-test-home"),
        )
        .unwrap();
        assert!(report.repositories.is_empty());
        assert_eq!(report.provider_errors[0].error, "offline");
        assert!(report.failed());
    }

    #[test]
    fn interactive_add_asks_registration_and_optional_hook_per_successful_repository() {
        let store = RecordingStore {
            config: config(),
            registered: RefCell::new(Vec::new()),
            removed: RefCell::new(Vec::new()),
        };
        let git = FakeGit {
            cloned: RefCell::new(vec![]),
        };
        let hooks = FakeHooks::default();
        let mut interaction = QueueInteraction {
            answers: [Ok(true), Ok(false), Ok(true), Ok(true)].into(),
            inputs: [Ok("  make setup && printf 'x y'  ".to_owned())].into(),
            prompts: Vec::new(),
        };
        let references = vec![
            "github.com/org/one".to_owned(),
            "github.com/org/two".to_owned(),
        ];
        let outcome = clone_many_with_interaction(
            &store,
            &git,
            &hooks,
            &mut interaction,
            Path::new("config"),
            &references,
            &PathBuf::from("/tmp/lager-test-home"),
        )
        .unwrap();
        assert!(!outcome.cancelled);
        assert_eq!(git.cloned.borrow().len(), 2);
        assert_eq!(
            store.registered.borrow().as_slice(),
            [
                ("github.com/org/one".to_owned(), None),
                (
                    "github.com/org/two".to_owned(),
                    Some("  make setup && printf 'x y'  ".to_owned())
                )
            ]
        );
        assert_eq!(
            hooks.commands.borrow().as_slice(),
            ["  make setup && printf 'x y'  "]
        );
        assert_eq!(
            interaction.prompts,
            [
                ("Register github.com/org/one?".to_owned(), true),
                (
                    "Configure a post-clone hook for github.com/org/one?".to_owned(),
                    false
                ),
                ("Register github.com/org/two?".to_owned(), true),
                (
                    "Configure a post-clone hook for github.com/org/two?".to_owned(),
                    false
                ),
                (
                    "Post-clone command for github.com/org/two?".to_owned(),
                    false
                )
            ]
        );
    }

    #[test]
    fn interactive_add_stops_after_a_new_hook_reports_cancellation() {
        struct CancelledHook;
        impl HookRunner for CancelledHook {
            type Error = std::io::Error;

            fn is_cancelled(&self, error: &Self::Error) -> bool {
                error.kind() == std::io::ErrorKind::Interrupted
            }

            fn run_hook(&self, _: &str, _: &Path) -> Result<(), Self::Error> {
                Err(std::io::ErrorKind::Interrupted.into())
            }
        }
        let store = RecordingStore {
            config: config(),
            registered: RefCell::new(Vec::new()),
            removed: RefCell::new(Vec::new()),
        };
        let git = FakeGit {
            cloned: RefCell::new(vec![]),
        };
        let mut interaction = QueueInteraction {
            answers: [Ok(true), Ok(true)].into(),
            inputs: [Ok("exit 130".to_owned())].into(),
            prompts: Vec::new(),
        };
        let outcome = clone_many_with_interaction(
            &store,
            &git,
            &CancelledHook,
            &mut interaction,
            Path::new("config"),
            &[
                "github.com/org/one".to_owned(),
                "github.com/org/two".to_owned(),
            ],
            Path::new("/tmp/lager-test-home"),
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.outcomes.len(), 1);
        assert_eq!(git.cloned.borrow().len(), 1);
        assert_eq!(store.registered.borrow().len(), 1);
    }

    #[test]
    fn interactive_add_cancellation_stops_before_cloning_later_repositories() {
        let store = RecordingStore {
            config: config(),
            registered: RefCell::new(Vec::new()),
            removed: RefCell::new(Vec::new()),
        };
        let git = FakeGit {
            cloned: RefCell::new(vec![]),
        };
        let mut interaction = QueueInteraction {
            answers: [Err(InteractionError::Cancelled)].into(),
            inputs: VecDeque::new(),
            prompts: Vec::new(),
        };
        let outcome = clone_many_with_interaction(
            &store,
            &git,
            &FakeHooks::default(),
            &mut interaction,
            Path::new("config"),
            &[
                "github.com/org/one".to_owned(),
                "github.com/org/two".to_owned(),
            ],
            &PathBuf::from("/tmp/lager-test-home"),
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.outcomes.len(), 1);
        assert_eq!(git.cloned.borrow().len(), 1);
    }

    #[test]
    fn clone_batch_continues_after_independent_failure() {
        let store = FakeStore { config: config() };
        let git = FakeGit {
            cloned: RefCell::new(vec![]),
        };
        let hooks = FakeHooks::default();
        let home = PathBuf::from("/tmp/lager-test-home");
        let references = vec![
            "github.com/org/bad".to_owned(),
            "github.com/org/good".to_owned(),
        ];
        let outcome = clone_many(
            &store,
            &git,
            &hooks,
            Path::new("config"),
            &references,
            false,
            None,
            &home,
        )
        .unwrap();
        assert!(outcome.failed());
        assert_eq!(outcome.outcomes.len(), 2);
        assert_eq!(git.cloned.borrow().len(), 1);
    }

    #[test]
    fn ensure_reports_clone_start_before_git_and_success_afterward() {
        let mut config = config();
        config
            .repositories
            .push(crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/good".to_owned(),
                post_clone: None,
                exclude: vec![],
            });
        let store = FakeStore { config };
        let events = RefCell::new(Vec::new());
        let git = EventGit { events: &events };
        let mut reporter = RecordingEnsureReporter { events: &events };

        let outcome = ensure(
            &store,
            &git,
            &FakeHooks::default(),
            &NoProviders,
            Path::new("config"),
            false,
            Path::new("/tmp/lager-test-home"),
            &mut reporter,
        )
        .unwrap();

        assert!(!outcome.failed());
        assert_eq!(
            events.into_inner(),
            [
                "start:git@github.com:org/good.git:/tmp/lager-test-home/repos/org/good",
                "git:git@github.com:org/good.git",
                "success:git@github.com:org/good.git",
            ]
        );
    }

    #[test]
    fn ensure_keeps_declaration_order_and_runs_explicit_hook_after_clone() {
        let mut config = config();
        config
            .repositories
            .push(crate::domain::config::RepositoryDeclaration {
                url: "github.com/org/good".to_owned(),
                post_clone: Some("printf hook".to_owned()),
                exclude: vec![],
            });
        let store = FakeStore { config };
        let git = FakeGit {
            cloned: RefCell::new(vec![]),
        };
        let hooks = FakeHooks::default();
        let events = RefCell::new(Vec::new());
        let mut reporter = RecordingEnsureReporter { events: &events };
        let outcome = ensure(
            &store,
            &git,
            &hooks,
            &NoProviders,
            Path::new("config"),
            false,
            Path::new("/tmp/lager-test-home"),
            &mut reporter,
        )
        .unwrap();
        assert!(!outcome.failed());
        assert_eq!(hooks.commands.borrow().as_slice(), ["printf hook"]);
    }

    #[test]
    fn ordinary_error_text_cancelled_does_not_stop_clone_entry_points() {
        struct OrdinaryFailure;
        impl GitClient for OrdinaryFailure {
            type Error = String;
            fn clone_repository(
                &self,
                _: &str,
                _: &Path,
                _: &Path,
                _: &Path,
            ) -> Result<(), String> {
                Err("cancelled".to_owned())
            }
        }
        impl GitState for OrdinaryFailure {
            fn classify_destination(&self, _: &Path, _: &str) -> LocalState {
                LocalState::Missing
            }
        }
        impl HookRunner for OrdinaryFailure {
            type Error = String;
            fn run_hook(&self, _: &str, _: &Path) -> Result<(), String> {
                Err("operation cancelled by ordinary error text".to_owned())
            }
        }
        let store = FakeStore { config: config() };
        let references = ["github.com/org/one", "github.com/org/two"].map(str::to_owned);
        let parsed = references
            .iter()
            .map(|value| RepositoryRef::parse(value).unwrap())
            .collect::<Vec<_>>();
        let path = Path::new("config");
        let home = Path::new("/tmp/lager-test-home");
        let mut interaction = QueueInteraction {
            answers: VecDeque::new(),
            inputs: VecDeque::new(),
            prompts: vec![],
        };
        let outcomes = [
            clone_many(
                &store,
                &OrdinaryFailure,
                &OrdinaryFailure,
                path,
                &references,
                false,
                None,
                home,
            )
            .unwrap(),
            clone_references(
                &store,
                &OrdinaryFailure,
                &OrdinaryFailure,
                path,
                &parsed,
                false,
                None,
                home,
            )
            .unwrap(),
            clone_many_with_interaction(
                &store,
                &OrdinaryFailure,
                &OrdinaryFailure,
                &mut interaction,
                path,
                &references,
                home,
            )
            .unwrap(),
            clone_references_with_interaction(
                &store,
                &OrdinaryFailure,
                &OrdinaryFailure,
                &mut interaction,
                path,
                &parsed,
                home,
            )
            .unwrap(),
            clone_many(
                &store,
                &FakeGit {
                    cloned: RefCell::new(vec![]),
                },
                &OrdinaryFailure,
                path,
                &references,
                true,
                Some("hook"),
                home,
            )
            .unwrap(),
        ];
        for outcome in outcomes {
            assert!(!outcome.cancelled, "{outcome:?}");
            assert!(outcome.failed());
            assert_eq!(outcome.outcomes.len(), 2);
            assert!(
                outcome
                    .outcomes
                    .iter()
                    .all(|outcome| matches!(&outcome.status,
                OperationStatus::Failed(error) if error.contains("cancelled")))
            );
        }
        assert!(interaction.prompts.is_empty());
    }
}
