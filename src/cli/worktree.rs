//! `lager worktree`: arguments in, service calls, text out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::args::{
    WorktreeAddArgs, WorktreeBranchesArgs, WorktreeCommand, WorktreeListArgs, WorktreeRemoveArgs,
};
use super::interaction::{Interaction, InteractionError};
use super::session;
use crate::application::ports::{Recorded, WorktreeGit, WorktreeLosses};
use crate::application::sessions::{self, Ensured};
use crate::application::worktrees::{
    self, AddError, AddPlan, AddRequest, Blocker, Branch, BranchChoice, BranchListing, Placement,
    RemovalInspection, RemoveError, Repository, WorktreeListing, short_id,
};
use crate::domain::config::{Config, WorktreeTemplate};
use crate::domain::repository::{RepositoryRef, validate_reference_safety};
use crate::domain::session::Layout;
use crate::infrastructure::{
    filesystem::Filesystem, git::NativeGit, rex::Rex, session_record::SessionFile,
};
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) fn error(message: impl std::fmt::Display) {
    eprintln!("error: {}", escape(message));
}

pub(super) struct Settings {
    pub(super) home: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) config: Option<Config>,
    template: Option<WorktreeTemplate>,
}

impl Settings {
    /// Reads the configuration when there is one; worktree commands run without it.
    pub(super) fn load(config_path: &Path) -> Result<Self, i32> {
        let fail = |message: String| {
            error(message);
            1
        };
        let cwd = std::env::current_dir()
            .map_err(|source| fail(format!("cannot read the working directory: {source}")))?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        // Git reports real paths, so placement compares against the real HOME.
        let home = std::fs::canonicalize(&home).unwrap_or(home);
        let mut settings = Self {
            home,
            cwd,
            config: None,
            template: None,
        };
        if std::fs::symlink_metadata(config_path).is_ok() {
            let (config, template, warnings) =
                crate::infrastructure::config::load_worktrees(config_path)
                    .map_err(|source| fail(source.to_string()))?;
            for warning in warnings {
                eprintln!("lager: warning: {warning}");
            }
            settings.template = template;
            settings.config = Some(config);
        }
        Ok(settings)
    }

    /// A path is absolute or starts with `.` or `..`; anything else is a repository reference.
    pub(super) fn repository(&self, argument: &str) -> Result<Repository, String> {
        let explicit = Path::new(argument);
        let path = if is_path(argument) {
            worktrees::absolute(&self.cwd, explicit)
        } else {
            validate_reference_safety(argument).map_err(|source| source.to_string())?;
            let reference = RepositoryRef::parse(argument).map_err(|source| source.to_string())?;
            if reference.is_wildcard() {
                return Err("a wildcard does not name one repository".to_owned());
            }
            let config = self.config.as_ref().ok_or_else(|| {
                "a repository reference needs a configuration; pass a checkout path".to_owned()
            })?;
            crate::application::warehouse::destination(config, &reference, &self.home)?
        };
        worktrees::resolve_repository(&GIT, &FILES, &path, true)
    }
}

/// A path is absolute or starts with `.` or `..`; anything else is a repository reference.
pub(super) fn is_path(argument: &str) -> bool {
    Path::new(argument).is_absolute()
        || matches!(argument, "." | "..")
        || argument.starts_with("./")
        || argument.starts_with("../")
}

