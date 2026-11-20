//! `lager session`: arguments in, service calls, text out.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::args::{SessionArgs, SessionCommand, SessionPickArgs};
use super::worktree::{Settings, error, is_path};
use crate::application::ports::{
    HostError, Recorded, RepositorySelector, SelectionCandidate, SelectionError, SessionRecords,
    WorktreeFilesystem, WorktreeGit,
};
use crate::application::sessions::{self, Ensured, SessionError};
use crate::application::worktrees;
use crate::domain::repository::RepositoryRef;
use crate::domain::session::Layout;
use crate::infrastructure::{
    filesystem::Filesystem, git::NativeGit, rex::Rex, session_record::SessionFile,
};
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) fn run(
    config_path: &Path,
    command: SessionCommand,
    selector: &dyn RepositorySelector,
    interactive: bool,
) -> i32 {
    let settings = match Settings::load(config_path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let layout = match layout(config_path) {
        Ok(layout) => layout,
        Err(message) => {
            error(message);
            return 1;
        }
    };
    let (host, records) = (Rex, SessionFile);
    let target = |args: SessionPickArgs, close: bool| match args.repository {
        Some(repository) => Ok(SessionArgs {
            repository,
            name: args.name,
        }),
        None => choose(&settings, &records, selector, interactive, close),
    };
    let result =
        match command {
            SessionCommand::Open(args) => {
                let args = match target(args, false) {
                    Ok(args) => args,
                    Err(code) => return code,
                };
                checkout(&settings, &args)
                    .and_then(present)
                    .and_then(|(path, label)| {
                        let opened = sessions::open(&host, &records, &path, &label, &layout)
                            .map_err(failure)?;
                        println!("Opened session {}", escape(&opened.recorded().label));
                        Ok(())
                    })
            }
            SessionCommand::Create(args) => checkout(&settings, &args.checkout)
                .and_then(present)
                .and_then(|(path, label)| {
                    let ensured = sessions::ensure(&host, &records, &path, &label, &layout)
                        .map_err(failure)?;
                    let (created, recorded) = match &ensured {
                        Ensured::Created(recorded) => (true, recorded),
                        Ensured::Existing(recorded) => (false, recorded),
                    };
                    if args.json {
                        print_json(&Created {
                            schema_version: 1,
                            created,
                            session: Entry::new(&path, recorded),
                        })
                    } else if created {
                        println!("Created session {}", escape(&recorded.label));
                        Ok(())
                    } else {
                        println!("Session {} already exists", escape(&recorded.label));
                        Ok(())
                    }
                }),
            SessionCommand::Close(args) => {
                let args = match target(args, true) {
                    Ok(args) => args,
                    Err(code) => return code,
                };
                recorded_or_checkout(&settings, &args, &records).and_then(|path| {
                    match sessions::close(&host, &records, &path).map_err(failure)? {
                        Some(recorded) => println!("Closed session {}", escape(&recorded.label)),
                        None => println!("No session for {}", escape(path.display())),
                    }
                    Ok(())
                })
            }
            SessionCommand::List(args) => sessions::list(&host, &records)
                .map_err(failure)
                .and_then(|listed| {
                    if args.json {
                        print_json(&Listed {
                            schema_version: 1,
                            sessions: listed
                                .iter()
                                .map(|(path, recorded)| Entry::new(path, recorded))
                                .collect(),
                        })
                    } else {
                        write(&table(&listed))
                    }
                }),
        };
    result.map_or_else(fail, |()| 0)
}

fn fail(lines: Lines) -> i32 {
    for line in lines {
        eprintln!("{line}");
    }
    1
}

/// The default layout of the `[session]` section; an error when there is none.
pub(super) fn layout(config_path: &Path) -> Result<Layout, String> {
    let not_configured = || {
        format!(
            "sessions are not configured; add [session] to {}",
            config_path.display()
        )
    };
    if std::fs::symlink_metadata(config_path).is_err() {
        return Err(not_configured());
    }
    crate::infrastructure::config::load_session(config_path)
        .map_err(|source| source.to_string())?
        .ok_or_else(not_configured)?
        .layout()
}

/// Error lines, each ready for stderr.
type Lines = Vec<String>;

fn line(message: impl std::fmt::Display) -> Lines {
    vec![format!("error: {}", escape(message))]
}

fn failure(error: SessionError) -> Lines {
    line(reason(error))
}

pub(super) fn reason(error: SessionError) -> String {
    match error {
        SessionError::Host(HostError::Unavailable(reason)) => {
            format!("session host unavailable: {reason}")
        }
        SessionError::Host(HostError::Failed(reason)) => format!("session host failed: {reason}"),
        SessionError::Records(reason) => reason,
    }
}

/// The canonical checkout `args` names, and its session label. A path names the working tree
/// it lies in, a reference the primary checkout; `NAME` picks a linked worktree by directory.
fn checkout(settings: &Settings, args: &SessionArgs) -> Result<(PathBuf, String), Lines> {
    let (path, repository) = if is_path(&args.repository) {
        let path = worktrees::absolute(&settings.cwd, Path::new(&args.repository));
        let path = GIT
            .locate(&path)
            .map_err(|_| line(format!("not a Git checkout: {}", path.display())))?
            .top_level;
        let repository = worktrees::resolve_repository(&GIT, &FILES, &path, false).map_err(line)?;
        (path, repository)
    } else {
        let repository = settings.repository(&args.repository).map_err(line)?;
        (repository.primary.clone(), repository)
    };
    let checkout = match &args.name {
        None => path,
        Some(name) => named(&repository, name)?,
    };
    let label = label(&repository.primary, &checkout);
    Ok((checkout, label))
}

/// The checkout a person picks to open, from every checkout, or to close, from those with a
/// live session; otherwise the exit code, `0` when there is nothing to pick.
fn choose(
    settings: &Settings,
    records: &SessionFile,
    selector: &dyn RepositorySelector,
    interactive: bool,
    close: bool,
) -> Result<SessionArgs, i32> {
    let verb = if close { "close" } else { "open" };
    if !interactive {
        error(format!(
            "session {verb} requires a repository outside a terminal"
        ));
        return Err(2);
    }
    let live = sessions::list(&Rex, records)
        .map_err(failure)
        .map_err(fail)?;
    let rows: Vec<(PathBuf, String, bool)> = if close {
        live.into_iter()
            .map(|(path, recorded)| (path, recorded.label, false))
            .collect()
    } else {
        checkouts(settings)
            .map_err(line)
            .map_err(fail)?
            .into_iter()
            .map(|(path, label)| {
                let open = live.contains_key(&path);
                (path, label, open)
            })
            .collect()
    };
    let width = rows
        .iter()
        .map(|(_, label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let candidates: Vec<_> = rows
        .iter()
        .map(|(path, label, open)| SelectionCandidate {
            // The selector hands the candidate back; nothing parses this reference.
            reference: RepositoryRef {
                clone_url: String::new(),
                host: "local".to_owned(),
                path: path.display().to_string(),
                destination_segments: Vec::new(),
                wildcard: false,
            },
            display: format!(
                "{label:width$}  {}{}",
                path.display(),
                if *open { " ●" } else { "" }
            ),
            archived: false,
            exact_path: None,
        })
        .collect();
    let picked = selector
        .select(&candidates)
        .map_err(|failure| match failure {
            SelectionError::Cancelled => 130,
            SelectionError::Unavailable(message) | SelectionError::Failed(message) => {
                error(message);
                1
            }
        })?;
    let index = match picked.as_slice() {
        [] => return Err(0),
        [one] => candidates
            .iter()
            .position(|candidate| candidate == one)
            .expect("the selector returns an offered candidate"),
        many => {
            error(format!(
                "session {verb} takes one checkout; {} were picked",
                many.len()
            ));
            return Err(2);
        }
    };
    Ok(SessionArgs {
        repository: rows[index].0.to_string_lossy().into_owned(),
        name: None,
    })
}

/// Each checkout under the root and every working tree of its repository whose directory
/// exists, by record key, with its label. A bare repository and a checkout Git cannot list are
/// left out.
fn checkouts(settings: &Settings) -> Result<std::collections::BTreeMap<PathBuf, String>, String> {
    let mut checkouts = std::collections::BTreeMap::new();
    let Some(config) = &settings.config else {
        return Ok(checkouts);
    };
    let root = config
        .resolve_root(&settings.home)
        .map_err(|source| source.to_string())?;
    if !root.is_dir() {
        return Ok(checkouts);
    }
    let found = crate::application::ports::RemovalFilesystem::discover_checkouts(&FILES, &root)
        .map_err(|source| source.to_string())?;
    for path in found {
        if checkouts.contains_key(&key(&path)) {
            continue;
        }
        let Ok(entries) = GIT.entries(&path) else {
            continue;
        };
        let Some(primary) = entries
            .first()
            .filter(|entry| !entry.bare)
            .map(|entry| key(&entry.path))
        else {
            continue;
        };
        for entry in entries.iter().filter(|entry| entry.path.is_dir()) {
            let checkout = key(&entry.path);
            checkouts.insert(checkout.clone(), label(&primary, &checkout));
        }
    }
    Ok(checkouts)
}

/// The checkout `args` names for closing. A path Git no longer knows still names the session
/// recorded under it, so a worktree removed outside lager keeps a way to close its session.
fn recorded_or_checkout(
    settings: &Settings,
    args: &SessionArgs,
    records: &SessionFile,
) -> Result<PathBuf, Lines> {
    let missing = match checkout(settings, args) {
        Ok((path, _)) => return Ok(path),
        Err(lines) => lines,
    };
    if args.name.is_some() || !is_path(&args.repository) {
        return Err(missing);
    }
    let path = key(&worktrees::absolute(
        &settings.cwd,
        Path::new(&args.repository),
    ));
    match records.update(|records| records.contains_key(&path)) {
        Ok(true) => Ok(path),
        _ => Err(missing),
    }
}

/// The session label of `checkout`, a linked worktree unless it is `primary`.
pub(super) fn label(primary: &Path, checkout: &Path) -> String {
    let directory = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let worktree = (checkout != primary).then(|| directory(checkout));
    sessions::label(&directory(primary), worktree.as_deref())
}

/// Opening and creating need the checkout's directory; closing also reaches a removed
/// worktree's session.
fn present((path, label): (PathBuf, String)) -> Result<(PathBuf, String), Lines> {
    if path.is_dir() {
        Ok((path, label))
    } else {
        Err(line(format!("not a Git checkout: {}", path.display())))
    }
}

/// The linked worktree whose directory is `name`, canonical while it exists; the error lists
/// the candidates and what Git could not list.
fn named(repository: &worktrees::Repository, name: &str) -> Result<PathBuf, Lines> {
    let listing = worktrees::list(&GIT, &FILES, repository);
    let linked = listing.worktrees.iter().filter(|record| !record.is_primary);
    let matching: Vec<_> = linked
        .clone()
        .filter(|record| record.path.file_name() == Some(name.as_ref()))
        .collect();
    let (message, paths): (_, Vec<_>) = match matching.as_slice() {
        [record] => return Ok(key(&record.path)),
        [] => (
            format!(
                "no linked worktree of {} is named {name}",
                repository.primary.display()
            ),
            linked.collect(),
        ),
        _ => (
            format!("several linked worktrees are named {name}"),
            matching,
        ),
    };
    let mut lines = line(message);
    lines.extend(
        paths
            .iter()
            .map(|record| format!("  {}", escape(record.path.display()))),
    );
    for fault in &listing.errors {
        lines.extend(line(match &fault.path {
            Some(path) => format!("{}: {}", path.display(), fault.error),
            None => fault.error.clone(),
        }));
    }
    Err(lines)
}

/// The record key of a worktree Git lists: its real path, which a removed worktree keeps
/// through its parent's real path.
pub(super) fn key(path: &Path) -> PathBuf {
    let canonical = FILES.canonicalize(path).or_else(|_| {
        let parent = path.parent().ok_or_else(String::new)?;
        Ok::<_, String>(
            FILES
                .canonicalize(parent)?
                .join(path.file_name().unwrap_or_default()),
        )
    });
    canonical.unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Serialize)]
struct Entry<'a> {
    path: String,
    #[serde(flatten)]
    recorded: &'a Recorded,
}

impl<'a> Entry<'a> {
    fn new(path: &Path, recorded: &'a Recorded) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            recorded,
        }
    }
}

#[derive(Serialize)]
struct Created<'a> {
    schema_version: u8,
    created: bool,
    #[serde(flatten)]
    session: Entry<'a>,
}

#[derive(Serialize)]
struct Listed<'a> {
    schema_version: u8,
    sessions: Vec<Entry<'a>>,
}

fn print_json(document: &impl Serialize) -> Result<(), Lines> {
    let document = serde_json::to_string_pretty(document)
        .map_err(|source| line(format!("could not render the sessions: {source}")))?;
    write(&(document + "\n"))
}

fn write(text: &str) -> Result<(), Lines> {
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|source| line(format!("could not write the sessions: {source}")))
}

/// Label, path and id, two spaces apart.
fn table(listed: &std::collections::BTreeMap<PathBuf, Recorded>) -> String {
    let rows: Vec<[String; 3]> = listed
        .iter()
        .map(|(path, recorded)| {
            [
                escape(&recorded.label),
                escape(path.display()),
                escape(&recorded.id),
            ]
        })
        .collect();
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (label, path) = (width(0), width(1));
    rows.iter()
        .map(|row| format!("{:label$}  {:path$}  {}\n", row[0], row[1], row[2]))
        .collect()
}
