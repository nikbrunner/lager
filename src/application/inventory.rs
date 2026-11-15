use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::application::ports::{ProviderCatalog, ProviderFailure, RemoteState, Visibility};
use crate::domain::config::{Config, RepositoryDeclaration};
use crate::domain::repository::{RepositoryRef, RepositorySummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryReport {
    pub rows: Vec<InventoryRow>,
    pub diagnostics: Vec<ScanDiagnostic>,
    pub config_revision: String,
    pub observation_generation: u64,
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRow {
    pub key: RowKey,
    pub repository: String,
    pub path: String,
    pub registration: RegistrationState,
    pub checkout: CheckoutState,
    pub origin: String,
    /// The original origin URL, when the row has one; `origin` is display text.
    pub remote: Option<String>,
    /// What the origin probe found, for a row with an observed checkout.
    pub origin_fact: Option<OriginFact>,
    pub branch: ObservationField,
    pub changes: ObservationField,
    pub configured_destination: Option<PathBuf>,
    pub observed_path: Option<PathBuf>,
    pub declaration: Option<String>,
    pub warnings: Vec<String>,
    pub markable: bool,
    /// Whether the provider reports the repository as archived; `None` until a provider says.
    pub archived: Option<bool>,
    /// What the provider holds at the row's repository; `None` while nothing tells.
    pub remote_status: Option<RemoteStatus>,
}

/// A repository's state on its provider, as a catalog or a per-repository lookup reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStatus {
    Public,
    Private,
    /// Wins over the visibility.
    Archived,
    /// A lookup found no repository there. A catalog never tells, since it lists only what
    /// the account sees.
    Missing,
    /// A lookup is running.
    Pending,
}

impl RemoteStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Archived => "archived",
            Self::Missing => "missing",
            Self::Pending => "checking",
        }
    }

    fn of(archived: bool, visibility: Option<Visibility>) -> Option<Self> {
        match (archived, visibility) {
            (true, _) => Some(Self::Archived),
            (false, Some(Visibility::Public)) => Some(Self::Public),
            (false, Some(Visibility::Private | Visibility::Internal)) => Some(Self::Private),
            (false, None) => None,
        }
    }

    /// The outcome of a per-repository lookup; an error leaves the state unknown.
    pub fn looked_up(state: Result<Option<RemoteState>, String>) -> Option<Self> {
        match state {
            Ok(Some(state)) => Self::of(state.archived, Some(state.visibility)),
            Ok(None) => Some(Self::Missing),
            Err(_) => None,
        }
    }
}