pub(super) fn run(
    config_path: &Path,
    command: WorktreeCommand,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    // What a request states on its own is checked before the configuration is read.
    let usage = match &command {
        WorktreeCommand::Add(args) => worktrees::check_request(&GIT, &add_request(args)).err(),
        WorktreeCommand::Remove(args) if !interactive && !args.yes => {
            Some("worktree remove requires --yes outside a terminal".to_owned())
        }
        _ => None,
    };
    if let Some(message) = usage {
        error(message);
        return 2;
    }
    let settings = match Settings::load(config_path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let layout = match &command {
        WorktreeCommand::List(_) => session::layout(config_path).ok(),
        WorktreeCommand::Branches(_) => None,
        WorktreeCommand::Add(WorktreeAddArgs {
            open_session: false,
            create_session: false,
            ..
        })
        | WorktreeCommand::Remove(WorktreeRemoveArgs {
            close_session: false,
            ..
        }) => None,
        _ => match session::layout(config_path) {
            Ok(layout) => Some(layout),
            Err(message) => {
                error(message);
                return 1;
            }
        },
    };
    match command {
        WorktreeCommand::List(args) => list(&settings, args, layout.is_some()),
        WorktreeCommand::Branches(args) => branches(&settings, args),
        WorktreeCommand::Add(args) => add(&settings, args, layout.as_ref()),
        WorktreeCommand::Remove(args) => remove(&settings, args, interaction),
    }
}

fn add_request(args: &WorktreeAddArgs) -> AddRequest<'_> {
    AddRequest {
        name: args.name.as_deref(),
        branch: match (&args.branch, &args.new_branch) {
            (Some(branch), _) => BranchChoice::Existing(branch),
            (None, Some(branch)) => BranchChoice::New(branch),
            (None, None) => BranchChoice::Named,
        },
        start: args.start.as_deref(),
        path: None,
    }
}

/// The named repository, or the one of the working directory. Fails with the exit code
/// after printing why: `2` for a working directory outside a repository, `1` otherwise.
fn target(settings: &Settings, argument: Option<&str>) -> Result<Repository, i32> {
    let repository = match argument {
        Some(argument) => settings.repository(argument),
        None => worktrees::resolve_repository(&GIT, &FILES, &settings.cwd, false),
    };
    repository.map_err(|message| {
        if argument.is_none() {
            error(format!("{message}; name a repository"));
            2
        } else {
            error(message);
            1
        }
    })
}

fn list(settings: &Settings, args: WorktreeListArgs, configured: bool) -> i32 {
    let repository = match target(settings, args.repository.as_deref()) {
        Ok(repository) => repository,
        Err(code) => return code,
    };
    let listing = worktrees::list(&GIT, &FILES, &repository);
    let hosted = configured.then(|| {
        sessions::list(&Rex, &SessionFile)
            .map_err(|failure| eprintln!("lager: warning: {}", escape(session::reason(failure))))
    });
    let sessions = hosted.as_ref().map(|hosted| {
        listing
            .worktrees
            .iter()
            .map(|record| match hosted {
                Ok(hosted) => Hosted::Known(hosted.get(&session::key(&record.path))),
                Err(()) => Hosted::Unknown("unknown"),
            })
            .collect::<Vec<_>>()
    });
    let stdout = if args.json {
        match serde_json::to_string_pretty(&Document::new(&listing, sessions.as_deref())) {
            Ok(document) => document + "\n",
            Err(source) => {
                error(format!("could not render the worktree list: {source}"));
                return 1;
            }
        }
    } else {
        table(&listing, sessions.as_deref())
    };
    let written = std::io::stdout().lock().write_all(stdout.as_bytes());
    for fault in &listing.errors {
        match &fault.path {
            Some(path) => error(format!("{}: {}", path.display(), fault.error)),
            None => error(&fault.error),
        }
    }
    if let Err(source) = written {
        error(format!("could not write the worktree list: {source}"));
        return 1;
    }
    i32::from(!listing.errors.is_empty())
}

#[derive(Serialize)]
struct Document<'a> {
    schema_version: u8,
    worktrees: Vec<Record<'a>>,
    errors: Vec<Fault>,
}

#[derive(Serialize)]
struct Record<'a> {
    repository_identity: Option<String>,
    primary_path: String,
    path: String,
    is_primary: bool,
    branch: Option<String>,
    head: Option<String>,
    locked: bool,
    lock_reason: Option<String>,
    prunable: bool,
    prune_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<Hosted<'a>>,
}

