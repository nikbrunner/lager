use std::path::{Path, PathBuf};

use crate::application::inventory::{
    CheckoutState, InventoryFilesystem, InventoryRow, RowKey, configured_destination,
    wildcard_relative,
};
use crate::application::ports::{
    AtomicRegistrationStore, ConfigStore, EnsureReporter, GitClient, GitRemoval, GitState,
    HookRunner, Interaction, InteractionError, Mutation,
};
use crate::application::registry::RegistrationError;
use crate::application::warehouse::{self, OperationStatus};
use crate::domain::config::Config;
use crate::domain::repository::RepositoryRef;
use crate::domain::state::LocalState;

/// A row mutation the inventory runs over targets frozen at invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    Register,
    Unregister,
    Clone,
    CloneRegister,
}

impl BatchAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Register => "Register",
            Self::Unregister => "Unregister",
            Self::Clone => "Clone only",
            Self::CloneRegister => "Clone + register",
        }
    }

    /// Clone actions run native Git and hooks; the others change configuration only.
    pub fn clones(self) -> bool {
        matches!(self, Self::Clone | Self::CloneRegister)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Available,
    /// Allowed, but the configuration already is in the requested state.
    Noop(&'static str),
    Unavailable(&'static str),
}

/// How a configuration declares one identity; it decides what registering or unregistering
/// that identity would change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registration {
    explicit: bool,
    /// Per wildcard covering the identity, in declaration order: whether it excludes it.
    exclusions: Vec<bool>,
}

impl Registration {
    fn of(identity: &str, config: &Config) -> Self {
        let mut registration = Self::default();
        for declaration in &config.repositories {
            let Ok(reference) = RepositoryRef::parse(&declaration.url) else {
                continue;
            };
            if !reference.is_wildcard() {
                registration.explicit |= reference.identity() == identity;
            } else if let Some(relative) = wildcard_relative(&reference, identity) {
                registration
                    .exclusions
                    .push(declaration.exclude.iter().any(|name| name == relative));
            }
        }
        registration
    }

    /// The registration label the table shows: explicit, wildcard, excluded or unregistered.
    pub fn label(&self) -> &'static str {
        if self.explicit {
            "explicit"
        } else if self.exclusions.iter().any(|excluded| !excluded) {
            "wildcard"
        } else if !self.exclusions.is_empty() {
            "excluded"
        } else {
            "unregistered"
        }
    }

    fn noop(&self, action: BatchAction) -> Option<&'static str> {
        let excluded = self.exclusions.iter().any(|excluded| *excluded);
        match action {
            BatchAction::Register
                if !excluded && (self.explicit || !self.exclusions.is_empty()) =>
            {
                Some("identity is already registered")
            }
            BatchAction::Unregister
                if !self.explicit && self.exclusions.iter().all(|excluded| *excluded) =>
            {
                Some("identity is not registered")
            }
            _ => None,
        }
    }
}

/// One concrete reference typed into Add, with its configured destination and the checkout
/// state observed there when the preview opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPreview {
    pub reference: String,
    pub identity: String,
    pub destination: PathBuf,
    pub checkout: CheckoutState,
    /// An unknown destination may still be observed: the scan is running or its probe there
    /// has not finished.
    pub pending: bool,
    pub registration: Registration,
    pub hook: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddChoice {
    Register,
    Clone,
    CloneRegister,
}

impl AddChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Register => "Register only",
            Self::Clone => "Clone only",
            Self::CloneRegister => "Clone + register",
        }
    }
}

/// Parses `input` as the CLI does and observes its configured destination. Only a destination
/// that does not exist is missing; a directory is cloned or a conflict only when the scan has
/// observed a checkout there, and otherwise stays unknown. Errors never contain the input.
pub fn add_preview(
    input: &str,
    config: &Config,
    root: &Path,
    rows: &[InventoryRow],
    scan_complete: bool,
    filesystem: &impl InventoryFilesystem,
) -> Result<AddPreview, String> {
    let parsed = RepositoryRef::parse(input).map_err(|error| error.to_string())?;
    // The parser keeps only a trailing `/*` as a pattern; any other `*` is no repository either.
    if parsed.is_wildcard() || input.contains('*') {
        return Err("wildcards are not accepted; Add takes one concrete repository".to_owned());
    }
    let identity = parsed.identity();
    let destination = configured_destination(config, &parsed, root);
    let checkout = match filesystem.destination_state(&destination) {
        CheckoutState::Unknown => rows
            .iter()
            .filter(|row| {
                row.observed_path.as_ref() == Some(&destination)
                    && row.checkout == CheckoutState::Cloned
            })
            .map(|row| match &row.key {
                RowKey::Checkout {
                    identity: found, ..
                }
                | RowKey::Declaration {
                    identity: found, ..
                } if *found == identity => CheckoutState::Cloned,
                _ => CheckoutState::Conflict,
            })
            .next()
            .unwrap_or(CheckoutState::Unknown),
        state => state,
    };
    let hook = config.repositories.iter().find_map(|declaration| {
        RepositoryRef::parse(&declaration.url)
            .ok()
            .filter(|reference| !reference.is_wildcard() && reference.identity() == identity)
            .and(declaration.post_clone.clone())
    });
    let pending = !scan_complete
        || rows
            .iter()
            .any(|row| row.observed_path.as_ref() == Some(&destination));
    Ok(AddPreview {
        reference: input.trim().to_owned(),
        pending,
        registration: Registration::of(&identity, config),
        identity,
        destination,
        checkout,
        hook,
    })
}

