use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

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
    pub branch: ObservationField,
    pub changes: ObservationField,
    pub configured_destination: Option<PathBuf>,
    pub observed_path: Option<PathBuf>,
    pub declaration: Option<String>,
    pub warnings: Vec<String>,
    pub markable: bool,
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

    let explicit_identities: HashSet<String> = concrete
        .iter()
        .map(|declaration| declaration.reference.identity())
        .collect();
    let mut rows_by_key = BTreeMap::new();

    for declaration in &concrete {
        let identity = declaration.reference.identity();
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
                warnings: Vec::new(),
                markable: true,
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
                branch: ObservationField::Pending,
                changes: ObservationField::Pending,
                configured_destination: None,
                observed_path: Some(pending.clone()),
                declaration: None,
                warnings: Vec::new(),
                markable: true,
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
                branch: ObservationField::Stale("target changed during observation".to_owned()),
                changes: ObservationField::Stale("target changed during observation".to_owned()),
                configured_destination: None,
                observed_path: Some(stale.clone()),
                declaration: None,
                warnings: vec!["Target changed before its Git observation completed".to_owned()],
                markable: true,
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
                    branch: checkout.branch,
                    changes: checkout.changes,
                    configured_destination,
                    observed_path: Some(observed),
                    declaration: matching_declarations
                        .first()
                        .map(|declaration| declaration.declaration.url.clone()),
                    warnings,
                    markable: true,
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
                    branch: checkout.branch,
                    changes: checkout.changes,
                    configured_destination: None,
                    observed_path: Some(observed),
                    declaration: None,
                    warnings,
                    markable: true,
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

fn declaration_key(identity: &str, destination: &Path) -> RowKey {
    RowKey::Declaration {
        identity: identity.to_owned(),
        destination: destination.to_path_buf(),
    }
}

fn configured_destination(config: &Config, reference: &RepositoryRef, root: &Path) -> PathBuf {
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

fn wildcard_relative<'a>(pattern: &RepositoryRef, identity: &'a str) -> Option<&'a str> {
    identity
        .strip_prefix(&(pattern.identity() + "/"))
        .filter(|relative| !relative.is_empty())
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
}