/// A worktree's session as the host reports it: the record, or `null` for none; `unknown`
/// when the host could not answer.
#[derive(Clone, Copy, Serialize)]
#[serde(untagged)]
enum Hosted<'a> {
    Known(Option<&'a Recorded>),
    Unknown(&'static str),
}

#[derive(Serialize)]
struct Fault {
    path: Option<String>,
    code: &'static str,
    error: String,
}

impl<'a> Document<'a> {
    /// Schema version 2 adds `session` to every record.
    fn new(listing: &WorktreeListing, sessions: Option<&[Hosted<'a>]>) -> Self {
        let text = |path: &Path| path.to_string_lossy().into_owned();
        Self {
            schema_version: if sessions.is_some() { 2 } else { 1 },
            worktrees: listing
                .worktrees
                .iter()
                .enumerate()
                .map(|(index, record)| Record {
                    repository_identity: record.repository_identity.clone(),
                    primary_path: text(&record.primary_path),
                    path: text(&record.path),
                    is_primary: record.is_primary,
                    branch: record.branch.clone(),
                    head: record.head.clone(),
                    locked: record.locked,
                    lock_reason: record.lock_reason.clone(),
                    prunable: record.prunable,
                    prune_reason: record.prune_reason.clone(),
                    session: sessions.map(|sessions| sessions[index]),
                })
                .collect(),
            errors: listing
                .errors
                .iter()
                .map(|fault| Fault {
                    path: fault.path.as_deref().map(text),
                    code: fault.code,
                    error: fault.error.clone(),
                })
                .collect(),
        }
    }
}

pub(super) fn branch_label(branch: Option<&str>, head: Option<&str>) -> String {
    match (branch, head) {
        (Some(branch), _) => branch
            .strip_prefix("refs/heads/")
            .unwrap_or(branch)
            .to_owned(),
        (None, Some(head)) => format!("detached {}", short_id(head)),
        (None, None) => "unknown".to_owned(),
    }
}

/// Path, branch and state, and with sessions configured the session's label, two spaces apart.
fn table(listing: &WorktreeListing, sessions: Option<&[Hosted]>) -> String {
    let rows = listing
        .worktrees
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let reason = |label: &str, reason: &Option<String>| match reason {
                Some(reason) if !reason.is_empty() => format!("{label}: {reason}"),
                _ => label.to_owned(),
            };
            let mut states = Vec::new();
            if record.is_primary {
                states.push("primary".to_owned());
            }
            if record.locked {
                states.push(reason("locked", &record.lock_reason));
            }
            if record.prunable {
                states.push(reason("prunable", &record.prune_reason));
            }
            let mut row = vec![
                escape(record.path.display()),
                escape(branch_label(
                    record.branch.as_deref(),
                    record.head.as_deref(),
                )),
                escape(states.join(", ")),
            ];
            row.extend(sessions.map(|sessions| match sessions[index] {
                Hosted::Known(Some(recorded)) => escape(&recorded.label),
                Hosted::Known(None) => String::new(),
                Hosted::Unknown(unknown) => unknown.to_owned(),
            }));
            row
        })
        .collect::<Vec<_>>();
    columns(&rows)
}

/// One line per row, every cell but the last padded to its column's width and two spaces
/// apart. The rows have equally many cells.
fn columns(rows: &[Vec<String>]) -> String {
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths = (0..rows.first().map_or(0, |row| row.len() - 1))
        .map(width)
        .collect::<Vec<_>>();
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(column, cell)| match widths.get(column) {
                    Some(width) => format!("{cell:width$}  "),
                    None => cell.clone(),
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
                + "\n"
        })
        .collect()
}

/// How long ago `committed` was, in its largest whole unit.
pub(super) fn age(committed: i64, now: i64) -> String {
    let seconds = (now - committed).max(0);
    let (minute, hour, day) = (60, 3600, 86_400);
    match seconds {
        seconds if seconds < minute => "now".to_owned(),
        seconds if seconds < hour => format!("{}m", seconds / minute),
        seconds if seconds < day => format!("{}h", seconds / hour),
        seconds if seconds < 7 * day => format!("{}d", seconds / day),
        seconds if seconds < 56 * day => format!("{}w", seconds / (7 * day)),
        seconds if seconds < 365 * day => format!("{}mo", seconds / (30 * day)),
        seconds => format!("{}y", seconds / (365 * day)),
    }
}