/// A row key deliberately preserves the observed platform path. Display strings are lossy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowKey {
    Declaration {
        identity: String,
        destination: PathBuf,
    },
    Checkout {
        identity: String,
        path: PathBuf,
    },
    Path(PathBuf),
    Pattern(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationState {
    Explicit,
    Wildcard { pattern: String },
    Excluded { pattern: String },
    Unregistered,
    Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutState {
    Cloned,
    Missing,
    Pattern,
    Unknown,
    Conflict,
    Unreadable,
    /// A provider member with no local checkout and nothing at its configured destination.
    RemoteOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationField {
    Pending,
    Known(String),
    Clean,
    Dirty(String),
    Error(String),
    Stale(String),
    NotApplicable,
}

/// The raw origin is retained separately from the normalized identity: only an observed absence
/// is rendered as `No origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginFact {
    Absent,
    Supported { original: String, identity: String },
    Unsupported(String),
    Error(String),
}

impl OriginFact {
    pub fn display(&self) -> String {
        match self {
            Self::Absent => "No origin".to_owned(),
            Self::Supported { original, .. } | Self::Unsupported(original) => original.clone(),
            Self::Error(error) => format!("origin error: {error}"),
        }
    }

    fn original(&self) -> Option<String> {
        match self {
            Self::Supported { original, .. } | Self::Unsupported(original) => {
                Some(original.clone())
            }
            Self::Absent | Self::Error(_) => None,
        }
    }

    fn warning(&self) -> Option<String> {
        match self {
            Self::Unsupported(_) => {
                Some("Unsupported origin; identity is the exact path".to_owned())
            }
            Self::Error(error) => Some(format!("Could not inspect origin: {error}")),
            Self::Absent | Self::Supported { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCheckout {
    pub path: PathBuf,
    pub identity: Option<String>,
    pub origin: OriginFact,
    pub branch: ObservationField,
    pub changes: ObservationField,
}

/// The message of the informational diagnostic a scan reports for a root that does not exist:
/// the scan is complete and empty, and the note keeps a mistyped root visible.
pub const ROOT_MISSING: &str = "root does not exist yet";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanDiagnostic {
    pub path: PathBuf,
    pub message: String,
    /// Exclusions are useful to show but do not mean traversal could not finish.
    pub incomplete: bool,
}

impl ScanDiagnostic {
    /// The note a scan reports for a root that does not exist yet.
    pub fn is_root_missing(&self) -> bool {
        !self.incomplete && self.message == ROOT_MISSING
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalScan {
    pub checkouts: Vec<LocalCheckout>,
    pub pending_paths: Vec<PathBuf>,
    /// A probe's immutable target snapshot changed before its result could be applied.
    pub stale_paths: Vec<PathBuf>,
    pub diagnostics: Vec<ScanDiagnostic>,
    /// Git observations that failed; the checkout keeps the failed fields as errors.
    pub failures: Vec<ScanDiagnostic>,
    pub incomplete: bool,
    pub complete: bool,
}

#[derive(Debug, Clone)]
struct ConcreteDeclaration<'a> {
    reference: RepositoryRef,
    declaration: &'a RepositoryDeclaration,
    destination: PathBuf,
}

#[derive(Debug, Clone)]
struct WildcardDeclaration<'a> {
    reference: RepositoryRef,
    declaration: &'a RepositoryDeclaration,
    destination: PathBuf,
}

pub trait InventoryFilesystem {
    fn destination_state(&self, destination: &Path) -> CheckoutState;
    fn config_revision(&self, path: &Path) -> String;
}

pub fn build_inventory(
    filesystem: &impl InventoryFilesystem,
    config: &Config,
    config_path: &Path,
    home: &Path,
    scan: LocalScan,
) -> Result<InventoryReport, String> {
    let (concrete, wildcards, explicit_identities) = declarations(config, home)?;
    let mut rows_by_key = BTreeMap::new();
    let identities: Vec<String> = concrete
        .iter()
        .map(|declaration| declaration.reference.identity())
        .collect();
    let mut declared_by_identity: HashMap<&str, Vec<&ConcreteDeclaration<'_>>> = HashMap::new();
    for (declaration, identity) in concrete.iter().zip(&identities) {
        declared_by_identity
            .entry(identity)
            .or_default()
            .push(declaration);
    }

    for (declaration, identity) in concrete.iter().zip(&identities) {
        let identity = identity.clone();
        let (_, relationships) = registration_for(&identity, &wildcards, &explicit_identities);
        let key = declaration_key(&identity, &declaration.destination);
        rows_by_key.insert(
            key.clone(),
            InventoryRow {
                key,
                repository: identity,
                path: declaration.destination.to_string_lossy().into_owned(),
                registration: RegistrationState::Explicit,
                checkout: filesystem.destination_state(&declaration.destination),
                origin: declaration.reference.clone_url.clone(),
                remote: Some(declaration.reference.clone_url.clone()),
                origin_fact: None,
                branch: if scan.complete {
                    ObservationField::NotApplicable
                } else {
                    ObservationField::Pending
                },
                changes: if scan.complete {
                    ObservationField::NotApplicable
                } else {
                    ObservationField::Pending
                },
                configured_destination: Some(declaration.destination.clone()),
                observed_path: None,
                declaration: Some(declaration.declaration.url.clone()),
                warnings: relationships,
                markable: true,
                archived: None,
                remote_status: None,
            },
        );
    }

    for wildcard in &wildcards {
        let identity = format!("{}/*", wildcard.reference.identity());
        let key = RowKey::Pattern(identity.clone());
        rows_by_key.insert(
            key.clone(),
            InventoryRow {
                key,
                repository: identity,
                path: wildcard.destination.to_string_lossy().into_owned(),
                registration: RegistrationState::Pattern,
                checkout: CheckoutState::Pattern,
                origin: wildcard.reference.clone_url.clone(),
                remote: None,
                origin_fact: None,
                branch: ObservationField::NotApplicable,
                changes: ObservationField::NotApplicable,
                configured_destination: Some(wildcard.destination.clone()),
                observed_path: None,
                declaration: Some(wildcard.declaration.url.clone()),
                warnings: wildcard
                    .declaration
                    .exclude
                    .iter()
                    .map(|excluded| format!("excludes {excluded}"))
                    .collect(),
                markable: false,
                archived: None,
                remote_status: None,
            },
        );
    }

    let observed_paths: HashSet<&Path> = scan
        .checkouts
        .iter()
        .map(|checkout| checkout.path.as_path())
        .collect();
    for pending in &scan.pending_paths {
        if observed_paths.contains(pending.as_path()) {
            continue;
        }
        let key = RowKey::Path(pending.clone());
        rows_by_key.insert(
            key.clone(),
            InventoryRow {
                key,
                repository: pending.to_string_lossy().into_owned(),
                path: pending.to_string_lossy().into_owned(),
                registration: RegistrationState::Unregistered,
                checkout: CheckoutState::Unknown,
                origin: "pending".to_owned(),
                remote: None,
                origin_fact: None,
                branch: ObservationField::Pending,
                changes: ObservationField::Pending,
                configured_destination: None,
                observed_path: Some(pending.clone()),
                declaration: None,
                warnings: Vec::new(),
                markable: true,
                archived: None,
                remote_status: None,
            },
        );
    }

    for stale in &scan.stale_paths {
        let key = RowKey::Path(stale.clone());
        rows_by_key.insert(
            key.clone(),
            InventoryRow {
                key,
                repository: stale.to_string_lossy().into_owned(),
                path: stale.to_string_lossy().into_owned(),
                registration: RegistrationState::Unregistered,
                checkout: CheckoutState::Unknown,
                origin: "stale observation".to_owned(),
                remote: None,
                origin_fact: None,
                branch: ObservationField::Stale("target changed during observation".to_owned()),
                changes: ObservationField::Stale("target changed during observation".to_owned()),
                configured_destination: None,
                observed_path: Some(stale.clone()),
                declaration: None,
                warnings: vec!["Target changed before its Git observation completed".to_owned()],
                markable: true,
                archived: None,
                remote_status: None,
            },
        );
    }

    for checkout in scan.checkouts {
        let observed = checkout.path.clone();
        let stale = matches!(&checkout.branch, ObservationField::Stale(_))
            || matches!(&checkout.changes, ObservationField::Stale(_));
        if let Some(identity) = checkout.identity.clone() {
            let matching_declarations = declared_by_identity
                .get(identity.as_str())
                .map_or(&[][..], Vec::as_slice);
            let exact = matching_declarations
                .iter()
                .find(|declaration| declaration.destination == observed);
            if let Some(declaration) = exact {
                let key = declaration_key(&identity, &declaration.destination);
                if let Some(row) = rows_by_key.get_mut(&key) {
                    if !stale {
                        row.checkout = CheckoutState::Cloned;
                    }
                    row.origin = checkout.origin.display();
                    row.remote = checkout.origin.original();
                    row.origin_fact = Some(checkout.origin.clone());
                    row.branch = checkout.branch;
                    row.changes = checkout.changes;
                    row.observed_path = Some(observed);
                }
                continue;
            }

            let (registration, relationships) =
                registration_for(&identity, &wildcards, &explicit_identities);
            let wildcard_destination = wildcards
                .iter()
                .filter_map(|wildcard| {
                    wildcard_relative(&wildcard.reference, &identity)
                        .filter(|_| matches!(registration, RegistrationState::Wildcard { .. }))
                        .map(|relative| wildcard.destination.join(relative))
                })
                .next_back();
            let configured_destination = matching_declarations
                .first()
                .map(|declaration| declaration.destination.clone())
                .or(wildcard_destination);
            let mut warnings = matching_declarations
                .iter()
                .map(|declaration| declaration.destination.clone())
                .chain(configured_destination.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|destination| destination != &observed)
                .map(|destination| {
                    format!(
                        "misplaced checkout; found: {}; configured: {}",
                        observed.to_string_lossy(),
                        destination.to_string_lossy()
                    )
                })
                .collect::<Vec<_>>();
            if let Some(warning) = checkout.origin.warning() {
                warnings.push(warning);
            }
            warnings.extend(relationships);
            let key = RowKey::Checkout {
                identity: identity.clone(),
                path: observed.clone(),
            };
            rows_by_key.insert(
                key.clone(),
                InventoryRow {
                    key,
                    repository: identity,
                    path: observed.to_string_lossy().into_owned(),
                    registration,
                    checkout: if stale {
                        CheckoutState::Unknown
                    } else {
                        CheckoutState::Cloned
                    },
                    origin: checkout.origin.display(),
                    remote: checkout.origin.original(),
                    origin_fact: Some(checkout.origin.clone()),
                    branch: checkout.branch,
                    changes: checkout.changes,
                    configured_destination,
                    observed_path: Some(observed),
                    declaration: matching_declarations
                        .first()
                        .map(|declaration| declaration.declaration.url.clone()),
                    warnings,
                    markable: true,
                    archived: None,
                    remote_status: None,
                },
            );
        } else {
            let mut warnings = vec!["Identity is the exact path".to_owned()];
            if let Some(warning) = checkout.origin.warning() {
                warnings.push(warning);
            }
            let key = RowKey::Path(observed.clone());
            rows_by_key.insert(
                key.clone(),
                InventoryRow {
                    key,
                    repository: checkout.origin.display(),
                    path: observed.to_string_lossy().into_owned(),
                    registration: RegistrationState::Unregistered,
                    checkout: if stale {
                        CheckoutState::Unknown
                    } else {
                        CheckoutState::Cloned
                    },
                    origin: checkout.origin.display(),
                    remote: checkout.origin.original(),
                    origin_fact: Some(checkout.origin.clone()),
                    branch: checkout.branch,
                    changes: checkout.changes,
                    configured_destination: None,
                    observed_path: Some(observed),
                    declaration: None,
                    warnings,
                    markable: true,
                    archived: None,
                    remote_status: None,
                },
            );
        }
    }

    // A real checkout at another declared destination is a conflict, not a missing clone target.
    let mut cloned_checkouts: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for row in rows_by_key.values() {
        if let (RowKey::Checkout { identity, .. }, Some(path), CheckoutState::Cloned) =
            (&row.key, &row.observed_path, &row.checkout)
        {
            cloned_checkouts
                .entry(path.clone())
                .or_default()
                .push(identity.clone());
        }
    }
    for (declaration, identity) in concrete.iter().zip(&identities) {
        let key = declaration_key(identity, &declaration.destination);
        let has_conflicting_checkout = cloned_checkouts
            .get(&declaration.destination)
            .is_some_and(|found| found.iter().any(|other| other != identity));
        if let Some(row) = rows_by_key.get_mut(&key)
            && has_conflicting_checkout
        {
            row.checkout = CheckoutState::Conflict;
            row.warnings
                .push("configured destination contains a different checkout".to_owned());
        }
    }

    Ok(InventoryReport {
        rows: rows_by_key.into_values().collect(),
        diagnostics: scan.diagnostics,
        config_revision: filesystem.config_revision(config_path),
        observation_generation: 1,
        incomplete: scan.incomplete,
    })
}

type Declarations<'a> = (
    Vec<ConcreteDeclaration<'a>>,
    Vec<WildcardDeclaration<'a>>,
    HashSet<String>,
);

fn declarations<'a>(config: &'a Config, home: &Path) -> Result<Declarations<'a>, String> {
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let mut concrete = Vec::new();
    let mut wildcards = Vec::new();
    for declaration in &config.repositories {
        let reference =
            RepositoryRef::parse(&declaration.url).map_err(|error| error.to_string())?;
        let destination = configured_destination(config, &reference, &root);
        if reference.is_wildcard() {
            wildcards.push(WildcardDeclaration {
                reference,
                declaration,
                destination,
            });
        } else {
            concrete.push(ConcreteDeclaration {
                reference,
                declaration,
                destination,
            });
        }
    }
    let explicit = concrete
        .iter()
        .map(|declaration| declaration.reference.identity())
        .collect();
    Ok((concrete, wildcards, explicit))
}

/// Expands every wildcard declaration through its provider and merges the members into the
/// rows. A member with a row of its own identity only gains its archived flag; one without a
/// local row becomes a declaration row at its configured destination, `remote only` while
/// nothing is there. Exclusions and explicit declarations take precedence over members, and an
/// archived member gains no row of its own unless `include_archived` is set. A provider failure
/// is returned and leaves every other row in place.
pub(crate) fn add_remote_members<P: ProviderCatalog>(
    report: &mut InventoryReport,
    filesystem: &impl InventoryFilesystem,
    config: &Config,
    home: &Path,
    providers: &P,
    include_archived: bool,
) -> Result<Vec<ProviderFailure>, String> {
    let (_, wildcards, explicit) = declarations(config, home)?;
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for wildcard in &wildcards {
        let members = match providers.expand(&wildcard.reference, include_archived) {
            Ok(members) => members,
            Err(error) => {
                failures.push(ProviderFailure {
                    provider: wildcard.reference.host.clone(),
                    error,
                });
                continue;
            }
        };
        for member in &members {
            crate::domain::repository::validate_reference_safety(&member.reference.clone_url)
                .map_err(|error| error.to_string())?;
        }
        merge_members(
            report,
            filesystem,
            config,
            &root,
            &wildcards,
            &explicit,
            members,
            include_archived,
            false,
        );
    }
    report.rows.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(failures)
}

/// Merges provider catalogs into the rows as `add_remote_members` merges wildcard members. A
/// repository no declaration registers gains a row too, a candidate: unregistered or excluded,
/// `remote only` while nothing is at its destination. Returns the keys of the added rows.
pub(crate) fn add_remote_catalog(
    report: &mut InventoryReport,
    filesystem: &impl InventoryFilesystem,
    config: &Config,
    home: &Path,
    members: Vec<RepositorySummary>,
    include_archived: bool,
) -> Result<Vec<RowKey>, String> {
    let (_, wildcards, explicit) = declarations(config, home)?;
    let root = config
        .resolve_root(home)
        .map_err(|error| error.to_string())?;
    let added = merge_members(
        report,
        filesystem,
        config,
        &root,
        &wildcards,
        &explicit,
        members,
        include_archived,
        true,
    );
    report.rows.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(added)
}

#[allow(clippy::too_many_arguments)]
fn merge_members(
    report: &mut InventoryReport,
    filesystem: &impl InventoryFilesystem,
    config: &Config,
    root: &Path,
    wildcards: &[WildcardDeclaration<'_>],
    explicit: &HashSet<String>,
    mut members: Vec<RepositorySummary>,
    include_archived: bool,
    candidates: bool,
) -> Vec<RowKey> {
    let mut added = Vec::new();
    let mut rows_by_identity: HashMap<String, Vec<usize>> = HashMap::new();
    let mut cloned_paths: HashSet<PathBuf> = HashSet::new();
    for (index, row) in report.rows.iter().enumerate() {
        if let Some(identity) = row_identity(row) {
            rows_by_identity
                .entry(identity.to_owned())
                .or_default()
                .push(index);
        }
        if row.checkout == CheckoutState::Cloned
            && let Some(path) = &row.observed_path
        {
            cloned_paths.insert(path.clone());
        }
    }
    members.sort_by_key(|member| member.reference.identity());
    for member in members {
        let identity = member.reference.identity();
        let remote_status = RemoteStatus::of(member.archived, member.visibility);
        let mut known = false;
        for &index in rows_by_identity.get(&identity).into_iter().flatten() {
            report.rows[index].archived = Some(member.archived);
            report.rows[index].remote_status = remote_status;
            known = true;
        }
        if known || member.archived && !include_archived {
            continue;
        }
        let (registration, relationships) = registration_for(&identity, wildcards, explicit);
        if !candidates && !matches!(registration, RegistrationState::Wildcard { .. }) {
            continue;
        }
        let destination = configured_destination(config, &member.reference, root);
        let occupied = cloned_paths.contains(&destination);
        let checkout = match filesystem.destination_state(&destination) {
            CheckoutState::Missing => CheckoutState::RemoteOnly,
            _ if occupied => CheckoutState::Conflict,
            state => state,
        };
        let key = declaration_key(&identity, &destination);
        added.push(key.clone());
        rows_by_identity
            .entry(identity.clone())
            .or_default()
            .push(report.rows.len());
        report.rows.push(InventoryRow {
            key,
            repository: identity,
            path: destination.to_string_lossy().into_owned(),
            registration,
            checkout,
            origin: member.reference.clone_url.clone(),
            remote: Some(member.reference.clone_url),
            origin_fact: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: Some(destination),
            observed_path: None,
            declaration: None,
            warnings: relationships,
            markable: true,
            archived: Some(member.archived),
            remote_status,
        });
    }
    added
}

/// The repositories of declared rows on a github provider whose catalog is ready but did not
/// list them. Only a per-repository lookup tells whether they exist: a catalog lists what the
/// account sees, so a repository of another owner is absent from it.
pub fn unlisted(
    report: &InventoryReport,
    config: &Config,
    ready: impl Fn(&str) -> bool,
) -> Vec<RepositoryRef> {
    let mut seen = HashSet::new();
    report
        .rows
        .iter()
        .filter(|row| {
            row.archived.is_none()
                && matches!(
                    row.registration,
                    RegistrationState::Explicit | RegistrationState::Wildcard { .. }
                )
        })
        .filter_map(|row| RepositoryRef::parse(row_identity(row)?).ok())
        .filter(|reference| {
            let host = reference.host.to_ascii_lowercase();
            !reference.is_wildcard()
                && config
                    .providers
                    .get(&host)
                    .is_some_and(|provider| provider.preset == "github")
                && ready(&host)
        })
        .filter(|reference| seen.insert(reference.identity()))
        .collect()
}

/// Applies per-repository lookups, by identity, to the rows no catalog listed: a catalog's
/// answer always wins. A lookup that answered `None` left the state unknown.
pub fn apply_lookups(
    report: &mut InventoryReport,
    lookups: &HashMap<String, Option<RemoteStatus>>,
) {
    for row in report.rows.iter_mut().filter(|row| row.archived.is_none()) {
        if let Some(status) = row_identity(row).and_then(|identity| *lookups.get(identity)?) {
            row.remote_status = Some(status);
        }
    }
}

fn row_identity(row: &InventoryRow) -> Option<&str> {
    match &row.key {
        RowKey::Declaration { identity, .. } | RowKey::Checkout { identity, .. } => Some(identity),
        RowKey::Path(_) | RowKey::Pattern(_) => None,
    }
}

fn declaration_key(identity: &str, destination: &Path) -> RowKey {
    RowKey::Declaration {
        identity: identity.to_owned(),
        destination: destination.to_path_buf(),
    }
}

pub(crate) fn configured_destination(
    config: &Config,
    reference: &RepositoryRef,
    root: &Path,
) -> PathBuf {
    let Some(provider) = config.providers.get(&reference.host) else {
        return reference.destination(root);
    };
    let mut destination = root.to_path_buf();
    for segment in provider
        .prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        destination.push(segment);
    }
    let mut remote_segments = reference.path.split('/');
    if provider.preset == "bitbucket-data-center"
        && let Some(project) = remote_segments.next()
    {
        destination.push(project.strip_prefix('~').unwrap_or(project));
    }
    for segment in remote_segments {
        destination.push(segment);
    }
    destination
}

fn registration_for(
    identity: &str,
    wildcards: &[WildcardDeclaration<'_>],
    explicit_identities: &HashSet<String>,
) -> (RegistrationState, Vec<String>) {
    let explicit = explicit_identities.contains(identity);
    let mut matching = Vec::new();
    let mut eligible = Vec::new();
    for wildcard in wildcards {
        let Some(relative) = wildcard_relative(&wildcard.reference, identity) else {
            continue;
        };
        let excluded = wildcard
            .declaration
            .exclude
            .iter()
            .any(|excluded| excluded == relative);
        matching.push((wildcard, excluded));
        if !excluded {
            eligible.push(wildcard);
        }
    }
    let relationships = matching
        .iter()
        .map(|(wildcard, excluded)| {
            format!(
                "{} by {}",
                if *excluded { "excluded" } else { "covered" },
                wildcard.declaration.url
            )
        })
        .collect();
    if explicit {
        return (RegistrationState::Explicit, relationships);
    }
    if let Some(wildcard) = eligible.last() {
        return (
            RegistrationState::Wildcard {
                pattern: wildcard.declaration.url.clone(),
            },
            relationships,
        );
    }
    if let Some((wildcard, _)) = matching.last() {
        return (
            RegistrationState::Excluded {
                pattern: wildcard.declaration.url.clone(),
            },
            relationships,
        );
    }
    (RegistrationState::Unregistered, relationships)
}

pub(crate) fn wildcard_relative<'a>(pattern: &RepositoryRef, identity: &'a str) -> Option<&'a str> {
    identity
        .strip_prefix(&(pattern.identity() + "/"))
        .filter(|relative| !relative.is_empty())
}

impl InventoryRow {
    /// Whether a declaration covers the row: explicit, by a wildcard, or the pattern itself.
    pub fn registered(&self) -> bool {
        matches!(
            self.registration,
            RegistrationState::Explicit
                | RegistrationState::Wildcard { .. }
                | RegistrationState::Pattern
        )
    }
}

impl RegistrationState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Wildcard { .. } => "wildcard",
            Self::Excluded { .. } => "excluded",
            Self::Unregistered => "unregistered",
            Self::Pattern => "pattern",
        }
    }
}

