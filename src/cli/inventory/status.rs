use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ratatui::text::Line;
use serde::Serialize;

use super::{HEADINGS, bounded_lookup, home_relative, short_names, table_cells};
use crate::application::inventory::{
    self as model, InventoryRow, ObservationField, OriginFact, RegistrationState, RowKey,
};
use crate::application::ports::ConfigStore;
use crate::application::remote::RemoteBounds;
use crate::cli::args::StatusArgs;
use crate::domain::config::Config;
use crate::infrastructure::config::CommandConfigStore;
use crate::infrastructure::inventory::{LocalInventoryFilesystem, scan_local_repositories};
use crate::infrastructure::providers::ConfiguredProviders;
use crate::presentation::escape;

const SCHEMA_VERSION: u32 = 2;

/// Prints the inventory rows of a finished scan: a table, or one JSON document with `--json`.
pub(crate) fn run(config_path: &Path, args: StatusArgs) -> i32 {
    if args.include_archived && !args.remote {
        eprintln!("lager: --include-archived requires --remote");
        return 2;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let fail = |error: String| {
        eprintln!("lager: {}", escape(error));
        1
    };
    let config = match CommandConfigStore::default().load(config_path) {
        Ok(config) => config,
        Err(error) => return fail(error.to_string()),
    };
    let providers = match ConfiguredProviders::from_config(&config) {
        Ok(providers) => Arc::new(providers),
        Err(error) => return fail(error.to_string()),
    };
    let root = match config.resolve_root(&home) {
        Ok(root) => root,
        Err(error) => return fail(error.to_string()),
    };
    let scan = scan_local_repositories(&root);
    let mut complete = !scan.incomplete;
    let mut problems = scan
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.incomplete)
        .map(|diagnostic| Problem::path("scan", &diagnostic.path, &diagnostic.message))
        .chain(
            scan.failures
                .iter()
                .map(|failure| Problem::path("probe", &failure.path, &failure.message)),
        )
        .collect::<Vec<_>>();
    let notes = scan
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_root_missing())
        .map(|diagnostic| Problem::path("scan", &diagnostic.path, &diagnostic.message))
        .collect::<Vec<_>>();
    let mut report = match model::build_inventory(
        &LocalInventoryFilesystem,
        &config,
        config_path,
        &home,
        scan,
    ) {
        Ok(report) => report,
        Err(error) => return fail(error),
    };
    let mut failed = Vec::new();
    if args.remote {
        match model::add_remote_members(
            &mut report,
            &LocalInventoryFilesystem,
            &config,
            &home,
            &*providers,
            args.include_archived,
        ) {
            Ok(failures) => {
                complete &= failures.is_empty();
                failed = failures
                    .iter()
                    .map(|failure| failure.provider.to_ascii_lowercase())
                    .collect();
                problems.extend(failures.into_iter().map(|failure| Problem {
                    source: "provider",
                    subject: failure.provider,
                    message: failure.error,
                    path: None,
                }));
            }
            Err(error) => return fail(error),
        }
    }
    // A declared repository no catalog listed may still exist, or not be there at all.
    let mut lookups = HashMap::new();
    let ready = |host: &str| !failed.iter().any(|failed| failed == host);
    let bound = RemoteBounds::default().request;
    let deadline = Instant::now() + bound;
    for reference in model::unlisted(&report, &config, args.remote, ready) {
        match bounded_lookup(&providers, &reference, deadline, bound) {
            Ok(state) => {
                lookups.insert(
                    reference.identity(),
                    model::RemoteStatus::looked_up(Ok(state)),
                );
            }
            // With remote discovery off, a failed lookup leaves the origin alone, quietly.
            Err(_) if !args.remote => {}
            Err(error) => {
                complete = false;
                problems.push(Problem {
                    source: "lookup",
                    subject: reference.identity(),
                    message: error,
                    path: None,
                });
            }
        }
    }
    model::apply_lookups(&mut report, &lookups, args.remote);
    // Names come from every row, as on the screen, so a filter never renames a copy.
    let names = short_names(&report.rows);
    let rows = report
        .rows
        .iter()
        .filter(|row| !args.registered || row.registered())
        .collect::<Vec<_>>();
    let stdout = if args.json {
        let document = Document {
            schema_version: SCHEMA_VERSION,
            complete,
            rows: rows.iter().map(|row| JsonRow::new(row, &config)).collect(),
            problems: &problems,
            notes: &notes,
        };
        match serde_json::to_string(&document) {
            Ok(document) => document + "\n",
            Err(error) => return fail(format!("could not render status: {error}")),
        }
    } else {
        // A wildcard is a scan pattern; only `--json` lists it.
        table(
            rows.iter()
                .filter(|row| !matches!(row.key, RowKey::Pattern(_)))
                .map(|row| {
                    table_cells(row, names[&row.key].clone(), &home, false, args.remote)
                        .map(|(cell, _)| cell)
                }),
        )
    };
    let written = std::io::stdout().lock().write_all(stdout.as_bytes());
    for problem in notes.iter().chain(&problems) {
        let subject = match &problem.path {
            Some(path) => home_relative(path, &home),
            None => escape(&problem.subject),
        };
        eprintln!("lager: {subject}: {}", escape(&problem.message));
    }
    if let Err(error) = written {
        return fail(format!("could not write status: {error}"));
    }
    i32::from(!problems.is_empty())
}