impl AddPreview {
    /// Cloning needs a destination observed as missing; a matching checkout makes it a no-op.
    pub fn eligibility(&self, choice: AddChoice) -> Eligibility {
        let registered = self.registration.noop(BatchAction::Register);
        let clone = match self.checkout {
            CheckoutState::Missing => Ok(false),
            CheckoutState::Cloned => Ok(true),
            CheckoutState::Conflict => Err("destination conflicts with repository"),
            CheckoutState::Unreadable => Err("destination is unreadable"),
            _ if self.pending => Err("checkout state is unknown until it is observed"),
            _ => Err("the destination exists and holds no observed checkout"),
        };
        match (choice, clone) {
            (AddChoice::Register, _) => {
                registered.map_or(Eligibility::Available, Eligibility::Noop)
            }
            (_, Err(reason)) => Eligibility::Unavailable(reason),
            (AddChoice::Clone, Ok(true)) => {
                Eligibility::Noop("a matching checkout is at the destination")
            }
            (AddChoice::CloneRegister, Ok(true)) if registered.is_some() => {
                Eligibility::Noop("checkout and registration are already in place")
            }
            _ => Eligibility::Available,
        }
    }

    /// The Register target, frozen like a row's: a checkout observed at the destination must
    /// still be there when the target runs.
    pub fn target(&self, name: String) -> BatchTarget {
        BatchTarget {
            key: RowKey::Declaration {
                identity: self.identity.clone(),
                destination: self.destination.clone(),
            },
            name,
            identity: Some(self.identity.clone()),
            path: Some(self.destination.clone()),
            found: self.checkout == CheckoutState::Cloned,
            reference: Some(self.reference.clone()),
            hidden: false,
            blocked: None,
            noop: self
                .registration
                .noop(BatchAction::Register)
                .map(str::to_owned),
            registration: self.registration.clone(),
            elsewhere: None,
        }
    }

    /// The target of a clone action, frozen with the destination state this preview observed:
    /// a destination that was missing must still be missing when its clone starts.
    pub fn clone_target(&self, action: BatchAction, name: String) -> BatchTarget {
        let choice = if action == BatchAction::CloneRegister {
            AddChoice::CloneRegister
        } else {
            AddChoice::Clone
        };
        let (blocked, noop) = match self.eligibility(choice) {
            Eligibility::Available => (None, None),
            Eligibility::Noop(reason) => (None, Some(reason.to_owned())),
            Eligibility::Unavailable(reason) => (Some(reason.to_owned()), None),
        };
        BatchTarget {
            blocked,
            noop,
            ..self.target(name)
        }
    }
}

/// The reference a row's identity is registered and cloned by: the configured or observed
/// clone URL text, as the CLI keeps it, when it names the same identity.
fn row_reference(row: &InventoryRow) -> Option<String> {
    let (RowKey::Checkout { identity, .. } | RowKey::Declaration { identity, .. }) = &row.key
    else {
        return None;
    };
    row.remote
        .clone()
        .filter(|remote| {
            RepositoryRef::parse(remote).is_ok_and(|parsed| parsed.identity() == *identity)
        })
        .or_else(|| Some(identity.clone()))
}

/// What cloning a row means: its identity at the configured destination, wherever a checkout
/// was found. Patterns and originless checkouts cannot be cloned.
pub fn clone_preview(
    row: &InventoryRow,
    config: &Config,
    root: &Path,
    rows: &[InventoryRow],
    scan_complete: bool,
    filesystem: &impl InventoryFilesystem,
) -> Result<AddPreview, &'static str> {
    let unsupported = "no supported origin observed";
    let reference = match &row.key {
        RowKey::Pattern(_) => return Err("patterns are inspect-only"),
        _ => row_reference(row).ok_or(unsupported)?,
    };
    add_preview(&reference, config, root, rows, scan_complete, filesystem).map_err(|_| unsupported)
}

/// Registration is identity-scoped: only an identity known from configuration or a supported
/// origin can be registered or unregistered.
pub fn eligibility(row: &InventoryRow, action: BatchAction, config: &Config) -> Eligibility {
    let identity = match &row.key {
        RowKey::Pattern(_) => return Eligibility::Unavailable("patterns are inspect-only"),
        RowKey::Path(_) => return Eligibility::Unavailable("no supported origin observed"),
        RowKey::Checkout { identity, .. } | RowKey::Declaration { identity, .. } => identity,
    };
    match Registration::of(identity, config).noop(action) {
        Some(reason) => Eligibility::Noop(reason),
        None => Eligibility::Available,
    }
}

/// One exact target, fixed when the batch starts; refreshes never change it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTarget {
    pub key: RowKey,
    pub name: String,
    pub identity: Option<String>,
    /// The found path, else the configured destination.
    pub path: Option<PathBuf>,
    /// A checkout was found at `path` at invocation; it must still be there, with the same
    /// identity, when the target runs.
    pub found: bool,
    /// The reference handed to the registry service.
    pub reference: Option<String>,
    /// Hidden by the filter when the batch started.
    pub hidden: bool,
    /// Why the target cannot run, decided at invocation.
    pub blocked: Option<String>,
    /// Why the configuration already is in the requested state at invocation.
    pub noop: Option<String>,
    /// How the configuration declared the identity at invocation; it must still hold when the
    /// target runs.
    pub registration: Registration,
    /// The checkout found at another path than the clone destination in `path`.
    pub elsewhere: Option<PathBuf>,
}