/// How far a local branch is from its remote-tracking branch: commits only on the local
/// branch, then commits only on the remote one; `None` when Git could not count them.
pub(super) fn distance(ahead_behind: Option<(usize, usize)>) -> String {
    match ahead_behind {
        None => "distance unknown".to_owned(),
        Some((0, 0)) => "same commit".to_owned(),
        Some((ahead, 0)) => format!("{ahead} ahead"),
        Some((0, behind)) => format!("{behind} behind"),
        Some((ahead, behind)) => format!("{ahead} ahead, {behind} behind"),
    }
}

pub(super) fn seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// `seconds` since the Unix epoch as an RFC 3339 timestamp in UTC, such as
/// `2023-11-14T22:13:20Z`.
fn rfc3339(seconds: i64) -> String {
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil date from days since 1970-01-01, counting years from March.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// The local branches or, with `--remote`, the remote-tracking branches, newest commit first.
fn branches(settings: &Settings, args: WorktreeBranchesArgs) -> i32 {
    let repository = match target(settings, args.repository.as_deref()) {
        Ok(repository) => repository,
        Err(code) => return code,
    };
    let last_fetch = GIT
        .last_fetch(&repository.common_dir)
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| rfc3339(elapsed.as_secs() as i64));
    let listing = worktrees::branches(&GIT, &repository);
    let empty = BranchListing::default();
    let found = listing.as_ref().unwrap_or(&empty);
    let rows = if args.remote {
        &found.remote
    } else {
        &found.local
    };
    let stdout = if args.json {
        let document = BranchesDocument {
            schema_version: 1,
            branches: rows
                .iter()
                .map(|branch| BranchRecord::new(branch, found))
                .collect(),
            last_fetch,
            errors: listing
                .as_ref()
                .err()
                .map(|message| Fault {
                    path: Some(repository.primary.to_string_lossy().into_owned()),
                    code: "git_failed",
                    error: message.clone(),
                })
                .into_iter()
                .collect(),
        };
        match serde_json::to_string_pretty(&document) {
            Ok(document) => document + "\n",
            Err(source) => {
                error(format!("could not render the branches: {source}"));
                return 1;
            }
        }
    } else {
        branch_table(rows, found, &repository, args.remote)
    };
    let written = std::io::stdout().lock().write_all(stdout.as_bytes());
    if let Err(message) = &listing {
        error(format!("{}: {message}", repository.primary.display()));
    }
    if let Err(source) = written {
        error(format!("could not write the branches: {source}"));
        return 1;
    }
    i32::from(listing.is_err())
}

#[derive(Serialize)]
struct BranchesDocument<'a> {
    schema_version: u8,
    branches: Vec<BranchRecord<'a>>,
    last_fetch: Option<String>,
    errors: Vec<Fault>,
}

#[derive(Serialize)]
struct BranchRecord<'a> {
    name: &'a str,
    remote: Option<&'a str>,
    commit: &'a str,
    committed_at: String,
    checked_out: Option<String>,
    local: Option<&'a str>,
    ahead: Option<usize>,
    behind: Option<usize>,
}

impl<'a> BranchRecord<'a> {
    fn new(branch: &'a Branch, listing: &'a BranchListing) -> Self {
        let counts = branch.local.as_ref().and_then(|local| local.ahead_behind);
        Self {
            name: &branch.name,
            remote: branch.remote.as_deref(),
            commit: &branch.commit,
            committed_at: rfc3339(branch.committed),
            checked_out: listing
                .checked_out(branch)
                .map(|path| path.to_string_lossy().into_owned()),
            local: branch.local.as_ref().map(|local| local.name.as_str()),
            ahead: counts.map(|(ahead, _)| ahead),
            behind: counts.map(|(_, behind)| behind),
        }
    }
}