impl CheckoutState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cloned => "cloned",
            Self::Missing => "missing",
            Self::Pattern => "pattern",
            Self::Unknown => "unknown",
            Self::Conflict => "conflict",
            Self::Unreadable => "unreadable",
            Self::RemoteOnly => "remote only",
        }
    }
}

impl ObservationField {
    pub fn label(&self) -> String {
        match self {
            Self::Pending => "pending".to_owned(),
            Self::Known(value) => value.clone(),
            Self::Clean => "known clean".to_owned(),
            Self::Dirty(value) => value.clone(),
            Self::Error(value) => format!("error: {value}"),
            Self::Stale(value) => format!("stale: {value}"),
            Self::NotApplicable => "-".to_owned(),
        }
    }
}

/// Session marks, kept by exact target. Pattern rows are never marked.
#[derive(Debug, Clone, Default)]
pub struct Marks(BTreeSet<RowKey>);

impl Marks {
    pub fn contains(&self, key: &RowKey) -> bool {
        self.0.contains(key)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &RowKey> {
        self.0.iter()
    }

    /// Returns whether the row is marked afterwards.
    pub fn toggle(&mut self, row: &InventoryRow) -> bool {
        row.markable && !self.0.remove(&row.key) && self.0.insert(row.key.clone())
    }

    pub fn mark_all<'a>(&mut self, rows: impl IntoIterator<Item = &'a InventoryRow>) {
        self.0.extend(
            rows.into_iter()
                .filter(|row| row.markable)
                .map(|row| row.key.clone()),
        );
    }

