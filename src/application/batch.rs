use std::path::{Path, PathBuf};

use crate::application::inventory::{
    CheckoutState, InventoryFilesystem, InventoryRow, RowKey, configured_destination,
    wildcard_relative,
};
use crate::application::ports::{
    AtomicRegistrationStore, GitRemoval, Interaction, InteractionError, Mutation,
};
use crate::application::registry::RegistrationError;
use crate::domain::config::Config;
use crate::domain::repository::RepositoryRef;

/// A row mutation the inventory runs over targets frozen at invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    Register,
    Unregister,
}

impl BatchAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Register => "Register",
            Self::Unregister => "Unregister",
        }
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
        }
    }
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
        // Keep the configured clone URL text, as the CLI does, when it names the same identity.
        let reference = row
            .remote
            .clone()
            .filter(|remote| {
                RepositoryRef::parse(remote).is_ok_and(|parsed| Some(parsed.identity()) == identity)
            })
            .or_else(|| identity.clone());
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
        let changed = || {
            TargetResult::new(
                Outcome::Blocked,
                "",
                format!("stale: registration changed {}", self.since()),
            )
        };
        let done = |reason| TargetResult::new(Outcome::Noop, "", reason);
        self.results[index] = if let Some(reason) = stale {
            TargetResult::new(Outcome::Blocked, "", reason)
        } else if let Some(earlier) = self.shared_with(index) {
            self.results[earlier].clone()
        } else if let Some(result) = match store.load(config_path) {
            Err(error) => Some(TargetResult::new(Outcome::Failed, "", error.to_string())),
            Ok(live) => {
                let identity = target.identity.as_deref().unwrap_or_default();
                if Registration::of(identity, &live) != target.registration {
                    Some(changed())
                } else {
                    target.noop.as_ref().map(|_| {
                        done(match self.action {
                            BatchAction::Register => "already registered",
                            BatchAction::Unregister => "already unregistered",
                        })
                    })
                }
            }
        } {
            result
        } else {
            let references = [target.reference.clone().unwrap_or_default()];
            let written = match self.action {
                BatchAction::Register => crate::application::registry::register_batch(
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
                BatchAction::Unregister => {
                    crate::application::configuration::unregister(store, config_path, &references)
                }
            };
            match written {
                Ok(mutations) if mutations.contains(&Mutation::Changed) => {
                    TargetResult::new(Outcome::Succeeded, "updated", "")
                }
                Ok(_) => changed(),
                Err(error) => TargetResult::new(Outcome::Failed, "write failed", error),
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
                            BatchAction::Unregister => declared.clone(),
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
}