/// A heading and one line per branch: name, short commit, age, and for a local branch where
/// it is checked out, for a remote one the local branch with its distance and checkout.
fn branch_table(
    rows: &[Branch],
    listing: &BranchListing,
    repository: &Repository,
    remote: bool,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let now = seconds_now();
    let mut table = vec![
        [
            "branch",
            "commit",
            "age",
            if remote { "local" } else { "checked out" },
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    table.extend(rows.iter().map(|branch| {
        let checked_out = listing.checked_out(branch);
        let primary = checked_out == Some(repository.primary.as_path());
        let place = checked_out.map(|path| escape(path.display()));
        let last = match &branch.local {
            None if primary => "primary".to_owned(),
            None => place.unwrap_or_default(),
            Some(local) => {
                let mut text =
                    format!("{} · {}", escape(&local.name), distance(local.ahead_behind));
                match place {
                    Some(_) if primary => text.push_str(" · primary"),
                    Some(place) => text.push_str(&format!(" · in {place}")),
                    None => {}
                }
                text
            }
        };
        vec![
            escape(&branch.name),
            short_id(&branch.commit).to_owned(),
            age(branch.committed, now),
            last,
        ]
    }));
    columns(&table)
}

fn add(settings: &Settings, args: WorktreeAddArgs, layout: Option<&Layout>) -> i32 {
    let request = add_request(&args);
    let repository = match settings.repository(&args.repository) {
        Ok(repository) => repository,
        Err(message) => {
            error(message);
            return 1;
        }
    };
    if args.fetch {
        match worktrees::fetch_remotes(&GIT, &repository, false) {
            Ok(failures) => {
                for (remote, reason) in failures {
                    eprintln!(
                        "lager: warning: fetch {} failed: {}",
                        escape(remote),
                        escape(reason)
                    );
                }
            }
            Err(message) => {
                error(message);
                return 1;
            }
        }
    }
    let path = args
        .path
        .as_deref()
        .map(|path| worktrees::absolute(&settings.cwd, path));
    let root = settings
        .config
        .as_ref()
        .and_then(|config| config.resolve_root(&settings.home).ok());
    let plan = worktrees::plan_add(
        &GIT,
        &FILES,
        &repository,
        &AddRequest {
            path: path.as_deref(),
            ..request
        },
        &Placement {
            home: &settings.home,
            root: root.as_deref(),
            template: settings.template.as_ref(),
        },
    );
    let plan = match plan {
        Ok(plan) => plan,
        Err(AddError::Usage(message)) => {
            error(message);
            return 2;
        }
        Err(AddError::Refused(message)) => {
            error(message);
            return 1;
        }
    };
    let added = if args.json {
        crate::infrastructure::git::stdout_to_stderr(|| worktrees::add(&GIT, &FILES, &plan))
    } else {
        worktrees::add(&GIT, &FILES, &plan)
    };
    if let Err(failure) = added {
        error(format!("worktree was not created: {}", failure.message));
        if failure.remaining.is_empty() {
            eprintln!("remaining: nothing");
        }
        for remaining in &failure.remaining {
            eprintln!("remaining: {}", escape(remaining));
        }
        return if failure.cancelled { 130 } else { 1 };
    }
    if !args.json {
        println!(
            "{}",
            created_line(&escape(plan.destination.display()), &plan)
        );
    }
    let ensured = match layout.map(|layout| add_session(&plan, layout, args.open_session)) {
        None => None,
        Some(Ok(ensured)) => Some(ensured),
        Some(Err(reason)) => {
            error(format!("worktree created, session not opened: {reason}"));
            return 1;
        }
    };
    if args.json {
        let document = Added {
            schema_version: 2,
            path: plan.destination.to_string_lossy().into_owned(),
            branch: &plan.branch,
            new_branch: plan.start.is_some(),
            upstream: plan
                .upstream
                .as_ref()
                .map(|upstream| upstream.name.as_str()),
            ahead: plan.upstream.as_ref().map(|upstream| upstream.ahead),
            behind: plan.upstream.as_ref().map(|upstream| upstream.behind),
            session: ensured.as_ref().map(|ensured| AddedSession {
                created: matches!(ensured, Ensured::Created(_)),
                recorded: ensured.recorded(),
            }),
        };
        let written = serde_json::to_string_pretty(&document)
            .map_err(|source| format!("could not render the worktree: {source}"))
            .and_then(|document| {
                std::io::stdout()
                    .lock()
                    .write_all((document + "\n").as_bytes())
                    .map_err(|source| format!("could not write the worktree: {source}"))
            });
        if let Err(message) = written {
            error(message);
            return 1;
        }
    } else if let Some(ensured) = &ensured {
        let label = escape(&ensured.recorded().label);
        match ensured {
            _ if args.open_session => println!("Opened session {label}"),
            Ensured::Created(_) => println!("Created session {label}"),
            Ensured::Existing(_) => println!("Session {label} already exists"),
        }
    }
    0
}

/// Creates the new worktree's session, keyed by its real path, and shows it when `open`.
fn add_session(plan: &AddPlan, layout: &Layout, open: bool) -> Result<Ensured, String> {
    let checkout = GIT
        .locate(&plan.destination)
        .map_err(|_| format!("not a Git checkout: {}", plan.destination.display()))?
        .top_level;
    let label = session::label(&plan.repository.primary, &checkout);
    let (host, records) = (Rex, SessionFile);
    if open {
        sessions::open(&host, &records, &checkout, &label, layout)
    } else {
        sessions::ensure(&host, &records, &checkout, &label, layout)
    }
    .map_err(session::reason)
}

#[derive(Serialize)]
struct Added<'a> {
    schema_version: u8,
    path: String,
    branch: &'a str,
    new_branch: bool,
    upstream: Option<&'a str>,
    ahead: Option<usize>,
    behind: Option<usize>,
    session: Option<AddedSession<'a>>,
}

/// `Created worktree DESTINATION on branch B`, then what the branch follows: a new branch
/// tracks its remote branch, an existing one says how far it is from its remote branch.
pub(super) fn created_line(destination: &str, plan: &AddPlan) -> String {
    let mut line = format!(
        "Created worktree {destination} on {}branch {}",
        if plan.start.is_some() { "new " } else { "" },
        escape(&plan.branch)
    );
    if let Some(clause) = upstream_clause(plan) {
        line += &format!(", {clause}");
    }
    line
}

/// `tracking origin/x` for a new branch, `2 behind origin/x` for an existing one; `None`
/// without an upstream.
pub(super) fn upstream_clause(plan: &AddPlan) -> Option<String> {
    let upstream = plan.upstream.as_ref()?;
    let name = escape(&upstream.name);
    Some(match (plan.tracks(), upstream.ahead, upstream.behind) {
        (true, _, _) => format!("tracking {name}"),
        (false, 0, 0) => format!("up to date with {name}"),
        (false, ahead, 0) => format!("{ahead} ahead of {name}"),
        (false, 0, behind) => format!("{behind} behind {name}"),
        (false, ahead, behind) => format!("{ahead} ahead, {behind} behind {name}"),
    })
}

#[derive(Serialize)]
struct AddedSession<'a> {
    created: bool,
    #[serde(flatten)]
    recorded: &'a Recorded,
}