    /// Returns how many marks were cleared.
    pub fn clear(&mut self) -> usize {
        let cleared = self.0.len();
        self.0.clear();
        cleared
    }

    pub fn hidden(&self, visible: &[&InventoryRow]) -> usize {
        if self.0.is_empty() {
            return 0;
        }
        let shown: HashSet<&RowKey> = visible.iter().map(|row| &row.key).collect();
        self.0.iter().filter(|key| !shown.contains(key)).count()
    }

    /// Moves each mark to the row that now shows its exact target. A mark whose target is gone
    /// is dropped only from a complete scan, since an incomplete one has not seen every row yet.
    /// Returns how many marks were dropped.
    /// A mark moves only when exactly one row shows its target.
    pub fn reconcile(&mut self, rows: &[InventoryRow], complete: bool) -> usize {
        if self.0.is_empty() {
            return 0;
        }
        let present: HashSet<&RowKey> = rows
            .iter()
            .filter(|row| row.markable)
            .map(|row| &row.key)
            .collect();
        let missing: Vec<RowKey> = self
            .0
            .iter()
            .filter(|key| !present.contains(key))
            .cloned()
            .collect();
        let mut dropped = 0;
        for key in missing {
            let mut targets = rows
                .iter()
                .filter(|row| row.markable && same_target(&key, row));
            match (targets.next(), targets.next()) {
                (Some(row), None) => {
                    self.0.remove(&key);
                    self.0.insert(row.key.clone());
                }
                _ if complete => {
                    self.0.remove(&key);
                    dropped += 1;
                }
                _ => {}
            }
        }
        dropped
    }
}

/// A path mark targets the checkout observed at that path, never a declaration that merely
/// names it; a declaration and a checkout of the same identity at the same path are one target.
fn same_target(marked: &RowKey, row: &InventoryRow) -> bool {
    match (marked, &row.key) {
        (RowKey::Path(path), _) => row.observed_path.as_ref() == Some(path),
        (
            RowKey::Declaration {
                identity,
                destination: path,
            }
            | RowKey::Checkout { identity, path },
            RowKey::Declaration {
                identity: other,
                destination: other_path,
            }
            | RowKey::Checkout {
                identity: other,
                path: other_path,
            },
        ) => identity == other && path == other_path,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyField {
    Path,
    Origin,
    Identity,
    Branch,
}

impl CopyField {
    pub const ALL: [Self; 4] = [Self::Path, Self::Origin, Self::Identity, Self::Branch];

    pub fn label(self) -> &'static str {
        match self {
            Self::Path => "Path",
            Self::Origin => "Origin",
            Self::Identity => "Identity",
            Self::Branch => "Branch",
        }
    }
}

/// The checkout whose worktrees a row can show, or why the row has none.
pub fn worktree_checkout(row: &InventoryRow) -> Result<&Path, &'static str> {
    match row.checkout {
        CheckoutState::Pattern => Err("a pattern has no checkout"),
        CheckoutState::Missing => Err("no checkout at the destination"),
        CheckoutState::RemoteOnly => Err("remote only, no local checkout"),
        CheckoutState::Unreadable => Err("the checkout is unreadable"),
        CheckoutState::Cloned | CheckoutState::Unknown | CheckoutState::Conflict => {
            row.observed_path.as_deref().ok_or("no checkout observed")
        }
    }
}

/// The original full value a Copy choice puts on the clipboard, or why the row has none.
pub fn copy_value(row: &InventoryRow, field: CopyField) -> Result<OsString, &'static str> {
    match field {
        CopyField::Path if matches!(row.key, RowKey::Pattern(_)) => {
            Err("patterns have no single path")
        }
        CopyField::Path => row
            .observed_path
            .as_ref()
            .or(row.configured_destination.as_ref())
            .map(|path| path.clone().into_os_string())
            .ok_or("no path known"),
        CopyField::Origin => row
            .remote
            .clone()
            .map(OsString::from)
            .ok_or("no origin observed"),
        CopyField::Identity => match &row.key {
            RowKey::Declaration { identity, .. }
            | RowKey::Checkout { identity, .. }
            | RowKey::Pattern(identity) => Ok(identity.into()),
            RowKey::Path(_) => Err("identity is the exact path"),
        },
        CopyField::Branch => match &row.branch {
            ObservationField::Known(branch) if branch.starts_with("detached HEAD ") => {
                Err("detached HEAD")
            }
            ObservationField::Known(branch) => Ok(branch.into()),
            ObservationField::Pending => Err("not observed yet"),
            ObservationField::Stale(_) => Err("stale until observed again"),
            ObservationField::Error(_) => Err("observation failed"),
            _ => Err("no checkout"),
        },
    }
}

/// The repository web address Open origin may hand to the platform opener. It is always HTTPS
/// and built from the normalized identity, never from the origin's own scheme.
pub fn web_url(row: &InventoryRow, config: &Config) -> Result<String, &'static str> {
    let identity = match &row.key {
        RowKey::Declaration { identity, .. } | RowKey::Checkout { identity, .. } => identity,
        RowKey::Pattern(_) => return Err("patterns have no single repository"),
        RowKey::Path(_) => return Err("no supported origin"),
    };
    let unsupported = "no supported origin";
    if identity.starts_with("file://") {
        return Err(unsupported);
    }
    if !identity
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-/~".contains(character))
    {
        return Err("the repository identity has characters a web address cannot carry safely");
    }
    let (host, path) = identity.split_once('/').ok_or(unsupported)?;
    let data_center = config
        .providers
        .get(host)
        .is_some_and(|provider| provider.preset == "bitbucket-data-center");
    let path = match path.split_once('/') {
        Some((user, name)) if data_center && !name.contains('/') => match user.strip_prefix('~') {
            Some(user) => format!("users/{user}/repos/{name}/browse"),
            None => format!("projects/{user}/repos/{name}/browse"),
        },
        _ => path.to_owned(),
    };
    let url = url::Url::parse(&format!("https://{host}/{path}")).map_err(|_| unsupported)?;
    (url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().len() > 1)
        .then(|| url.to_string())
        .ok_or(unsupported)
}

/// The lowercase reason at the end of an error chain, as the session problems list words it:
/// `unreadable path: Permission denied (os error 13)` reads `permission denied`.
pub fn problem_reason(error: &str) -> String {
    let mut reason = error.trim();
    loop {
        let unwrapped = [
            "unreadable path: ",
            "unreadable directory entry: ",
            "could not inspect bare repository: ",
            "fatal: ",
            "error: ",
        ]
        .iter()
        .find_map(|prefix| reason.strip_prefix(prefix))
        .or_else(|| {
            reason
                .strip_prefix("git command failed (exit ")
                .and_then(|rest| rest.split_once("): "))
                .map(|(_, rest)| rest)
        });
        match unwrapped {
            Some(rest) => reason = rest.trim(),
            None => break,
        }
    }
    let reason = match reason.rfind(" (os error ") {
        Some(index) if reason.ends_with(')') => &reason[..index],
        _ => reason,
    };
    let mut characters = reason.chars();
    characters
        .next()
        .map(|first| first.to_lowercase().chain(characters).collect())
        .unwrap_or_else(|| error.to_owned())
}

/// A failure that makes the inventory session exit 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProblem {
    pub at: SystemTime,
    pub source: String,
    pub message: String,
}

/// Every session failure in order. A later successful retry never removes one.
#[derive(Debug, Clone, Default)]
pub struct SessionProblems(Vec<SessionProblem>);