impl BatchTarget {
    pub fn freeze(
        row: &InventoryRow,
        name: String,
        hidden: bool,
        action: BatchAction,
        config: &Config,
    ) -> Self {
        let identity = match &row.key {
            RowKey::Checkout { identity, .. } | RowKey::Declaration { identity, .. } => {
                Some(identity.clone())
            }
            RowKey::Path(_) | RowKey::Pattern(_) => None,
        };
        let reference = row_reference(row);
        let eligibility = eligibility(row, action, config);
        Self {
            key: row.key.clone(),
            name,
            registration: Registration::of(identity.as_deref().unwrap_or_default(), config),
            identity,
            path: row
                .observed_path
                .clone()
                .or_else(|| row.configured_destination.clone()),
            reference,
            hidden,
            found: row.observed_path.is_some(),
            blocked: match eligibility {
                Eligibility::Unavailable(reason) => Some(reason.to_owned()),
                Eligibility::Available | Eligibility::Noop(_) => None,
            },
            noop: match eligibility {
                Eligibility::Noop(reason) => Some(reason.to_owned()),
                Eligibility::Available | Eligibility::Unavailable(_) => None,
            },
            elsewhere: None,
        }
    }

    /// Freezes a row for a clone action. The clone lands at the configured destination; a
    /// checkout found at another path stays where it is.
    #[allow(clippy::too_many_arguments)]
    pub fn freeze_clone(
        row: &InventoryRow,
        name: String,
        hidden: bool,
        action: BatchAction,
        config: &Config,
        root: &Path,
        rows: &[InventoryRow],
        scan_complete: bool,
        filesystem: &impl InventoryFilesystem,
    ) -> Self {
        match clone_preview(row, config, root, rows, scan_complete, filesystem) {
            Ok(preview) => Self {
                key: row.key.clone(),
                hidden,
                elsewhere: row
                    .observed_path
                    .clone()
                    .filter(|found| *found != preview.destination),
                ..preview.clone_target(action, name)
            },
            Err(reason) => Self {
                blocked: Some(reason.to_owned()),
                noop: None,
                ..Self::freeze(row, name, hidden, BatchAction::Register, config)
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Noop,
    Failed,
    Blocked,
    Cancelled,
    Unattempted,
}

/// Disk and configuration results are reported separately; an empty cell means unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetResult {
    pub outcome: Outcome,
    pub disk: String,
    pub config: String,
    pub reason: String,
}

impl TargetResult {
    fn new(outcome: Outcome, config: &str, reason: impl Into<String>) -> Self {
        Self {
            outcome,
            disk: String::new(),
            config: config.to_owned(),
            reason: reason.into(),
        }
    }
}

/// A sequential batch: one target per step, each with its own configuration write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub action: BatchAction,
    pub targets: Vec<BatchTarget>,
    pub results: Vec<TargetResult>,
    /// The user accepted a preview of these targets.
    pub previewed: bool,
}

impl Batch {
    pub fn new(action: BatchAction, targets: Vec<BatchTarget>) -> Self {
        let results = targets
            .iter()
            .map(|_| TargetResult::new(Outcome::Unattempted, "", ""))
            .collect();
        Self {
            action,
            targets,
            results,
            previewed: false,
        }
    }

    fn since(&self) -> &'static str {
        if self.previewed {
            "after preview"
        } else {
            "since the action started"
        }
    }

    /// The index of the next target to run.
    pub fn active(&self) -> Option<usize> {
        self.results
            .iter()
            .position(|result| result.outcome == Outcome::Unattempted)
    }

