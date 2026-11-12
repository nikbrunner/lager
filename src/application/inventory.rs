use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::application::ports::{ProviderCatalog, ProviderFailure};
use crate::domain::config::{Config, RepositoryDeclaration};
use crate::domain::repository::RepositoryRef;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanDiagnostic {
    pub path: PathBuf,
    pub message: String,
    /// Exclusions are useful to show but do not mean traversal could not finish.
    pub incomplete: bool,
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

    for declaration in &concrete {
        let identity = declaration.reference.identity();
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
            },
        );
    }

    for pending in &scan.pending_paths {
        if scan
            .checkouts
            .iter()
            .any(|checkout| checkout.path == *pending)
        {
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
            },
        );
    }

    for checkout in scan.checkouts {
        let observed = checkout.path.clone();
        let stale = matches!(&checkout.branch, ObservationField::Stale(_))
            || matches!(&checkout.changes, ObservationField::Stale(_));
        if let Some(identity) = checkout.identity.clone() {
            let exact = concrete.iter().find(|declaration| {
                declaration.reference.identity() == identity && declaration.destination == observed
            });
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

            let matching_declarations: Vec<_> = concrete
                .iter()
                .filter(|declaration| declaration.reference.identity() == identity)
                .collect();
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
                },
            );
        }
    }

    // A real checkout at another declared destination is a conflict, not a missing clone target.
    for declaration in &concrete {
        let key = declaration_key(&declaration.reference.identity(), &declaration.destination);
        let has_conflicting_checkout = rows_by_key.values().any(|row| {
            row.observed_path.as_ref() == Some(&declaration.destination)
                && row.checkout == CheckoutState::Cloned
                && matches!(&row.key, RowKey::Checkout { identity, .. } if *identity != declaration.reference.identity())
        });
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
        let mut members = match providers.expand(&wildcard.reference, include_archived) {
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
        members.sort_by_key(|member| member.reference.identity());
        for member in members {
            let identity = member.reference.identity();
            let mut known = false;
            for row in &mut report.rows {
                if row_identity(row) == Some(identity.as_str()) {
                    row.archived = Some(member.archived);
                    known = true;
                }
            }
            if known || member.archived && !include_archived {
                continue;
            }
            let (registration, relationships) = registration_for(&identity, &wildcards, &explicit);
            if !matches!(registration, RegistrationState::Wildcard { .. }) {
                continue;
            }
            let destination = configured_destination(config, &member.reference, &root);
            let occupied = report.rows.iter().any(|row| {
                row.observed_path.as_ref() == Some(&destination)
                    && row.checkout == CheckoutState::Cloned
            });
            let checkout = match filesystem.destination_state(&destination) {
                CheckoutState::Missing => CheckoutState::RemoteOnly,
                _ if occupied => CheckoutState::Conflict,
                state => state,
            };
            let key = declaration_key(&identity, &destination);
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
            });
        }
    }
    report.rows.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(failures)
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
}