impl SessionProblems {
    pub fn record(&mut self, source: impl Into<String>, message: impl Into<String>) {
        self.0.push(SessionProblem {
            at: SystemTime::now(),
            source: source.into(),
            message: message.into(),
        });
    }

    pub fn entries(&self) -> &[SessionProblem] {
        &self.0
    }

    pub fn exit_code(&self) -> i32 {
        i32::from(!self.0.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_error_is_unknown_and_only_an_empty_answer_is_missing() {
        let state = |archived, visibility| RemoteState {
            name: "org/new".to_owned(),
            archived,
            visibility,
        };
        assert_eq!(RemoteStatus::looked_up(Err("HTTP 502".to_owned())), None);
        assert_eq!(
            RemoteStatus::looked_up(Ok(None)),
            Some(RemoteStatus::Missing)
        );
        assert_eq!(
            RemoteStatus::looked_up(Ok(Some(state(true, Visibility::Public)))),
            Some(RemoteStatus::Archived)
        );
        assert_eq!(
            RemoteStatus::looked_up(Ok(Some(state(false, Visibility::Internal)))),
            Some(RemoteStatus::Private)
        );
        assert_eq!(
            RemoteStatus::looked_up(Ok(Some(state(false, Visibility::Public)))),
            Some(RemoteStatus::Public)
        );
    }

    #[test]
    fn row_keys_preserve_non_utf8_paths_that_share_a_lossy_display() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let left = PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0x80]));
            let right = PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0x81]));
            assert_eq!(left.to_string_lossy(), right.to_string_lossy());
            assert_ne!(RowKey::Path(left), RowKey::Path(right));
        }
    }

    fn config(declarations: &[(&str, &[&str])]) -> Config {
        Config {
            root: "repos".to_owned(),
            providers: [(
                "github.com".to_owned(),
                crate::domain::config::ProviderConfig {
                    preset: "github".to_owned(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            repositories: declarations
                .iter()
                .map(|(url, exclude)| RepositoryDeclaration {
                    url: (*url).to_owned(),
                    post_clone: None,
                    exclude: exclude.iter().map(|value| (*value).to_owned()).collect(),
                })
                .collect(),
        }
    }

    fn checkout(path: PathBuf, identity: &str) -> LocalCheckout {
        LocalCheckout {
            path,
            identity: Some(identity.to_owned()),
            origin: OriginFact::Supported {
                original: format!("git@{}.git", identity),
                identity: identity.to_owned(),
            },
            branch: ObservationField::Known("main".to_owned()),
            changes: ObservationField::Clean,
        }
    }

    #[cfg(unix)]
    #[test]
    fn declared_symlink_destination_is_unreadable_not_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join("repos/org")).unwrap();
        let destination = home.join("repos/org/repo");
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(target, &destination).unwrap();
        let report = build_inventory(
            &crate::infrastructure::inventory::LocalInventoryFilesystem,
            &config(&[("github.com/org/repo", &[])]),
            &temp.path().join("config.toml"),
            &home,
            LocalScan::default(),
        )
        .unwrap();
        assert_eq!(report.rows[0].checkout, CheckoutState::Unreadable);
    }

    #[test]
    fn misplaced_wildcard_checkout_keeps_configured_destination_warning() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let observed = home.join("repos/elsewhere/repo");
        let report = build_inventory(
            &crate::infrastructure::inventory::LocalInventoryFilesystem,
            &config(&[("github.com/org/*", &[])]),
            &temp.path().join("config.toml"),
            &home,
            LocalScan {
                checkouts: vec![checkout(observed.clone(), "github.com/org/repo")],
                complete: true,
                ..Default::default()
            },
        )
        .unwrap();
        let row = report
            .rows
            .iter()
            .find(|row| row.observed_path.as_ref() == Some(&observed))
            .unwrap();
        assert_eq!(
            row.configured_destination,
            Some(home.join("repos/org/repo"))
        );
        assert!(row.warnings.iter().any(|warning| warning
            == &format!(
                "misplaced checkout; found: {}; configured: {}",
                observed.display(),
                home.join("repos/org/repo").display()
            )));
    }

    #[test]
    fn explicit_relationships_include_overlapping_wildcards_in_each_declaration_order() {
        for declarations in [
            vec![
                ("github.com/org/*", &[][..]),
                ("github.com/org/team/*", &[][..]),
                ("github.com/org/team/repo", &[][..]),
            ],
            vec![
                ("github.com/org/team/*", &[][..]),
                ("github.com/org/*", &[][..]),
                ("github.com/org/team/repo", &[][..]),
            ],
        ] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let observed = home.join("repos/elsewhere/repo");
            let report = build_inventory(
                &crate::infrastructure::inventory::LocalInventoryFilesystem,
                &config(&declarations),
                &temp.path().join("config.toml"),
                &home,
                LocalScan {
                    checkouts: vec![checkout(observed.clone(), "github.com/org/team/repo")],
                    complete: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let row = report
                .rows
                .iter()
                .find(|row| row.observed_path.as_ref() == Some(&observed))
                .unwrap();
            assert_eq!(row.registration, RegistrationState::Explicit);
            assert!(
                row.warnings
                    .iter()
                    .any(|warning| warning == "covered by github.com/org/*")
            );
            assert!(
                row.warnings
                    .iter()
                    .any(|warning| warning == "covered by github.com/org/team/*")
            );
        }
    }

    fn row(key: RowKey) -> InventoryRow {
        InventoryRow {
            markable: !matches!(key, RowKey::Pattern(_)),
            key,
            repository: String::new(),
            path: String::new(),
            registration: RegistrationState::Unregistered,
            checkout: CheckoutState::Cloned,
            origin: String::new(),
            remote: None,
            origin_fact: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: None,
            warnings: Vec::new(),
            archived: None,
            remote_status: None,
        }
    }

    fn checkout_key(identity: &str, path: &str) -> RowKey {
        RowKey::Checkout {
            identity: identity.to_owned(),
            path: path.into(),
        }
    }

    #[test]
    fn web_url_is_https_from_the_identity_and_never_the_origin_scheme() {
        let mut providers = config(&[]).providers;
        providers.insert(
            "git.example.com".to_owned(),
            crate::domain::config::ProviderConfig {
                preset: "bitbucket-data-center".to_owned(),
                ..Default::default()
            },
        );
        let config = Config {
            providers,
            ..config(&[])
        };
        for (key, expected) in [
            (
                checkout_key("github.com/org/alpha", "/r/alpha"),
                Ok("https://github.com/org/alpha"),
            ),
            (
                RowKey::Declaration {
                    identity: "gitlab.com/group/sub/beta".to_owned(),
                    destination: "/r/beta".into(),
                },
                Ok("https://gitlab.com/group/sub/beta"),
            ),
            (
                checkout_key("git.example.com/PROJ/repo", "/r/repo"),
                Ok("https://git.example.com/projects/PROJ/repos/repo/browse"),
            ),
            (
                checkout_key("git.example.com/~nik/repo", "/r/repo"),
                Ok("https://git.example.com/users/nik/repos/repo/browse"),
            ),
            (
                checkout_key("file:///srv/git/alpha.git", "/r/alpha"),
                Err("no supported origin"),
            ),
            (
                checkout_key("user@evil.example/org/x", "/r/x"),
                Err("the repository identity has characters a web address cannot carry safely"),
            ),
            (
                checkout_key("github.com/org/x?y#z", "/r/x"),
                Err("the repository identity has characters a web address cannot carry safely"),
            ),
            (RowKey::Path("/r/x".into()), Err("no supported origin")),
            (
                checkout_key("github.com/org/a$(touch)'b", "/r/x"),
                Err("the repository identity has characters a web address cannot carry safely"),
            ),
            (
                checkout_key("github.com/org/a;b&c", "/r/x"),
                Err("the repository identity has characters a web address cannot carry safely"),
            ),
            (
                RowKey::Pattern("github.com/org/*".to_owned()),
                Err("patterns have no single repository"),
            ),
        ] {
            assert_eq!(
                web_url(&row(key.clone()), &config).as_deref(),
                expected.as_ref().copied(),
                "{key:?}"
            );
        }
    }

    #[test]
    fn problem_reasons_drop_known_wrappers_and_keep_the_reason() {
        assert_eq!(
            problem_reason("unreadable path: Permission denied (os error 13)"),
            "permission denied"
        );
        assert_eq!(
            problem_reason(
                "git command failed (exit Some(128)): fatal: not a git repository (or any of the parent directories): .git\n"
            ),
            "not a git repository (or any of the parent directories): .git"
        );
        assert_eq!(
            problem_reason(
                "git command failed (exit Some(128)): fatal: not a git repository: '/r/x/.git/worktrees/w'"
            ),
            "not a git repository: '/r/x/.git/worktrees/w'"
        );
        assert_eq!(
            problem_reason(
                "could not inspect bare repository: git command failed (exit Some(128)): fatal: detected dubious ownership in repository at '/r/x'"
            ),
            "detected dubious ownership in repository at '/r/x'"
        );
        assert_eq!(problem_reason("cancelled"), "cancelled");
    }

    #[test]
    fn copy_values_are_original_and_display_text_is_never_copied() {
        let mut checkout = row(checkout_key("github.com/org/alpha", "/r/a\tb/alpha"));
        checkout.observed_path = Some("/r/a\tb/alpha".into());
        checkout.origin = "git@github.com:org/alpha.git".to_owned();
        checkout.remote = Some("git@github.com:org/alpha.git".to_owned());
        checkout.branch = ObservationField::Known("feature/long".to_owned());
        let values = CopyField::ALL.map(|field| copy_value(&checkout, field));
        assert_eq!(
            values,
            [
                Ok("/r/a\tb/alpha".into()),
                Ok("git@github.com:org/alpha.git".into()),
                Ok("github.com/org/alpha".into()),
                Ok("feature/long".into()),
            ]
        );

        let mut originless = row(RowKey::Path("/r/scratch".into()));
        originless.observed_path = Some("/r/scratch".into());
        originless.origin = "No origin".to_owned();
        originless.branch = ObservationField::Known("detached HEAD abc123".to_owned());
        assert_eq!(
            CopyField::ALL.map(|field| copy_value(&originless, field)),
            [
                Ok("/r/scratch".into()),
                Err("no origin observed"),
                Err("identity is the exact path"),
                Err("detached HEAD"),
            ]
        );
        let pattern = row(RowKey::Pattern("github.com/org/*".to_owned()));
        assert_eq!(
            copy_value(&pattern, CopyField::Path),
            Err("patterns have no single path")
        );
    }

    #[test]
    fn marks_follow_exact_targets_and_drop_vanished_ones_only_when_complete() {
        let pending = row(RowKey::Path("/r/alpha".into()));
        let gone = row(RowKey::Path("/r/gone".into()));
        let pattern = row(RowKey::Pattern("github.com/org/*".to_owned()));
        let mut marks = Marks::default();
        assert!(marks.toggle(&pending));
        assert!(marks.toggle(&gone));
        assert!(!marks.toggle(&pattern));
        assert_eq!(marks.hidden(&[&pending]), 1);

        let mut identified = row(checkout_key("github.com/org/alpha", "/r/alpha"));
        identified.observed_path = Some("/r/alpha".into());
        let other = row(checkout_key("github.com/org/other", "/r/other"));
        let rows = [identified.clone(), other.clone()];
        assert_eq!(marks.reconcile(&rows, false), 0);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks.reconcile(&rows, true), 1);
        assert!(marks.contains(&identified.key));
        assert!(!marks.contains(&other.key));

        let mut shared = Marks::default();
        shared.toggle(&row(RowKey::Path("/r/shared".into())));
        let declared = row(RowKey::Declaration {
            identity: "github.com/org/declared".to_owned(),
            destination: "/r/shared".into(),
        });
        assert_eq!(shared.reconcile(std::slice::from_ref(&declared), false), 0);
        assert!(shared.contains(&RowKey::Path("/r/shared".into())));
        let mut found = row(checkout_key("github.com/org/found", "/r/shared"));
        found.observed_path = Some("/r/shared".into());
        let conflict = [declared, found.clone()];
        assert_eq!(shared.reconcile(&conflict, true), 0);
        assert!(shared.contains(&found.key));
        assert!(!shared.contains(&conflict[0].key));

        marks.mark_all([&identified, &other, &pattern]);
        assert_eq!(marks.len(), 2);
        assert!(!marks.toggle(&other));
        assert_eq!(marks.clear(), 1);
        assert!(marks.is_empty());
    }

    struct FakeFilesystem(Vec<PathBuf>);

    impl InventoryFilesystem for FakeFilesystem {
        fn destination_state(&self, destination: &Path) -> CheckoutState {
            if self.0.iter().any(|path| path == destination) {
                CheckoutState::Cloned
            } else {
                CheckoutState::Missing
            }
        }

        fn config_revision(&self, _: &Path) -> String {
            "rev".to_owned()
        }
    }

    fn summary(url: &str, archived: bool) -> RepositorySummary {
        RepositorySummary {
            reference: RepositoryRef::parse(url).unwrap(),
            archived,
            visibility: None,
        }
    }

    fn render(report: &InventoryReport) -> String {
        report
            .rows
            .iter()
            .map(|row| {
                format!(
                    "{:?} | {} | {} | {:?} | {:?} | {} | {:?} | {:?} | {:?} | {:?} | {:?} | {:?}\n",
                    row.key,
                    row.repository,
                    row.path,
                    row.registration,
                    row.checkout,
                    row.origin,
                    row.branch,
                    row.changes,
                    row.observed_path,
                    row.declaration,
                    row.warnings,
                    row.archived,
                )
            })
            .collect()
    }

    /// Pins the whole merge on a mixed fixture: declarations, a conflict, a misplaced checkout,
    /// pending and stale paths, and provider members with candidates, archived flags and an
    /// occupied destination.
    #[test]
    fn mixed_inventory_with_remote_rows_is_pinned() {
        let home = PathBuf::from("/h");
        let at = |relative: &str| home.join("repos").join(relative);
        let config = config(&[
            ("github.com/org/a", &[]),
            ("github.com/org/b", &[]),
            ("github.com/org/c", &[]),
            ("github.com/org/d", &[]),
            ("github.com/team/*", &["skip"]),
        ]);
        let mut stale = checkout(at("other/stale"), "github.com/other/stale");
        stale.branch = ObservationField::Stale("moved".to_owned());
        let scan = LocalScan {
            checkouts: vec![
                checkout(at("org/a"), "github.com/org/a"),
                checkout(at("org/c"), "github.com/org/x"),
                checkout(PathBuf::from("/h/elsewhere/d"), "github.com/org/d"),
                checkout(at("other/u"), "github.com/other/u"),
                checkout(at("zzz/occ"), "github.com/org/y"),
                checkout(at("team/m1"), "github.com/team/m1"),
                stale,
                LocalCheckout {
                    identity: None,
                    ..checkout(at("loose"), "unused")
                },
            ],
            pending_paths: vec![at("org/a"), at("pending/one")],
            stale_paths: vec![at("stale/path")],
            complete: false,
            ..Default::default()
        };
        let filesystem = FakeFilesystem(vec![at("org/c"), at("zzz/occ")]);
        let mut report =
            build_inventory(&filesystem, &config, Path::new("/c.toml"), &home, scan).unwrap();
        add_remote_catalog(
            &mut report,
            &filesystem,
            &config,
            &home,
            vec![
                summary("github.com/org/a", true),
                summary("github.com/other/u", false),
                summary("github.com/team/m1", false),
                summary("github.com/team/m2", false),
                summary("github.com/team/m2", true),
                summary("github.com/team/skip", false),
                summary("github.com/team/old", true),
                summary("github.com/other/new", false),
                summary("github.com/zzz/occ", false),
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            render(&report),
            r##"Declaration { identity: "github.com/org/a", destination: "/h/repos/org/a" } | github.com/org/a | /h/repos/org/a | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/org/a") | Some("github.com/org/a") | [] | Some(true)
Declaration { identity: "github.com/org/b", destination: "/h/repos/org/b" } | github.com/org/b | /h/repos/org/b | Explicit | Missing | git@github.com:org/b.git | Pending | Pending | None | Some("github.com/org/b") | [] | None
Declaration { identity: "github.com/org/c", destination: "/h/repos/org/c" } | github.com/org/c | /h/repos/org/c | Explicit | Conflict | git@github.com:org/c.git | Pending | Pending | None | Some("github.com/org/c") | ["configured destination contains a different checkout"] | None
Declaration { identity: "github.com/org/d", destination: "/h/repos/org/d" } | github.com/org/d | /h/repos/org/d | Explicit | Missing | git@github.com:org/d.git | Pending | Pending | None | Some("github.com/org/d") | [] | None
Declaration { identity: "github.com/other/new", destination: "/h/repos/other/new" } | github.com/other/new | /h/repos/other/new | Unregistered | RemoteOnly | git@github.com:other/new.git | NotApplicable | NotApplicable | None | None | [] | Some(false)
Declaration { identity: "github.com/team/m2", destination: "/h/repos/team/m2" } | github.com/team/m2 | /h/repos/team/m2 | Wildcard { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/m2.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/team/*"] | Some(true)
Declaration { identity: "github.com/team/skip", destination: "/h/repos/team/skip" } | github.com/team/skip | /h/repos/team/skip | Excluded { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/skip.git | NotApplicable | NotApplicable | None | None | ["excluded by github.com/team/*"] | Some(false)
Declaration { identity: "github.com/zzz/occ", destination: "/h/repos/zzz/occ" } | github.com/zzz/occ | /h/repos/zzz/occ | Unregistered | Conflict | git@github.com:zzz/occ.git | NotApplicable | NotApplicable | None | None | [] | Some(false)
Checkout { identity: "github.com/org/d", path: "/h/elsewhere/d" } | github.com/org/d | /h/elsewhere/d | Explicit | Cloned | git@github.com/org/d.git | Known("main") | Clean | Some("/h/elsewhere/d") | Some("github.com/org/d") | ["misplaced checkout; found: /h/elsewhere/d; configured: /h/repos/org/d"] | None
Checkout { identity: "github.com/org/x", path: "/h/repos/org/c" } | github.com/org/x | /h/repos/org/c | Unregistered | Cloned | git@github.com/org/x.git | Known("main") | Clean | Some("/h/repos/org/c") | None | [] | None
Checkout { identity: "github.com/org/y", path: "/h/repos/zzz/occ" } | github.com/org/y | /h/repos/zzz/occ | Unregistered | Cloned | git@github.com/org/y.git | Known("main") | Clean | Some("/h/repos/zzz/occ") | None | [] | None
Checkout { identity: "github.com/other/stale", path: "/h/repos/other/stale" } | github.com/other/stale | /h/repos/other/stale | Unregistered | Unknown | git@github.com/other/stale.git | Stale("moved") | Clean | Some("/h/repos/other/stale") | None | [] | None
Checkout { identity: "github.com/other/u", path: "/h/repos/other/u" } | github.com/other/u | /h/repos/other/u | Unregistered | Cloned | git@github.com/other/u.git | Known("main") | Clean | Some("/h/repos/other/u") | None | [] | Some(false)
Checkout { identity: "github.com/team/m1", path: "/h/repos/team/m1" } | github.com/team/m1 | /h/repos/team/m1 | Wildcard { pattern: "github.com/team/*" } | Cloned | git@github.com/team/m1.git | Known("main") | Clean | Some("/h/repos/team/m1") | None | ["covered by github.com/team/*"] | Some(false)
Path("/h/repos/loose") | git@unused.git | /h/repos/loose | Unregistered | Cloned | git@unused.git | Known("main") | Clean | Some("/h/repos/loose") | None | ["Identity is the exact path"] | None
Path("/h/repos/pending/one") | /h/repos/pending/one | /h/repos/pending/one | Unregistered | Unknown | pending | Pending | Pending | Some("/h/repos/pending/one") | None | [] | None
Path("/h/repos/stale/path") | /h/repos/stale/path | /h/repos/stale/path | Unregistered | Unknown | stale observation | Stale("target changed during observation") | Stale("target changed during observation") | Some("/h/repos/stale/path") | None | ["Target changed before its Git observation completed"] | None
Pattern("github.com/team/*") | github.com/team/* | /h/repos/team | Pattern | Pattern | git@github.com:team/*.git | NotApplicable | NotApplicable | None | Some("github.com/team/*") | ["excludes skip"] | None
"##
        );
    }

    /// Pins the merge where identities collide: duplicate declarations of one identity,
    /// overlapping wildcards, a declaration matched by several checkouts, swapped checkouts at
    /// two declared destinations, repeated pending and stale paths, repeated members with mixed
    /// archived flags and a second catalog merged into the first.
    #[test]
    fn colliding_identities_merge_the_same_way_every_time() {
        let home = PathBuf::from("/h");
        let at = |relative: &str| home.join("repos").join(relative);
        let config = config(&[
            ("github.com/org/a", &[]),
            ("https://github.com/org/a", &[]),
            ("github.com/org/b", &[]),
            ("github.com/org/c", &[]),
            ("github.com/org/e", &[]),
            ("github.com/org/*", &["c"]),
            ("github.com/org/*", &["z"]),
            ("github.com/team/*", &["skip"]),
        ]);
        let mut stale = checkout(at("z/a3"), "github.com/org/a");
        stale.changes = ObservationField::Stale("moved".to_owned());
        let scan = LocalScan {
            checkouts: vec![
                checkout(at("org/a"), "github.com/org/a"),
                checkout(at("x/a1"), "github.com/org/a"),
                checkout(at("y/a2"), "github.com/org/a"),
                stale,
                checkout(at("org/b"), "github.com/org/c"),
                checkout(at("org/c"), "github.com/org/b"),
                checkout(at("org/m1"), "github.com/org/m1"),
                checkout(at("o/occ"), "github.com/other/occ"),
            ],
            pending_paths: vec![at("org/a"), at("p/one"), at("p/one"), at("x/a1")],
            stale_paths: vec![at("s/one"), at("s/one"), at("p/one")],
            complete: false,
            ..Default::default()
        };
        let filesystem = FakeFilesystem(vec![at("org/b"), at("org/c"), at("o/occ")]);
        let mut report =
            build_inventory(&filesystem, &config, Path::new("/c.toml"), &home, scan).unwrap();
        let mut seen = String::new();
        for (members, include_archived) in [
            (
                vec![
                    summary("github.com/org/m1", false),
                    summary("github.com/org/m1", true),
                    summary("github.com/org/m2", true),
                    summary("github.com/org/m2", false),
                    summary("github.com/org/z", false),
                    summary("github.com/org/c", false),
                    summary("github.com/team/skip", false),
                    summary("github.com/team/t1", true),
                    summary("github.com/other/occ", false),
                    summary("github.com/other/free", false),
                ],
                false,
            ),
            (
                vec![
                    summary("github.com/org/m2", false),
                    summary("github.com/team/t1", true),
                    summary("github.com/team/t2", true),
                    summary("github.com/other/free", true),
                    summary("github.com/org/e", false),
                ],
                true,
            ),
        ] {
            add_remote_catalog(
                &mut report,
                &filesystem,
                &config,
                &home,
                members,
                include_archived,
            )
            .unwrap();
            seen.push_str(&render(&report));
            seen.push_str("--\n");
        }
        assert_eq!(
            seen,
            r##"Declaration { identity: "github.com/org/a", destination: "/h/repos/org/a" } | github.com/org/a | /h/repos/org/a | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/org/a") | Some("https://github.com/org/a") | ["covered by github.com/org/*", "covered by github.com/org/*"] | None
Declaration { identity: "github.com/org/b", destination: "/h/repos/org/b" } | github.com/org/b | /h/repos/org/b | Explicit | Conflict | git@github.com:org/b.git | Pending | Pending | None | Some("github.com/org/b") | ["covered by github.com/org/*", "covered by github.com/org/*", "configured destination contains a different checkout"] | None
Declaration { identity: "github.com/org/c", destination: "/h/repos/org/c" } | github.com/org/c | /h/repos/org/c | Explicit | Conflict | git@github.com:org/c.git | Pending | Pending | None | Some("github.com/org/c") | ["excluded by github.com/org/*", "covered by github.com/org/*", "configured destination contains a different checkout"] | Some(false)
Declaration { identity: "github.com/org/e", destination: "/h/repos/org/e" } | github.com/org/e | /h/repos/org/e | Explicit | Missing | git@github.com:org/e.git | Pending | Pending | None | Some("github.com/org/e") | ["covered by github.com/org/*", "covered by github.com/org/*"] | None
Declaration { identity: "github.com/org/m2", destination: "/h/repos/org/m2" } | github.com/org/m2 | /h/repos/org/m2 | Wildcard { pattern: "github.com/org/*" } | RemoteOnly | git@github.com:org/m2.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/org/*", "covered by github.com/org/*"] | Some(false)
Declaration { identity: "github.com/org/z", destination: "/h/repos/org/z" } | github.com/org/z | /h/repos/org/z | Wildcard { pattern: "github.com/org/*" } | RemoteOnly | git@github.com:org/z.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/org/*", "excluded by github.com/org/*"] | Some(false)
Declaration { identity: "github.com/other/free", destination: "/h/repos/other/free" } | github.com/other/free | /h/repos/other/free | Unregistered | RemoteOnly | git@github.com:other/free.git | NotApplicable | NotApplicable | None | None | [] | Some(false)
Declaration { identity: "github.com/team/skip", destination: "/h/repos/team/skip" } | github.com/team/skip | /h/repos/team/skip | Excluded { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/skip.git | NotApplicable | NotApplicable | None | None | ["excluded by github.com/team/*"] | Some(false)
Checkout { identity: "github.com/org/a", path: "/h/repos/x/a1" } | github.com/org/a | /h/repos/x/a1 | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/x/a1") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/x/a1; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/a", path: "/h/repos/y/a2" } | github.com/org/a | /h/repos/y/a2 | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/y/a2") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/y/a2; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/a", path: "/h/repos/z/a3" } | github.com/org/a | /h/repos/z/a3 | Explicit | Unknown | git@github.com/org/a.git | Known("main") | Stale("moved") | Some("/h/repos/z/a3") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/z/a3; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/b", path: "/h/repos/org/c" } | github.com/org/b | /h/repos/org/c | Explicit | Cloned | git@github.com/org/b.git | Known("main") | Clean | Some("/h/repos/org/c") | Some("github.com/org/b") | ["misplaced checkout; found: /h/repos/org/c; configured: /h/repos/org/b", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/c", path: "/h/repos/org/b" } | github.com/org/c | /h/repos/org/b | Explicit | Cloned | git@github.com/org/c.git | Known("main") | Clean | Some("/h/repos/org/b") | Some("github.com/org/c") | ["misplaced checkout; found: /h/repos/org/b; configured: /h/repos/org/c", "excluded by github.com/org/*", "covered by github.com/org/*"] | Some(false)
Checkout { identity: "github.com/org/m1", path: "/h/repos/org/m1" } | github.com/org/m1 | /h/repos/org/m1 | Wildcard { pattern: "github.com/org/*" } | Cloned | git@github.com/org/m1.git | Known("main") | Clean | Some("/h/repos/org/m1") | None | ["covered by github.com/org/*", "covered by github.com/org/*"] | Some(true)
Checkout { identity: "github.com/other/occ", path: "/h/repos/o/occ" } | github.com/other/occ | /h/repos/o/occ | Unregistered | Cloned | git@github.com/other/occ.git | Known("main") | Clean | Some("/h/repos/o/occ") | None | [] | Some(false)
Path("/h/repos/p/one") | /h/repos/p/one | /h/repos/p/one | Unregistered | Unknown | stale observation | Stale("target changed during observation") | Stale("target changed during observation") | Some("/h/repos/p/one") | None | ["Target changed before its Git observation completed"] | None
Path("/h/repos/s/one") | /h/repos/s/one | /h/repos/s/one | Unregistered | Unknown | stale observation | Stale("target changed during observation") | Stale("target changed during observation") | Some("/h/repos/s/one") | None | ["Target changed before its Git observation completed"] | None
Pattern("github.com/org/*") | github.com/org/* | /h/repos/org | Pattern | Pattern | git@github.com:org/*.git | NotApplicable | NotApplicable | None | Some("github.com/org/*") | ["excludes z"] | None
Pattern("github.com/team/*") | github.com/team/* | /h/repos/team | Pattern | Pattern | git@github.com:team/*.git | NotApplicable | NotApplicable | None | Some("github.com/team/*") | ["excludes skip"] | None
--
Declaration { identity: "github.com/org/a", destination: "/h/repos/org/a" } | github.com/org/a | /h/repos/org/a | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/org/a") | Some("https://github.com/org/a") | ["covered by github.com/org/*", "covered by github.com/org/*"] | None
Declaration { identity: "github.com/org/b", destination: "/h/repos/org/b" } | github.com/org/b | /h/repos/org/b | Explicit | Conflict | git@github.com:org/b.git | Pending | Pending | None | Some("github.com/org/b") | ["covered by github.com/org/*", "covered by github.com/org/*", "configured destination contains a different checkout"] | None
Declaration { identity: "github.com/org/c", destination: "/h/repos/org/c" } | github.com/org/c | /h/repos/org/c | Explicit | Conflict | git@github.com:org/c.git | Pending | Pending | None | Some("github.com/org/c") | ["excluded by github.com/org/*", "covered by github.com/org/*", "configured destination contains a different checkout"] | Some(false)
Declaration { identity: "github.com/org/e", destination: "/h/repos/org/e" } | github.com/org/e | /h/repos/org/e | Explicit | Missing | git@github.com:org/e.git | Pending | Pending | None | Some("github.com/org/e") | ["covered by github.com/org/*", "covered by github.com/org/*"] | Some(false)
Declaration { identity: "github.com/org/m2", destination: "/h/repos/org/m2" } | github.com/org/m2 | /h/repos/org/m2 | Wildcard { pattern: "github.com/org/*" } | RemoteOnly | git@github.com:org/m2.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/org/*", "covered by github.com/org/*"] | Some(false)
Declaration { identity: "github.com/org/z", destination: "/h/repos/org/z" } | github.com/org/z | /h/repos/org/z | Wildcard { pattern: "github.com/org/*" } | RemoteOnly | git@github.com:org/z.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/org/*", "excluded by github.com/org/*"] | Some(false)
Declaration { identity: "github.com/other/free", destination: "/h/repos/other/free" } | github.com/other/free | /h/repos/other/free | Unregistered | RemoteOnly | git@github.com:other/free.git | NotApplicable | NotApplicable | None | None | [] | Some(true)
Declaration { identity: "github.com/team/skip", destination: "/h/repos/team/skip" } | github.com/team/skip | /h/repos/team/skip | Excluded { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/skip.git | NotApplicable | NotApplicable | None | None | ["excluded by github.com/team/*"] | Some(false)
Declaration { identity: "github.com/team/t1", destination: "/h/repos/team/t1" } | github.com/team/t1 | /h/repos/team/t1 | Wildcard { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/t1.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/team/*"] | Some(true)
Declaration { identity: "github.com/team/t2", destination: "/h/repos/team/t2" } | github.com/team/t2 | /h/repos/team/t2 | Wildcard { pattern: "github.com/team/*" } | RemoteOnly | git@github.com:team/t2.git | NotApplicable | NotApplicable | None | None | ["covered by github.com/team/*"] | Some(true)
Checkout { identity: "github.com/org/a", path: "/h/repos/x/a1" } | github.com/org/a | /h/repos/x/a1 | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/x/a1") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/x/a1; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/a", path: "/h/repos/y/a2" } | github.com/org/a | /h/repos/y/a2 | Explicit | Cloned | git@github.com/org/a.git | Known("main") | Clean | Some("/h/repos/y/a2") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/y/a2; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/a", path: "/h/repos/z/a3" } | github.com/org/a | /h/repos/z/a3 | Explicit | Unknown | git@github.com/org/a.git | Known("main") | Stale("moved") | Some("/h/repos/z/a3") | Some("github.com/org/a") | ["misplaced checkout; found: /h/repos/z/a3; configured: /h/repos/org/a", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/b", path: "/h/repos/org/c" } | github.com/org/b | /h/repos/org/c | Explicit | Cloned | git@github.com/org/b.git | Known("main") | Clean | Some("/h/repos/org/c") | Some("github.com/org/b") | ["misplaced checkout; found: /h/repos/org/c; configured: /h/repos/org/b", "covered by github.com/org/*", "covered by github.com/org/*"] | None
Checkout { identity: "github.com/org/c", path: "/h/repos/org/b" } | github.com/org/c | /h/repos/org/b | Explicit | Cloned | git@github.com/org/c.git | Known("main") | Clean | Some("/h/repos/org/b") | Some("github.com/org/c") | ["misplaced checkout; found: /h/repos/org/b; configured: /h/repos/org/c", "excluded by github.com/org/*", "covered by github.com/org/*"] | Some(false)
Checkout { identity: "github.com/org/m1", path: "/h/repos/org/m1" } | github.com/org/m1 | /h/repos/org/m1 | Wildcard { pattern: "github.com/org/*" } | Cloned | git@github.com/org/m1.git | Known("main") | Clean | Some("/h/repos/org/m1") | None | ["covered by github.com/org/*", "covered by github.com/org/*"] | Some(true)
Checkout { identity: "github.com/other/occ", path: "/h/repos/o/occ" } | github.com/other/occ | /h/repos/o/occ | Unregistered | Cloned | git@github.com/other/occ.git | Known("main") | Clean | Some("/h/repos/o/occ") | None | [] | Some(false)
Path("/h/repos/p/one") | /h/repos/p/one | /h/repos/p/one | Unregistered | Unknown | stale observation | Stale("target changed during observation") | Stale("target changed during observation") | Some("/h/repos/p/one") | None | ["Target changed before its Git observation completed"] | None
Path("/h/repos/s/one") | /h/repos/s/one | /h/repos/s/one | Unregistered | Unknown | stale observation | Stale("target changed during observation") | Stale("target changed during observation") | Some("/h/repos/s/one") | None | ["Target changed before its Git observation completed"] | None
Pattern("github.com/org/*") | github.com/org/* | /h/repos/org | Pattern | Pattern | git@github.com:org/*.git | NotApplicable | NotApplicable | None | Some("github.com/org/*") | ["excludes z"] | None
Pattern("github.com/team/*") | github.com/team/* | /h/repos/team | Pattern | Pattern | git@github.com:team/*.git | NotApplicable | NotApplicable | None | Some("github.com/team/*") | ["excludes skip"] | None
--
"##
        );
    }
}