    pub fn processed(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.outcome != Outcome::Unattempted)
            .count()
    }

    pub fn finished(&self) -> bool {
        self.active().is_none()
    }

    /// The earlier target whose registration change this target shares, if any. Blocked and
    /// stale targets share nothing.
    pub fn shared_with(&self, index: usize) -> Option<usize> {
        let blocked = |index: usize| {
            self.targets[index].blocked.is_some() || self.results[index].outcome == Outcome::Blocked
        };
        if blocked(index) {
            return None;
        }
        let identity = self.targets[index].identity.as_ref()?;
        (0..index).find(|earlier| {
            !blocked(*earlier) && self.targets[*earlier].identity.as_ref() == Some(identity)
        })
    }

    /// Why a target no longer matches what was frozen at invocation: a checkout found then must
    /// still be at its exact path with the same origin identity.
    fn stale(&self, target: &BatchTarget, git: &impl GitRemoval) -> Option<String> {
        let path = target.path.as_ref().filter(|_| target.found)?;
        match git.origin_identity(path) {
            Ok(identity) if Some(&identity) == target.identity.as_ref() => None,
            Ok(_) => Some(format!("stale: origin changed {}", self.since())),
            Err(_) => Some(format!(
                "stale: checkout is gone or unreadable {}",
                self.since()
            )),
        }
    }

    /// Runs the next target and returns its index. A target that shares an earlier target's
    /// identity takes over that result instead of writing again. Otherwise the live
    /// configuration must still declare the identity as it did at invocation, and a write that
    /// then finds nothing to change is stale too: only what was frozen runs.
    pub fn step<S: AtomicRegistrationStore>(
        &mut self,
        store: &S,
        git: &impl GitRemoval,
        config_path: &Path,
    ) -> Option<usize> {
        let index = self.active()?;
        let target = &self.targets[index];
        let stale = target.blocked.clone().or_else(|| self.stale(target, git));
        self.results[index] = if let Some(reason) = stale {
            TargetResult::new(Outcome::Blocked, "", reason)
        } else if let Some(earlier) = self.shared_with(index) {
            self.results[earlier].clone()
        } else {
            self.write_registration(index, store, config_path)
        };
        Some(index)
    }

    /// The registration write of one target, after the live configuration is compared with
    /// what was frozen.
    fn write_registration<S: AtomicRegistrationStore>(
        &self,
        index: usize,
        store: &S,
        config_path: &Path,
    ) -> TargetResult {
        let target = &self.targets[index];
        let changed = || {
            TargetResult::new(
                Outcome::Blocked,
                "",
                format!("stale: registration changed {}", self.since()),
            )
        };
        let live = match store.load(config_path) {
            Ok(live) => live,
            Err(error) => return TargetResult::new(Outcome::Failed, "", error.to_string()),
        };
        let identity = target.identity.as_deref().unwrap_or_default();
        if Registration::of(identity, &live) != target.registration {
            return changed();
        }
        // A clone target's own no-op covers the checkout too; its registration decides here.
        let noop = if self.action.clones() {
            target.registration.noop(BatchAction::Register).is_some()
        } else {
            target.noop.is_some()
        };
        if noop {
            return TargetResult::new(
                Outcome::Noop,
                "",
                match self.action {
                    BatchAction::Unregister => "already unregistered",
                    _ => "already registered",
                },
            );
        }
        let references = [target.reference.clone().unwrap_or_default()];
        let written = match self.action {
            BatchAction::Unregister => {
                crate::application::configuration::unregister(store, config_path, &references)
            }
            _ => crate::application::registry::register_batch(
                store,
                config_path,
                &references,
                None,
                false,
                &mut NoPrompts,
            )
            .map_err(|error| match error {
                RegistrationError::Cancelled => "cancelled".to_owned(),
                RegistrationError::Interaction(message) | RegistrationError::Store(message) => {
                    message
                }
            }),
        };
        match written {
            Ok(mutations) if mutations.contains(&Mutation::Changed) => {
                TargetResult::new(Outcome::Succeeded, "updated", "")
            }
            Ok(_) => changed(),
            Err(error) => TargetResult::new(Outcome::Failed, "write failed", error),
        }
    }

    /// The next target of a clone action would run native Git: it was frozen as missing.
    pub fn needs_terminal(&self) -> bool {
        self.action.clones()
            && self.active().is_some_and(|index| {
                let target = &self.targets[index];
                target.blocked.is_none() && !target.found && self.shared_with(index).is_none()
            })
    }

    /// Why a clone would no longer do what was frozen: the configured layout must still lead
    /// to the frozen destination, and a destination frozen as missing must still be missing.
    fn stale_clone<S: ConfigStore>(
        &self,
        target: &BatchTarget,
        store: &S,
        git: &impl GitState,
        config_path: &Path,
        home: &Path,
    ) -> Option<TargetResult> {
        let live = match store.load(config_path) {
            Ok(live) => live,
            Err(error) => return Some(TargetResult::new(Outcome::Failed, "", error.to_string())),
        };
        let reference = target.reference.as_deref().unwrap_or_default();
        let path = target.path.as_deref()?;
        let destination = RepositoryRef::parse(reference)
            .ok()
            .zip(live.resolve_root(home).ok())
            .map(|(parsed, root)| configured_destination(&live, &parsed, &root));
        let reason = if destination.as_deref() != Some(path) {
            "configured destination changed"
        } else if !target.found && git.classify_destination(path, reference) != LocalState::Missing
        {
            "destination is no longer missing"
        } else {
            return None;
        };
        Some(TargetResult::new(
            Outcome::Blocked,
            "",
            format!("stale: {reason} {}", self.since()),
        ))
    }

    /// Runs the next target of a clone action through the service `lager add` uses and returns
    /// its index. Git and a configured hook run with the caller's streams; `reporter` hears of
    /// a clone just before Git starts. Clone + register writes its registration only after a
    /// successful or no-op clone, and a failed write leaves the checkout in place. A cancelled
    /// child gives `Outcome::Cancelled`.
    pub fn step_clone<S, G, H>(
        &mut self,
        store: &S,
        git: &G,
        hooks: &H,
        config_path: &Path,
        home: &Path,
        reporter: &mut dyn EnsureReporter,
    ) -> Option<usize>
    where
        S: AtomicRegistrationStore,
        G: GitClient + GitState + GitRemoval,
        H: HookRunner,
    {
        let index = self.active()?;
        let target = &self.targets[index];
        let stale = target.blocked.clone().or_else(|| self.stale(target, git));
        self.results[index] = if let Some(reason) = stale {
            TargetResult::new(Outcome::Blocked, "", reason)
        } else if let Some(earlier) = self.shared_with(index) {
            self.results[earlier].clone()
        } else if let Some(result) = self.stale_clone(target, store, git, config_path, home) {
            result
        } else {
            let reference = target.reference.clone().unwrap_or_default();
            // A checkout found at the destination is a no-op and never reaches Git, so Git
            // only runs for targets `needs_terminal` announced.
            let (status, cancelled) = if target.found {
                (OperationStatus::Noop, false)
            } else {
                match warehouse::clone_many_with_reporter(
                    store,
                    git,
                    hooks,
                    config_path,
                    std::slice::from_ref(&reference),
                    false,
                    None,
                    home,
                    reporter,
                ) {
                    Ok(mut batch) => (
                        batch
                            .outcomes
                            .pop()
                            .map_or(OperationStatus::Noop, |outcome| outcome.status),
                        batch.cancelled,
                    ),
                    Err(error) => (OperationStatus::Failed(error), false),
                }
            };
            // A hook that fails after its clone leaves the checkout in place.
            let cloned = match &status {
                OperationStatus::Cloned => true,
                OperationStatus::Failed(_) => {
                    !target.found
                        && target.path.as_deref().is_some_and(|path| {
                            git.classify_destination(path, &reference) == LocalState::Cloned
                        })
                }
                _ => false,
            };
            let disk = if cloned { "cloned" } else { "" }.to_owned();
            match status {
                OperationStatus::Failed(reason) => TargetResult {
                    outcome: if cancelled {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed
                    },
                    disk,
                    config: String::new(),
                    reason,
                },
                _ if self.action == BatchAction::CloneRegister => {
                    let written = self.write_registration(index, store, config_path);
                    TargetResult {
                        outcome: match written.outcome {
                            Outcome::Noop if cloned => Outcome::Succeeded,
                            outcome => outcome,
                        },
                        reason: match written.outcome {
                            Outcome::Noop if !cloned => "already cloned and registered".to_owned(),
                            _ => written.reason,
                        },
                        disk,
                        config: written.config,
                    }
                }
                _ if cloned => TargetResult {
                    disk,
                    ..TargetResult::new(Outcome::Succeeded, "", "")
                },
                _ => TargetResult::new(Outcome::Noop, "", "already cloned"),
            }
        };
        Some(index)
    }
}