fn print_blockers(blockers: &[Blocker]) {
    for blocker in blockers {
        error(format!("blocked: {}", blocker.message));
        if let Some(hint) = &blocker.hint {
            eprintln!("hint: {}", escape(hint));
        }
    }
}

/// Everything the removal deletes, one loss per line: the files, then each named state.
pub(super) fn removal_losses(inspection: &RemovalInspection) -> Vec<String> {
    let files = (!inspection.losses.is_empty() || inspection.git_state.is_empty())
        .then(|| losses_label(&inspection.losses));
    files
        .into_iter()
        .chain(inspection.git_state.iter().map(escape))
        .collect()
}

pub(super) fn losses_label(losses: &WorktreeLosses) -> String {
    let count = |count: usize, noun: &str| {
        (count > 0).then(|| format!("{count} {noun} file{}", if count == 1 { "" } else { "s" }))
    };
    let parts = [
        count(losses.modified, "modified"),
        count(losses.untracked, "untracked"),
        losses.ignored.then(|| "ignored files".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// What stays after the removal: the branch, or the ref that holds a detached commit.
pub(super) fn kept_label(inspection: &RemovalInspection) -> String {
    let label = branch_label(inspection.branch.as_deref(), inspection.head.as_deref());
    match (&inspection.branch, &inspection.protected_by) {
        (Some(_), _) => match inspection.unpushed {
            Some(count) if count > 0 => format!(
                "{label} · kept · {count} commit{} not pushed",
                if count == 1 { "" } else { "s" }
            ),
            _ => format!("{label} · kept"),
        },
        (None, Some(protecting)) => format!("{label} · kept by {protecting}"),
        (None, None) => label,
    }
}

/// Why the removal stopped: "local changes" only when the losses are files alone.
fn losses_refused(inspection: &RemovalInspection) -> String {
    format!(
        "{} would be lost; rerun with --force to accept these losses",
        if inspection.git_state.is_empty() {
            "local changes"
        } else {
            "the listed state"
        }
    )
}

fn remove(settings: &Settings, args: WorktreeRemoveArgs, interaction: &mut dyn Interaction) -> i32 {
    let context = Some(settings.cwd.as_path());
    let path = worktrees::absolute(&settings.cwd, &args.path);
    let inspection = worktrees::inspect_removal(&GIT, &FILES, &path, context, None);
    if !inspection.blockers.is_empty() {
        print_blockers(&inspection.blockers);
        return 1;
    }
    let shown = escape(inspection.path.display());
    println!("Remove worktree");
    for (key, value) in [
        ("path", shown.clone()),
        ("branch", escape(kept_label(&inspection))),
    ] {
        println!("  {key:<10}{value}");
    }
    for (index, loss) in removal_losses(&inspection).iter().enumerate() {
        println!("  {:<10}{loss}", if index == 0 { "losses" } else { "" });
    }
    if inspection.force_helps() && !args.force {
        error(losses_refused(&inspection));
        return 1;
    }
    if !args.yes {
        let question = if !inspection.lossy() {
            "Remove this worktree?"
        } else {
            "Accept these losses and remove this worktree?"
        };
        match interaction.confirm(question, false) {
            Ok(true) => {}
            Ok(false) => {
                println!("Skipped {shown}");
                return 0;
            }
            Err(InteractionError::Cancelled) => return 130,
            Err(InteractionError::Failed(message)) => {
                error(message);
                return 1;
            }
        }
    }
    match worktrees::remove(&GIT, &FILES, &inspection, args.force, context) {
        Ok(()) => {
            let kept = match (&inspection.branch, &inspection.protected_by) {
                (Some(_), _) => format!(
                    "branch {} kept",
                    branch_label(inspection.branch.as_deref(), None)
                ),
                _ => kept_label(&inspection),
            };
            println!("Removed worktree {shown} · {}", escape(kept));
            if !args.close_session {
                return 0;
            }
            match sessions::close(&Rex, &SessionFile, &inspection.path) {
                Ok(Some(recorded)) => println!("Closed session {}", escape(&recorded.label)),
                Ok(None) => println!("No session for {shown}"),
                Err(failure) => {
                    error(format!(
                        "worktree removed, session not closed: {}",
                        session::reason(failure)
                    ));
                    return 1;
                }
            }
            0
        }
        Err(RemoveError::Stale) => {
            error("the worktree changed after it was inspected; nothing removed");
            1
        }
        Err(RemoveError::Blocked(blockers)) => {
            print_blockers(&blockers);
            1
        }
        Err(RemoveError::Losses) => {
            error(losses_refused(&inspection));
            1
        }
        Err(RemoveError::Failed(failure)) => {
            error(format!("git worktree remove failed: {}", failure.message));
            if failure.cancelled { 130 } else { 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn rfc3339_writes_utc_around_the_epoch_and_a_non_leap_century() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59Z");
        assert_eq!(rfc3339(4_107_542_400), "2100-03-01T00:00:00Z");
    }
}