/// Aligns the cells under the wide grid's headings, two spaces apart, never truncated.
fn table(rows: impl Iterator<Item = [String; 6]>) -> String {
    let lines = std::iter::once(HEADINGS.map(str::to_owned))
        .chain(rows)
        .collect::<Vec<_>>();
    let width = |cell: &String| Line::raw(cell.as_str()).width();
    let widths = (0..HEADINGS.len())
        .map(|column| lines.iter().map(|line| width(&line[column])).max())
        .collect::<Vec<_>>();
    let mut text = String::new();
    for line in &lines {
        for (column, cell) in line.iter().enumerate() {
            text.push_str(cell);
            if column + 1 < line.len() {
                let pad = widths[column].unwrap_or(0) - width(cell) + 2;
                text.extend(std::iter::repeat_n(' ', pad));
            }
        }
        text.push('\n');
    }
    text
}

#[derive(Serialize)]
struct Document<'a> {
    schema_version: u32,
    complete: bool,
    rows: Vec<JsonRow<'a>>,
    problems: &'a [Problem],
    /// Facts worth seeing that are not failures; present only when there is one.
    #[serde(skip_serializing_if = "<[Problem]>::is_empty")]
    notes: &'a [Problem],
}

#[derive(Serialize)]
struct Problem {
    source: &'static str,
    subject: String,
    message: String,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Problem {
    fn path(source: &'static str, path: &Path, message: &str) -> Self {
        Self {
            source,
            subject: path.to_string_lossy().into_owned(),
            message: message.to_owned(),
            path: Some(path.to_path_buf()),
        }
    }
}

#[derive(Serialize)]
struct JsonRow<'a> {
    kind: &'static str,
    identity: Option<&'a str>,
    declaration: Option<&'a str>,
    hook: Option<&'a str>,
    registration: &'static str,
    pattern: Option<&'a str>,
    exclusions: Option<&'a [String]>,
    checkout: String,
    found_path: Option<String>,
    configured_path: Option<String>,
    origin: Fact,
    branch: Fact,
    changes: Fact,
    archived: Option<bool>,
    /// `public`, `private`, `archived` or `missing`; `None` while the provider has not said.
    remote: Option<&'static str>,
    warnings: &'a [String],
}

/// An observed or declared value with the state that says how far it is known.
#[derive(Serialize)]
struct Fact {
    state: &'static str,
    value: Option<String>,
}

impl Fact {
    fn new(state: &'static str, value: Option<&str>) -> Self {
        Self {
            state,
            value: value.map(str::to_owned),
        }
    }
}

impl<'a> JsonRow<'a> {
    fn new(row: &'a InventoryRow, config: &'a Config) -> Self {
        let declared = row.declaration.as_deref().and_then(|url| {
            config
                .repositories
                .iter()
                .find(|declaration| declaration.url == url)
        });
        let (kind, identity) = match &row.key {
            RowKey::Declaration { identity, .. } => ("declaration", Some(identity.as_str())),
            RowKey::Checkout { identity, .. } => ("checkout", Some(identity.as_str())),
            RowKey::Path(_) => ("path", None),
            RowKey::Pattern(pattern) => ("pattern", Some(pattern.as_str())),
        };
        let pattern = match &row.registration {
            RegistrationState::Wildcard { pattern } | RegistrationState::Excluded { pattern } => {
                Some(pattern.as_str())
            }
            _ => None,
        };
        let lossy = |path: &Option<PathBuf>| {
            path.as_ref()
                .map(|path| path.to_string_lossy().into_owned())
        };
        let origin = match &row.origin_fact {
            Some(OriginFact::Absent) => Fact::new("absent", None),
            Some(OriginFact::Supported { original, .. }) => Fact::new("supported", Some(original)),
            Some(OriginFact::Unsupported(original)) => Fact::new("unsupported", Some(original)),
            Some(OriginFact::Error(error)) => Fact::new("error", Some(error)),
            None if row.observed_path.is_some() => Fact::new("pending", None),
            None if matches!(row.key, RowKey::Pattern(_)) => Fact::new("not_applicable", None),
            None => Fact::new(
                "declared",
                Some(row.remote.as_deref().unwrap_or(&row.origin)),
            ),
        };
        Self {
            kind,
            identity,
            declaration: row.declaration.as_deref(),
            hook: declared.and_then(|declaration| declaration.post_clone.as_deref()),
            registration: row.registration.label(),
            pattern,
            exclusions: declared
                .filter(|_| kind == "pattern")
                .map(|declaration| declaration.exclude.as_slice()),
            checkout: row.checkout.label().replace(' ', "_"),
            found_path: lossy(&row.observed_path),
            configured_path: lossy(&row.configured_destination),
            origin,
            branch: observation(&row.branch),
            changes: observation(&row.changes),
            archived: row.archived,
            remote: row
                .remote_status
                .filter(|status| *status != model::RemoteStatus::Pending)
                .map(model::RemoteStatus::label),
            warnings: &row.warnings,
        }
    }
}

fn observation(field: &ObservationField) -> Fact {
    match field {
        ObservationField::Pending => Fact::new("pending", None),
        ObservationField::Known(value) => Fact::new("known", Some(value)),
        ObservationField::Clean => Fact::new("clean", None),
        ObservationField::Dirty(value) => Fact::new("dirty", Some(value)),
        ObservationField::Error(value) => Fact::new("error", Some(value)),
        ObservationField::Stale(value) => Fact::new("stale", Some(value)),
        ObservationField::NotApplicable => Fact::new("not_applicable", None),
    }
}