/// Batches never ask; registration runs without a post-clone hook choice.
struct NoPrompts;

impl Interaction for NoPrompts {
    fn confirm(&mut self, _: &str, _: bool) -> Result<bool, InteractionError> {
        Err(InteractionError::Cancelled)
    }

    fn input(&mut self, _: &str, _: &str, _: Option<&str>) -> Result<String, InteractionError> {
        Err(InteractionError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;

    #[test]
    fn add_rejects_every_wildcard_without_quoting_it() {
        struct Absent;
        impl crate::application::inventory::InventoryFilesystem for Absent {
            fn destination_state(&self, _: &Path) -> crate::application::inventory::CheckoutState {
                crate::application::inventory::CheckoutState::Missing
            }
            fn config_revision(&self, _: &Path) -> String {
                String::new()
            }
        }
        let config = Config {
            root: "repos".into(),
            providers: Default::default(),
            repositories: Vec::new(),
        };
        for input in [
            "org/*",
            "org/*/repo",
            "github.com/org/*/repo",
            "git@github.com:*/x.git",
        ] {
            let error = super::add_preview(input, &config, Path::new("/r"), &[], true, &Absent)
                .unwrap_err();
            assert_eq!(
                error, "wildcards are not accepted; Add takes one concrete repository",
                "{input}"
            );
        }
        assert!(
            super::add_preview("org/repo", &config, Path::new("/r"), &[], true, &Absent).is_ok()
        );
    }

    use super::{Batch, BatchAction, BatchTarget, Outcome, Registration};
    use crate::application::inventory::RowKey;
    use crate::application::ports::GitRemoval;
    use crate::application::ports::{
        AtomicRegistrationStore, ConfigStore, Mutation, RegistrationRequest,
    };
    use crate::domain::config::{Config, RepositoryDeclaration};

    /// Records every written reference; `1` lists the identities the live config declares, and
    /// `2` makes every write find nothing to change.
    #[derive(Default)]
    struct CountingStore(RefCell<Vec<String>>, Vec<&'static str>, bool);

    impl ConfigStore for CountingStore {
        type Error = String;
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn load(&self, _: &Path) -> Result<Config, String> {
            Ok(Config {
                root: "repos".into(),
                providers: Default::default(),
                repositories: self
                    .1
                    .iter()
                    .map(|url| RepositoryDeclaration {
                        url: (*url).to_owned(),
                        post_clone: None,
                        exclude: Vec::new(),
                    })
                    .collect(),
            })
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
        fn remove(&self, _: &Path, references: &[String]) -> Result<Vec<Mutation>, String> {
            self.0.borrow_mut().extend(references.iter().cloned());
            Ok(vec![self.mutation(); references.len()])
        }
    }

    impl AtomicRegistrationStore for CountingStore {
        fn register_batch(
            &self,
            _: &Path,
            requests: &[RegistrationRequest],
        ) -> Result<Vec<Mutation>, String> {
            self.0
                .borrow_mut()
                .extend(requests.iter().map(|request| request.reference.clone()));
            Ok(vec![self.mutation(); requests.len()])
        }
    }

    impl CountingStore {
        fn mutation(&self) -> Mutation {
            if self.2 {
                Mutation::Noop
            } else {
                Mutation::Changed
            }
        }
    }

    /// Every checkout's origin resolves to `0`, or the path is gone when it is `None`.
    struct Origins(Option<&'static str>);

    impl GitRemoval for Origins {
        fn inspect_removal(&self, _: &Path, _: &str) -> Result<Vec<String>, String> {
            unreachable!()
        }
        fn origin_identity(&self, _: &Path) -> Result<String, String> {
            self.0
                .map(str::to_owned)
                .ok_or_else(|| "no such directory".to_owned())
        }
    }

    fn target(name: &str, identity: Option<&str>, blocked: Option<&str>) -> BatchTarget {
        BatchTarget {
            key: RowKey::Path(name.into()),
            name: name.to_owned(),
            identity: identity.map(str::to_owned),
            path: None,
            found: false,
            reference: identity.map(str::to_owned),
            hidden: false,
            blocked: blocked.map(str::to_owned),
            noop: None,
            registration: Registration::default(),
            elsewhere: None,
        }
    }

    #[test]
    fn a_registration_that_changed_since_freeze_is_stale_either_way_and_writes_nothing() {
        let (alpha, beta) = ("github.com/org/alpha", "github.com/org/beta");
        let declared = Registration {
            explicit: true,
            exclusions: Vec::new(),
        };
        let since = ["since the action started", "after preview"];
        for (action, live, frozen, noop, previewed) in [
            // Shown as a no-op, registered since.
            (
                BatchAction::Unregister,
                vec![alpha, beta],
                Registration::default(),
                Some("identity is not registered"),
                true,
            ),
            (
                BatchAction::Register,
                vec![],
                declared.clone(),
                Some("identity is already registered"),
                false,
            ),
            // Shown as a change, applied since.
            (
                BatchAction::Register,
                vec![alpha],
                Registration::default(),
                None,
                true,
            ),
            (
                BatchAction::Unregister,
                vec![beta],
                declared.clone(),
                None,
                false,
            ),
        ] {
            let store = CountingStore(RefCell::default(), live, false);
            let mut batch = Batch::new(
                action,
                vec![
                    BatchTarget {
                        noop: noop.map(str::to_owned),
                        registration: frozen,
                        ..target("alpha", Some(alpha), None)
                    },
                    BatchTarget {
                        registration: match action {
                            BatchAction::Register => Registration::default(),
                            _ => declared.clone(),
                        },
                        ..target("beta", Some(beta), None)
                    },
                ],
            );
            batch.previewed = previewed;
            while batch
                .step(&store, &Origins(None), Path::new("config"))
                .is_some()
            {}
            assert_eq!(batch.results[0].outcome, Outcome::Blocked);
            assert_eq!(
                batch.results[0].reason,
                format!("stale: registration changed {}", since[previewed as usize])
            );
            assert_eq!(batch.results[1].outcome, Outcome::Succeeded);
            assert_eq!(store.0.borrow().as_slice(), [beta]);
        }
    }

    #[test]
    fn a_write_that_finds_nothing_to_change_is_stale_not_a_no_op() {
        for action in [BatchAction::Register, BatchAction::Unregister] {
            let store = CountingStore(RefCell::default(), Vec::new(), true);
            let mut batch = Batch::new(
                action,
                vec![target("alpha", Some("github.com/org/alpha"), None)],
            );
            batch.step(&store, &Origins(None), Path::new("config"));
            assert_eq!(batch.results[0].outcome, Outcome::Blocked);
            assert_eq!(
                batch.results[0].reason,
                "stale: registration changed since the action started"
            );
        }
    }

    #[test]
    fn a_no_op_that_still_holds_reports_no_op_without_reaching_the_store() {
        let store = CountingStore::default();
        let mut batch = Batch::new(
            BatchAction::Unregister,
            vec![BatchTarget {
                noop: Some("identity is not registered".to_owned()),
                ..target("alpha", Some("github.com/org/alpha"), None)
            }],
        );
        batch.step(&store, &Origins(None), Path::new("config"));
        assert_eq!(batch.results[0].outcome, Outcome::Noop);
        assert_eq!(batch.results[0].reason, "already unregistered");
        assert!(store.0.borrow().is_empty());
    }

    #[test]
    fn a_found_checkout_whose_origin_changed_or_vanished_is_stale_and_writes_nothing() {
        for (git, reason) in [
            (
                Origins(Some("github.com/org/other")),
                "stale: origin changed since the action started",
            ),
            (
                Origins(None),
                "stale: checkout is gone or unreadable since the action started",
            ),
        ] {
            let store = CountingStore::default();
            let found = |name: &str| BatchTarget {
                path: Some(format!("/repos/{name}").into()),
                found: true,
                ..target(name, Some("github.com/org/alpha"), None)
            };
            let mut batch = Batch::new(
                BatchAction::Register,
                vec![
                    found("alpha"),
                    target("alpha", Some("github.com/org/alpha"), None),
                ],
            );
            while batch.step(&store, &git, Path::new("config")).is_some() {}
            assert_eq!(batch.results[0].outcome, Outcome::Blocked);
            assert_eq!(batch.results[0].reason, reason);
            // A stale target shares nothing, so the independent target of its identity still writes.
            assert_eq!(batch.results[1].outcome, Outcome::Succeeded);
            assert_eq!(batch.shared_with(1), None);
            assert_eq!(store.0.borrow().as_slice(), ["github.com/org/alpha"]);
        }
    }

    #[test]
    fn a_checkout_awaiting_its_reprobe_keeps_its_identity_scoped_eligibility() {
        use super::{Eligibility, eligibility};
        use crate::application::inventory::{
            CheckoutState, InventoryRow, ObservationField, RegistrationState,
        };
        let row = InventoryRow {
            key: RowKey::Checkout {
                identity: "github.com/org/beta".into(),
                path: "/repos/org/beta".into(),
            },
            repository: String::new(),
            path: String::new(),
            registration: RegistrationState::Unregistered,
            checkout: CheckoutState::Unknown,
            origin: String::new(),
            remote: None,
            branch: ObservationField::Stale("main".into()),
            changes: ObservationField::Stale("known clean".into()),
            configured_destination: None,
            observed_path: Some("/repos/org/beta".into()),
            declaration: None,
            warnings: Vec::new(),
            markable: true,
        };
        let config = CountingStore::default().load(Path::new("config")).unwrap();
        assert_eq!(
            eligibility(&row, BatchAction::Register, &config),
            Eligibility::Available
        );
        assert_eq!(
            eligibility(&row, BatchAction::Unregister, &config),
            Eligibility::Noop("identity is not registered")
        );
        let pattern = InventoryRow {
            key: RowKey::Pattern("github.com/org/*".into()),
            ..row.clone()
        };
        assert_eq!(
            eligibility(&pattern, BatchAction::Register, &config),
            Eligibility::Unavailable("patterns are inspect-only")
        );
        let path = InventoryRow {
            key: RowKey::Path("/repos/scratch".into()),
            ..row
        };
        assert_eq!(
            eligibility(&path, BatchAction::Unregister, &config),
            Eligibility::Unavailable("no supported origin observed")
        );
    }

    #[test]
    fn one_identity_is_written_once_and_blocked_targets_never_reach_the_store() {
        let store = CountingStore::default();
        let mut batch = Batch::new(
            BatchAction::Register,
            vec![
                target("scratch", None, Some("no supported origin observed")),
                target("alpha", Some("github.com/org/alpha"), None),
                target("alpha (copy)", Some("github.com/org/alpha"), None),
            ],
        );
        while batch
            .step(&store, &Origins(None), Path::new("config"))
            .is_some()
        {}
        assert_eq!(store.0.borrow().as_slice(), ["github.com/org/alpha"]);
        let outcomes = batch
            .results
            .iter()
            .map(|result| {
                (
                    result.outcome,
                    result.config.as_str(),
                    result.reason.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes,
            [
                (Outcome::Blocked, "", "no supported origin observed"),
                (Outcome::Succeeded, "updated", ""),
                (Outcome::Succeeded, "updated", ""),
            ]
        );
        assert_eq!(batch.shared_with(2), Some(1));
        assert_eq!(batch.shared_with(1), None);
        assert!(batch.finished());
    }

    /// Git whose destination is missing until a clone succeeds; `0` counts the clones.
    #[derive(Default)]
    struct Cloner(std::cell::Cell<usize>);

    impl crate::application::ports::GitClient for Cloner {
        type Error = String;
        fn clone_repository(&self, _: &str, _: &Path, _: &Path, _: &Path) -> Result<(), String> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    impl crate::application::ports::GitState for Cloner {
        fn classify_destination(&self, _: &Path, _: &str) -> crate::domain::state::LocalState {
            if self.0.get() == 0 {
                crate::domain::state::LocalState::Missing
            } else {
                crate::domain::state::LocalState::Cloned
            }
        }
    }

    impl GitRemoval for Cloner {
        fn inspect_removal(&self, _: &Path, _: &str) -> Result<Vec<String>, String> {
            unreachable!()
        }
        fn origin_identity(&self, _: &Path) -> Result<String, String> {
            Ok("github.com/org/alpha".to_owned())
        }
    }

    struct NoHooks;

    impl crate::application::ports::HookRunner for NoHooks {
        type Error = String;
        fn run_hook(&self, _: &str, _: &Path) -> Result<(), String> {
            unreachable!()
        }
    }

    struct Silent;

    impl crate::application::ports::EnsureReporter for Silent {
        fn report(&mut self, _: crate::application::ports::EnsureEvent<'_>) {}
    }

    /// Loads like `CountingStore` and refuses every write.
    struct ReadOnlyStore;

    impl ConfigStore for ReadOnlyStore {
        type Error = String;
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn load(&self, path: &Path) -> Result<Config, String> {
            CountingStore::default().load(path)
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
            unreachable!()
        }
    }

    impl AtomicRegistrationStore for ReadOnlyStore {
        fn register_batch(
            &self,
            _: &Path,
            _: &[RegistrationRequest],
        ) -> Result<Vec<Mutation>, String> {
            Err("config is read-only".to_owned())
        }
    }

    fn clone_target(path: &str) -> BatchTarget {
        BatchTarget {
            path: Some(path.into()),
            ..target("alpha", Some("github.com/org/alpha"), None)
        }
    }

    #[test]
    fn a_failed_registration_write_keeps_the_clone_and_reports_both_cells() {
        let git = Cloner::default();
        let mut batch = Batch::new(
            BatchAction::CloneRegister,
            vec![clone_target("/h/repos/org/alpha")],
        );
        assert!(batch.needs_terminal());
        batch.step_clone(
            &ReadOnlyStore,
            &git,
            &NoHooks,
            Path::new("config"),
            Path::new("/h"),
            &mut Silent,
        );
        let result = &batch.results[0];
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(
            (result.disk.as_str(), result.config.as_str()),
            ("cloned", "write failed")
        );
        assert_eq!(result.reason, "config is read-only");
        assert_eq!(git.0.get(), 1);
    }

    #[test]
    fn a_checkout_found_at_the_destination_never_reaches_git_even_if_it_vanishes() {
        // The origin re-check passes, then the destination classifies as missing.
        let git = Cloner::default();
        let mut batch = Batch::new(
            BatchAction::Clone,
            vec![BatchTarget {
                found: true,
                ..clone_target("/h/repos/org/alpha")
            }],
        );
        assert!(!batch.needs_terminal());
        batch.step_clone(
            &CountingStore::default(),
            &git,
            &NoHooks,
            Path::new("config"),
            Path::new("/h"),
            &mut Silent,
        );
        assert_eq!(batch.results[0].outcome, Outcome::Noop);
        assert_eq!(batch.results[0].reason, "already cloned");
        assert_eq!(git.0.get(), 0);
    }

    #[test]
    fn a_clone_runs_only_at_the_frozen_destination_and_only_while_it_is_missing() {
        for (path, clones_before, reason) in [
            (
                "/h/elsewhere/alpha",
                0,
                "stale: configured destination changed since the action started",
            ),
            (
                "/h/repos/org/alpha",
                1,
                "stale: destination is no longer missing since the action started",
            ),
        ] {
            let git = Cloner(std::cell::Cell::new(clones_before));
            let store = CountingStore::default();
            let mut batch = Batch::new(BatchAction::Clone, vec![clone_target(path)]);
            batch.step_clone(
                &store,
                &git,
                &NoHooks,
                Path::new("config"),
                Path::new("/h"),
                &mut Silent,
            );
            assert_eq!(batch.results[0].outcome, Outcome::Blocked, "{path}");
            assert_eq!(batch.results[0].reason, reason);
            assert_eq!(git.0.get(), clones_before);
            assert!(store.0.borrow().is_empty());
        }
    }

    #[test]
    fn a_row_clones_only_at_its_configured_destination_and_only_when_that_is_safe() {
        use crate::application::inventory::{
            CheckoutState, InventoryFilesystem, InventoryRow, ObservationField, RegistrationState,
        };
        /// `/r/org/gone` is unreadable, `/r/org/alpha` is absent, any other path is a directory.
        struct Disk;
        impl InventoryFilesystem for Disk {
            fn destination_state(&self, path: &Path) -> CheckoutState {
                match path.to_str() {
                    Some("/r/org/alpha") => CheckoutState::Missing,
                    Some("/r/org/gone") => CheckoutState::Unreadable,
                    _ => CheckoutState::Unknown,
                }
            }
            fn config_revision(&self, _: &Path) -> String {
                String::new()
            }
        }
        let row = |key: RowKey, observed: Option<&str>| InventoryRow {
            key,
            repository: String::new(),
            path: String::new(),
            registration: RegistrationState::Unregistered,
            checkout: if observed.is_some() {
                CheckoutState::Cloned
            } else {
                CheckoutState::Missing
            },
            origin: String::new(),
            remote: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: observed.map(Into::into),
            declaration: None,
            warnings: Vec::new(),
            markable: true,
        };
        let declared = |name: &str| RowKey::Declaration {
            identity: format!("github.com/org/{name}"),
            destination: format!("/r/org/{name}").into(),
        };
        let checkout = |name: &str, path: &str| RowKey::Checkout {
            identity: format!("github.com/org/{name}"),
            path: path.into(),
        };
        let rows = [
            row(RowKey::Pattern("github.com/org/*".into()), None),
            row(RowKey::Path("/r/scratch".into()), Some("/r/scratch")),
            row(declared("alpha"), None),
            row(declared("gone"), None),
            row(declared("taken"), None),
            row(checkout("other", "/r/org/taken"), Some("/r/org/taken")),
            row(checkout("alpha", "/r/misc/one"), Some("/r/misc/one")),
            row(checkout("beta", "/r/org/beta"), Some("/r/org/beta")),
        ];
        let config = CountingStore::default().load(Path::new("config")).unwrap();
        // Per row: why it is blocked, why it is a no-op, found at its path, found elsewhere.
        type Frozen = (
            Option<&'static str>,
            Option<&'static str>,
            bool,
            Option<&'static str>,
        );
        let expected: [Frozen; 8] = [
            (Some("patterns are inspect-only"), None, false, None),
            (Some("no supported origin observed"), None, true, None),
            (None, None, false, None),
            (Some("destination is unreadable"), None, false, None),
            (
                Some("destination conflicts with repository"),
                None,
                false,
                None,
            ),
            (
                Some("the destination exists and holds no observed checkout"),
                None,
                false,
                Some("/r/org/taken"),
            ),
            (None, None, false, Some("/r/misc/one")),
            (
                None,
                Some("a matching checkout is at the destination"),
                true,
                None,
            ),
        ];
        for (row, (blocked, noop, found, elsewhere)) in rows.iter().zip(expected) {
            let target = BatchTarget::freeze_clone(
                row,
                String::new(),
                false,
                BatchAction::Clone,
                &config,
                Path::new("/r"),
                &rows,
                true,
                &Disk,
            );
            assert_eq!(target.key, row.key);
            assert_eq!(target.blocked.as_deref(), blocked, "{:?}", row.key);
            assert_eq!(target.noop.as_deref(), noop, "{:?}", row.key);
            assert_eq!(target.found, found, "{:?}", row.key);
            assert_eq!(target.elsewhere.as_deref(), elsewhere.map(Path::new));
        }
        let misplaced = BatchTarget::freeze_clone(
            &rows[6],
            String::new(),
            false,
            BatchAction::Clone,
            &config,
            Path::new("/r"),
            &rows,
            true,
            &Disk,
        );
        assert_eq!(misplaced.path.as_deref(), Some(Path::new("/r/org/alpha")));
    }
}
