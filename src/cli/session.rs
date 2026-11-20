//! `lager session`: arguments in, service calls, text out.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::args::{SessionArgs, SessionCommand};
use super::worktree::{Settings, error, is_path};
use crate::application::ports::{
    HostError, Recorded, SessionRecords, WorktreeFilesystem, WorktreeGit,
};
use crate::application::sessions::{self, Ensured, SessionError};
use crate::application::worktrees;
use crate::domain::session::Layout;
use crate::infrastructure::{
    filesystem::Filesystem, git::NativeGit, rex::Rex, session_record::SessionFile,
};
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) fn run(config_path: &Path, command: SessionCommand) -> i32 {
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
    let result =
        match command {
            SessionCommand::Open(args) => {
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
            SessionCommand::Close(args) => recorded_or_checkout(&settings, &args, &records)
                .and_then(|path| {
                    match sessions::close(&host, &records, &path).map_err(failure)? {
                        Some(recorded) => println!("Closed session {}", escape(&recorded.label)),
                        None => println!("No session for {}", escape(path.display())),
                    }
                    Ok(())
                }),
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
    match result {
        Ok(()) => 0,
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            1
        }
    }
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
