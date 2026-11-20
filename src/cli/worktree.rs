//! `lager worktree`: arguments in, service calls, text out.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::args::{WorktreeAddArgs, WorktreeCommand, WorktreeListArgs, WorktreeRemoveArgs};
use super::interaction::{Interaction, InteractionError};
use crate::application::ports::WorktreeLosses;
use crate::application::worktrees::{
    self, AddError, AddRequest, Blocker, BranchChoice, Placement, RemovalInspection, RemoveError,
    Repository, WorktreeListing, short_id,
};
use crate::domain::config::{Config, resolve_worktree_root};
use crate::domain::repository::{RepositoryRef, validate_reference_safety};
use crate::infrastructure::{filesystem::Filesystem, git::NativeGit};
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) fn error(message: impl std::fmt::Display) {
    eprintln!("error: {}", escape(message));
}

pub(super) struct Settings {
    home: PathBuf,
    pub(super) cwd: PathBuf,
    config: Option<Config>,
    worktree_root: Option<PathBuf>,
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
            worktree_root: None,
        };
        if std::fs::symlink_metadata(config_path).is_ok() {
            let (config, worktree_root, warnings) =
                crate::infrastructure::config::load_worktrees(config_path)
                    .map_err(|source| fail(source.to_string()))?;
            for warning in warnings {
                eprintln!("lager: warning: {warning}");
            }
            settings.worktree_root = worktree_root
                .map(|value| resolve_worktree_root(&value, &settings.home))
                .transpose()
                .map_err(|source| fail(source.to_string()))?;
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
    match command {
        WorktreeCommand::List(args) => list(&settings, args),
        WorktreeCommand::Add(args) => add(&settings, args),
        WorktreeCommand::Remove(args) => remove(&settings, args, interaction),
    }
}

fn add_request(args: &WorktreeAddArgs) -> AddRequest<'_> {
    AddRequest {
        name: &args.name,
        branch: match (&args.branch, &args.new_branch) {
            (Some(branch), _) => BranchChoice::Existing(branch),
            (None, Some(branch)) => BranchChoice::New(branch),
            (None, None) => BranchChoice::Named,
        },
        start: args.start.as_deref(),
        path: None,
    }
}

fn list(settings: &Settings, args: WorktreeListArgs) -> i32 {
    let repository = match &args.repository {
        Some(argument) => settings.repository(argument),
        None => worktrees::resolve_repository(&GIT, &FILES, &settings.cwd, false),
    };
    let repository = match repository {
        Ok(repository) => repository,
        Err(message) if args.repository.is_none() => {
            error(format!("{message}; name a repository"));
            return 2;
        }
        Err(message) => {
            error(message);
            return 1;
        }
    };
    let listing = worktrees::list(&GIT, &FILES, &repository);
    let stdout = if args.json {
        match serde_json::to_string_pretty(&Document::new(&listing)) {
            Ok(document) => document + "\n",
            Err(source) => {
                error(format!("could not render the worktree list: {source}"));
                return 1;
            }
        }
    } else {
        table(&listing)
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
struct Document {
    schema_version: u8,
    worktrees: Vec<Record>,
    errors: Vec<Fault>,
}

#[derive(Serialize)]
struct Record {
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
}

#[derive(Serialize)]
struct Fault {
    path: Option<String>,
    code: &'static str,
    error: String,
}

impl Document {
    fn new(listing: &WorktreeListing) -> Self {
        let text = |path: &Path| path.to_string_lossy().into_owned();
        Self {
            schema_version: 1,
            worktrees: listing
                .worktrees
                .iter()
                .map(|record| Record {
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

/// Path, branch and state, two spaces apart.
fn table(listing: &WorktreeListing) -> String {
    let rows = listing
        .worktrees
        .iter()
        .map(|record| {
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
            [
                escape(record.path.display()),
                escape(branch_label(
                    record.branch.as_deref(),
                    record.head.as_deref(),
                )),
                escape(states.join(", ")),
            ]
        })
        .collect::<Vec<_>>();
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (path, branch) = (width(0), width(1));
    rows.iter()
        .map(|row| {
            format!("{:path$}  {:branch$}  {}", row[0], row[1], row[2])
                .trim_end()
                .to_owned()
                + "\n"
        })
        .collect()
}

fn add(settings: &Settings, args: WorktreeAddArgs) -> i32 {
    let request = add_request(&args);
    let repository = match settings.repository(&args.repository) {
        Ok(repository) => repository,
        Err(message) => {
            error(message);
            return 1;
        }
    };
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
            root: root.as_deref(),
            worktree_root: settings.worktree_root.as_deref(),
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
    match worktrees::add(&GIT, &FILES, &plan) {
        Ok(()) => {
            println!(
                "Created worktree {} on {}branch {}",
                escape(plan.destination.display()),
                if plan.start.is_some() { "new " } else { "" },
                escape(&plan.branch)
            );
            0
        }
        Err(failure) => {
            error(format!("worktree was not created: {}", failure.message));
            if failure.remaining.is_empty() {
                eprintln!("remaining: nothing");
            }
            for remaining in &failure.remaining {
                eprintln!("remaining: {}", escape(remaining));
            }
            if failure.cancelled { 130 } else { 1 }
        }
    }
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
