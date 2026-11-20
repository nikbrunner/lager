//! The GitHub group of the row menu and its dialogs: rename, archive, visibility, delete and
//! create. Every rule and check comes from the remote service the CLI verbs use; a dialog only
//! stands in for the CLI's prompt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::keys::{Action, Mode};
use super::{
    Entry, InventorySession, Menu, MenuLine, POPUP, Selection, View, WIDE_POPUP, choice_line, dim,
    edit_text, exact_field_lines, field_lines, first_key_hint, focus, home_relative, model,
    popup_body_width, refresh, start_batch, tint, warn,
};
use crate::application::batch::{self, AddChoice, Batch, BatchAction, Eligibility};
use crate::application::ports::{Interaction, InteractionError, RemovalFilesystem, Visibility};
use crate::application::remote::{self, ActionError, Done};
use crate::application::warehouse;
use crate::domain::config::{Config, RemoteAction};
use crate::domain::repository::RepositoryRef;
use crate::infrastructure::config::CommandConfigStore;
use crate::infrastructure::filesystem::Filesystem;
use crate::infrastructure::git::NativeGit;
use crate::infrastructure::inventory::LocalInventoryFilesystem;
use crate::infrastructure::providers::ConfiguredProviders;
use crate::presentation::escape;

/// What a GitHub menu entry does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    Rename,
    Archive(bool),
    Visibility(Visibility),
    Delete,
    Create,
}

/// The running action and what earlier actions taught about each repository.
#[derive(Default)]
pub(super) struct Remote {
    job: Option<Job>,
    /// Archive and visibility as the last action left them, by identity.
    known: HashMap<String, (Option<bool>, Option<Visibility>)>,
}

struct Job {
    identity: String,
    status: String,
    handle: JoinHandle<Result<Done, ActionError>>,
    /// The form that started the action, reopened when the action is refused.
    dialog: Option<Dialog>,
}

enum Op {
    Archive(bool),
    Visibility(Visibility),
    Rename(String),
    Delete,
    Create {
        visibility: Visibility,
        description: Option<String>,
        register: bool,
    },
}

pub(super) enum Dialog {
    Create {
        repository: String,
        description: String,
        private: bool,
        register: bool,
        /// Repository, description, Private, Register.
        focus: usize,
        rejected: Option<String>,
    },
    Rename {
        reference: RepositoryRef,
        checkout: Option<PathBuf>,
        registration: model::RegistrationState,
        text: String,
        rejected: Option<String>,
    },
    Delete {
        reference: RepositoryRef,
        /// The checkout and its losses, or why it cannot be removed.
        checkout: Option<(PathBuf, Result<Vec<String>, String>)>,
        registration: model::RegistrationState,
        text: String,
        /// The cursor is on "Delete everywhere" instead of Cancel.
        delete: bool,
    },
    Visibility {
        reference: RepositoryRef,
        target: Visibility,
        confirm: bool,
    },
}

impl Dialog {
    pub(super) fn view(&self) -> View {
        match self {
            Self::Create { .. } => View::RemoteCreateInput,
            Self::Rename { .. } => View::RemoteRenameInput,
            Self::Delete { .. } => View::RemoteDeleteConfirm,
            Self::Visibility { .. } => View::RemoteVisibilityConfirm,
        }
    }
}

/// The dialogs confirm before an action starts, so the service never prompts.
struct Unattended;

impl Interaction for Unattended {
    fn confirm(&mut self, _message: &str, _default: bool) -> Result<bool, InteractionError> {
        Err(InteractionError::Failed(
            "the inventory confirms before an action starts".to_owned(),
        ))
    }

    fn input(
        &mut self,
        _message: &str,
        _placeholder: &str,
        _default: Option<&str>,
    ) -> Result<String, InteractionError> {
        Err(InteractionError::Failed(
            "the inventory confirms before an action starts".to_owned(),
        ))
    }
}

/// The one GitHub repository a row stands for, when its host has a github provider.
fn target(row: &model::InventoryRow, config: &Config) -> Option<RepositoryRef> {
    let reference = RepositoryRef::parse(super::identity(row)?).ok()?;
    let github = config
        .providers
        .get(&reference.host.to_ascii_lowercase())
        .is_some_and(|provider| provider.preset == "github");
    (github && !reference.is_wildcard()).then_some(reference)
}

/// Hosts whose github provider allows creating repositories.
fn create_hosts(config: &Config) -> Vec<&str> {
    config
        .providers
        .iter()
        .filter(|(_, provider)| {
            provider.preset == "github" && provider.remote_actions.contains(&RemoteAction::Create)
        })
        .map(|(host, _)| host.as_str())
        .collect()
}

impl Remote {
    pub(super) fn running(&self) -> bool {
        self.job.is_some()
    }
}

/// Why no GitHub action can start right now.
fn busy(session: &InventorySession) -> Option<String> {
    if session.github.job.is_some() {
        Some("a GitHub action is running".to_owned())
    } else if session.running() {
        Some("a batch is running".to_owned())
    } else {
        None
    }
}

fn line(entry: Entry, label: &str, hint: &str, blocked: Option<String>) -> MenuLine {
    MenuLine {
        entry: Some(entry),
        label: label.to_owned(),
        enabled: blocked.is_none(),
        hint: blocked.unwrap_or_else(|| hint.to_owned()),
    }
}

/// Why `action` is unavailable on `reference`; a GitHub action targets one row, never the marks.
fn blocked(
    session: &InventorySession,
    reference: &RepositoryRef,
    action: RemoteAction,
) -> Option<String> {
    let marked = session.marks.len();
    (marked > 0)
        .then(|| format!("one row only · {marked} marked"))
        .or_else(|| busy(session))
        .or_else(|| {
            remote::allowed(&session.config, reference, action)
                .err()
                .map(|_| format!("not in remote_actions (add \"{}\")", action.as_str()))
        })
}

fn visibility_of(session: &InventorySession, reference: &RepositoryRef) -> Option<Visibility> {
    session
        .github
        .known
        .get(&reference.identity())
        .and_then(|(_, visibility)| *visibility)
}

/// The "On <host>" group of the Repository actions menu; empty for a row without one GitHub
/// repository.
pub(super) fn group(
    session: &InventorySession,
    row: Option<&model::InventoryRow>,
) -> Vec<MenuLine> {
    let Some((row, reference)) = row.and_then(|row| Some((row, target(row, &session.config)?)))
    else {
        return Vec::new();
    };
    let known = session.github.known.get(&reference.identity());
    let archived = known.and_then(|(archived, _)| *archived).or(row.archived);
    let gate = |action| blocked(session, &reference, action);
    let heading = |label: String| MenuLine {
        entry: None,
        label,
        hint: String::new(),
        enabled: false,
    };
    vec![
        heading(String::new()),
        heading(format!("On {}", escape(&reference.host))),
        line(
            Entry::Github(Act::Rename),
            "Rename…",
            if model::worktree_checkout(row).is_ok() {
                "moves the checkout"
            } else {
                ""
            },
            gate(RemoteAction::Rename),
        ),
        if archived == Some(true) {
            line(
                Entry::Github(Act::Archive(false)),
                "Unarchive",
                "",
                gate(RemoteAction::Archive),
            )
        } else {
            line(
                Entry::Github(Act::Archive(true)),
                "Archive",
                "",
                gate(RemoteAction::Archive),
            )
        },
        match visibility_of(session, &reference) {
            Some(Visibility::Private) => line(
                Entry::Github(Act::Visibility(Visibility::Public)),
                "Make public…",
                "private",
                gate(RemoteAction::Visibility),
            ),
            Some(Visibility::Public) => line(
                Entry::Github(Act::Visibility(Visibility::Private)),
                "Make private…",
                "public",
                gate(RemoteAction::Visibility),
            ),
            _ => line(
                Entry::Open(Menu::Visibility),
                "Change visibility…",
                "",
                gate(RemoteAction::Visibility),
            ),
        },
        line(
            Entry::Github(Act::Delete),
            "Delete everywhere…",
            "GitHub, checkout, config",
            gate(RemoteAction::Delete),
        ),
    ]
}

/// The two choices of "Change visibility…".
pub(super) fn visibility_lines(
    session: &InventorySession,
    row: Option<&model::InventoryRow>,
) -> Vec<MenuLine> {
    let gate = row
        .and_then(|row| target(row, &session.config))
        .map_or(Some("no GitHub repository".to_owned()), |reference| {
            blocked(session, &reference, RemoteAction::Visibility)
        });
    vec![
        line(
            Entry::Github(Act::Visibility(Visibility::Public)),
            "Make public…",
            "",
            gate.clone(),
        ),
        line(
            Entry::Github(Act::Visibility(Visibility::Private)),
            "Make private…",
            "",
            gate,
        ),
    ]
}

/// "New repository on GitHub…" while some github provider allows create.
pub(super) fn create_line(session: &InventorySession) -> Option<MenuLine> {
    (!create_hosts(&session.config).is_empty()).then(|| {
        line(
            Entry::Github(Act::Create),
            "New repository on GitHub…",
            "",
            busy(session),
        )
    })
}

/// Runs a chosen GitHub entry: opens its dialog or starts it right away.
pub(super) fn activate(
    session: &mut InventorySession,
    selection: &mut Selection,
    act: Act,
    row: Option<&model::InventoryRow>,
) {
    if act == Act::Create {
        selection.github = Some(Dialog::Create {
            repository: create_hosts(&session.config)
                .first()
                .map(|host| format!("{host}/"))
                .unwrap_or_default(),
            description: String::new(),
            private: true,
            register: true,
            focus: 0,
            rejected: None,
        });
        return;
    }
    let Some((row, reference)) = row.and_then(|row| Some((row, target(row, &session.config)?)))
    else {
        return;
    };
    match act {
        Act::Archive(archived) => start(session, reference, Op::Archive(archived), None),
        Act::Visibility(target) => {
            selection.preview_fresh = true;
            selection.github = Some(Dialog::Visibility {
                reference,
                target,
                confirm: false,
            });
        }
        Act::Rename => {
            let checkout = warehouse::destination(&session.config, &reference, &session.home)
                .ok()
                .filter(|path| std::fs::symlink_metadata(path).is_ok());
            let name = reference
                .path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            selection.github = Some(Dialog::Rename {
                reference,
                checkout,
                registration: row.registration.clone(),
                text: name,
                rejected: None,
            });
        }
        Act::Delete => {
            // The same guards the service runs; it runs them again before GitHub is touched.
            let checkout = match warehouse::destination(&session.config, &reference, &session.home)
            {
                Err(error) => Some((PathBuf::new(), Err(error))),
                Ok(path) => match Filesystem.metadata_exists(&path) {
                    Ok(false) => None,
                    Ok(true) => {
                        let inspected = warehouse::inspect_exact_removal(
                            &NativeGit,
                            &Filesystem,
                            &session.root,
                            &path,
                            &reference.clone_url,
                        )
                        .map(|checkout| checkout.losses);
                        Some((path, inspected))
                    }
                    Err(error) => Some((path, Err(error.to_string()))),
                },
            };
            selection.preview_fresh = true;
            selection.github = Some(Dialog::Delete {
                reference,
                checkout,
                registration: row.registration.clone(),
                text: String::new(),
                delete: false,
            });
        }
        Act::Create => {}
    }
}

fn verb(op: &Op) -> &'static str {
    match op {
        Op::Archive(true) => "Archiving",
        Op::Archive(false) => "Unarchiving",
        Op::Visibility(Visibility::Public) => "Making public",
        Op::Visibility(_) => "Making private",
        Op::Rename(_) => "Renaming",
        Op::Delete => "Deleting",
        Op::Create { .. } => "Creating",
    }
}

/// Runs `op` off the UI loop; the status line shows it until `poll` reports the outcome.
fn start(session: &mut InventorySession, reference: RepositoryRef, op: Op, dialog: Option<Dialog>) {
    if let Some(reason) = busy(session) {
        session.message = Some(format!("GitHub action unavailable: {reason}"));
        return;
    }
    let identity = reference.identity();
    let status = format!(
        "{} {} on {}… · quitting waits for it",
        verb(&op),
        escape(&reference.path),
        escape(&reference.host)
    );
    let config = session.config.clone();
    let config_path = session.config_path.clone();
    let home = session.home.clone();
    let handle = std::thread::spawn(move || {
        let providers = ConfiguredProviders::from_config(&config)?;
        let store = CommandConfigStore::quiet();
        let mut unattended = Unattended;
        match op {
            Op::Archive(archived) => remote::archive(&config, &providers, &reference, archived)
                .map(|mutation| Done::Archived(archived, mutation)),
            Op::Visibility(visibility) => remote::visibility(
                &config,
                &providers,
                &mut unattended,
                &reference,
                visibility,
                true,
            )
            .map(|mutation| Done::Visibility(visibility, mutation)),
            Op::Rename(name) => remote::rename(
                &config,
                &providers,
                &store,
                &NativeGit,
                &Filesystem,
                &mut unattended,
                &config_path,
                &reference,
                &name,
                true,
                &home,
            )
            .map(Done::Renamed),
            // Never forced: local losses refuse before GitHub is touched.
            Op::Delete => remote::delete(
                &config,
                &providers,
                &store,
                &NativeGit,
                &Filesystem,
                &mut unattended,
                &config_path,
                &reference,
                true,
                false,
                &home,
            )
            .map(Done::Deleted),
            Op::Create {
                visibility,
                description,
                register,
            } => remote::create(
                &config,
                &providers,
                &NativeGit,
                &reference,
                visibility,
                description.as_deref(),
                &home,
            )
            .map(|_| Done::Created { register }),
        }
    });
    session.github.job = Some(Job {
        identity,
        status,
        handle,
        dialog,
    });
}

/// The status line of the running action.
pub(super) fn status(session: &InventorySession) -> Option<String> {
    session.github.job.as_ref().map(|job| job.status.clone())
}

fn report(identity: &str, result: &Result<Done, ActionError>) -> String {
    match result {
        Ok(done) => format!("{}: {}", escape(identity), done.outcome()),
        Err(ActionError::Declined | ActionError::Cancelled) => {
            format!("{}: cancelled", escape(identity))
        }
        Err(ActionError::Failed(error)) => format!("{}: {}", escape(identity), escape(error)),
    }
}

/// Reports a finished action and refreshes what it changed. A created repository goes on to
/// the clone batch Add uses.
pub(super) fn poll(session: &mut InventorySession, selection: &mut Selection) {
    if !session
        .github
        .job
        .as_ref()
        .is_some_and(|job| job.handle.is_finished())
    {
        return;
    }
    let job = session.github.job.take().expect("a finished job");
    let result = job
        .handle
        .join()
        .unwrap_or_else(|_| Err(ActionError::Failed("the action panicked".to_owned())));
    let known = session
        .github
        .known
        .entry(job.identity.clone())
        .or_default();
    match &result {
        Ok(Done::Archived(archived, _)) => known.0 = Some(*archived),
        Ok(Done::Visibility(visibility, _)) => known.1 = Some(*visibility),
        _ => {}
    }
    session.message = Some(match (&result, job.dialog) {
        // A refused form comes back with what was typed and the whole reason.
        (Err(ActionError::Failed(error)), Some(mut dialog)) if selection.github.is_none() => {
            if let Dialog::Create { rejected, .. } | Dialog::Rename { rejected, .. } = &mut dialog {
                *rejected = Some(error.clone());
            }
            selection.github = Some(dialog);
            session.problems.record("github", escape(error));
            format!("{}: failed, the form shows why", escape(&job.identity))
        }
        // The status line cuts long text; the problems popup wraps the whole message.
        (Err(ActionError::Failed(error)), _) => {
            session.problems.record("github", escape(error));
            format!(
                "{}: failed, see Problems · {}",
                escape(&job.identity),
                escape(error)
            )
        }
        _ => report(&job.identity, &result),
    });
    // A failure can follow a change on GitHub, so the rows are read again either way.
    refresh(session);
    if session.remote.enabled() {
        session.remote.refresh();
    }
    if let Ok(Done::Created { register }) = result {
        clone_created(session, selection, &job.identity, register);
    }
}

fn clone_created(
    session: &mut InventorySession,
    selection: &mut Selection,
    identity: &str,
    register: bool,
) {
    let (action, choice) = if register {
        (BatchAction::CloneRegister, AddChoice::CloneRegister)
    } else {
        (BatchAction::Clone, AddChoice::Clone)
    };
    let rows = session
        .report()
        .map(|report| report.rows)
        .unwrap_or_default();
    let preview = batch::add_preview(
        identity,
        &session.config,
        &session.root,
        &rows,
        session.complete,
        &LocalInventoryFilesystem,
    )
    .and_then(|preview| match preview.eligibility(choice) {
        Eligibility::Unavailable(reason) => Err(reason.to_owned()),
        _ if session.running() => Err("a batch is running".to_owned()),
        _ => Ok(preview),
    });
    match preview {
        Ok(preview) => {
            let target = preview.clone_target(action, escape(identity));
            start_batch(
                session,
                selection,
                Batch {
                    previewed: true,
                    ..Batch::new(action, vec![target])
                },
            );
        }
        Err(reason) => {
            session.message = Some(format!(
                "created {}; clone unavailable: {}; rerun `lager add {}`",
                escape(identity),
                escape(reason),
                escape(identity)
            ));
        }
    }
}

/// A confirmed action always finishes: quitting waits for the running one and prints its
/// outcome, so a rename or delete is never cut between GitHub and the checkout.
pub(super) fn finish(session: &mut InventorySession) {
    let Some(job) = session.github.job.take() else {
        return;
    };
    if !job.handle.is_finished() {
        eprintln!(
            "lager: waiting for the GitHub action on {} to finish…",
            escape(&job.identity)
        );
    }
    let result = job
        .handle
        .join()
        .unwrap_or_else(|_| Err(ActionError::Failed("the action panicked".to_owned())));
    eprintln!("lager: {}", report(&job.identity, &result));
}

/// Applies one key to the open dialog.
pub(super) fn key(
    session: &mut InventorySession,
    selection: &mut Selection,
    action: Option<Action>,
    key: KeyEvent,
) {
    let Some(dialog) = selection.github.as_mut() else {
        return;
    };
    let cancelled = |what: &str| Some(format!("{what} cancelled · nothing changed"));
    let forward = matches!(key.code, KeyCode::Tab | KeyCode::Down);
    let backward = matches!(key.code, KeyCode::BackTab | KeyCode::Up);
    match dialog {
        Dialog::Create {
            repository,
            description,
            private,
            register,
            focus,
            rejected,
        } => match action {
            Some(Action::Cancel) => {
                selection.github = None;
                session.message = cancelled("New repository");
            }
            Some(Action::Accept) if *focus == 2 => *private = !*private,
            Some(Action::Accept) if *focus == 3 => *register = !*register,
            Some(Action::Accept) => {
                let text = repository.trim();
                match RepositoryRef::parse(text) {
                    Ok(reference) if !reference.is_wildcard() && !text.contains('*') => {
                        let op = Op::Create {
                            visibility: if *private {
                                Visibility::Private
                            } else {
                                Visibility::Public
                            },
                            description: Some(description.trim().to_owned())
                                .filter(|description| !description.is_empty()),
                            register: *register,
                        };
                        *rejected = None;
                        let dialog = selection.github.take();
                        start(session, reference, op, dialog);
                    }
                    Ok(_) => *rejected = Some("one repository, no wildcards".to_owned()),
                    Err(error) => *rejected = Some(error.to_string()),
                }
            }
            None if forward => *focus = (*focus + 1) % 4,
            None if backward => *focus = (*focus + 3) % 4,
            None => {
                match *focus {
                    0 => edit_text(repository, key),
                    1 => edit_text(description, key),
                    2 if key.code == KeyCode::Char(' ') => *private = !*private,
                    3 if key.code == KeyCode::Char(' ') => *register = !*register,
                    _ => {}
                }
                *rejected = None;
            }
            _ => {}
        },
        Dialog::Rename {
            reference,
            text,
            rejected,
            ..
        } => match action {
            Some(Action::Cancel) => {
                selection.github = None;
                session.message = cancelled("Rename");
            }
            Some(Action::Accept) if !remote::valid_name(text) => {
                *rejected = Some("use letters, digits, -, _ and . without a leading -".to_owned());
            }
            Some(Action::Accept) => {
                let (reference, name) = (reference.clone(), text.clone());
                *rejected = None;
                let dialog = selection.github.take();
                start(session, reference, Op::Rename(name), dialog);
            }
            None => {
                edit_text(text, key);
                *rejected = None;
            }
            _ => {}
        },
        Dialog::Delete {
            reference,
            checkout,
            text,
            delete,
            ..
        } => match action {
            Some(Action::Cancel) => {
                selection.github = None;
                session.message = cancelled("Delete everywhere");
            }
            // Enter on Cancel moves on to the delete choice once it is enabled.
            Some(Action::Accept) if !*delete && deletable(reference, checkout, text) => {
                *delete = true;
            }
            Some(Action::Accept) if !*delete => {
                selection.github = None;
                session.message = cancelled("Delete everywhere");
            }
            Some(Action::Accept) => {
                if deletable(reference, checkout, text) {
                    let reference = reference.clone();
                    selection.github = None;
                    start(session, reference, Op::Delete, None);
                }
            }
            None if forward || backward => *delete = !*delete,
            None => edit_text(text, key),
            _ => {}
        },
        Dialog::Visibility {
            reference,
            target,
            confirm,
        } => match action {
            Some(Action::Up | Action::Down) => *confirm = action == Some(Action::Down),
            Some(Action::Accept) if *confirm => {
                let (reference, target) = (reference.clone(), *target);
                selection.github = None;
                start(session, reference, Op::Visibility(target), None);
            }
            Some(Action::Accept | Action::Cancel) => {
                let what = visibility_label(*target);
                selection.github = None;
                session.message = cancelled(what);
            }
            _ => {}
        },
    }
}

fn visibility_label(target: Visibility) -> &'static str {
    if target == Visibility::Public {
        "Make public"
    } else {
        "Make private"
    }
}

/// The typed name matches exactly and nothing local would be lost.
fn deletable(
    reference: &RepositoryRef,
    checkout: &Option<(PathBuf, Result<Vec<String>, String>)>,
    text: &str,
) -> bool {
    text == reference.path
        && checkout
            .as_ref()
            .is_none_or(|(_, losses)| losses.as_ref().is_ok_and(Vec::is_empty))
}

fn text_line(text: &str, focused: bool, placeholder: &str) -> Line<'static> {
    if focused {
        Line::styled(format!("{}█", escape(text)), focus())
    } else if text.is_empty() {
        Line::styled(placeholder.to_owned(), dim())
    } else {
        Line::raw(escape(text))
    }
}

fn rejected_line(rejected: &Option<String>) -> Option<Line<'static>> {
    rejected
        .as_ref()
        .map(|reason| Line::styled(format!("Rejected: {}", escape(reason)), tint(Color::Red)))
}

fn shown(path: &Path, home: &Path) -> String {
    home_relative(path, home)
}

/// Draws the open dialog inside `area`, which leaves the status line readable.
pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    dialog: &Dialog,
    session: &InventorySession,
    area: Rect,
) {
    let bindings = &session.bindings;
    let home = session.home.as_path();
    let hint = |mode, action, label| first_key_hint(bindings, mode, action, label);
    let (cap, title, lines, footer) = match dialog {
        Dialog::Create {
            repository,
            description,
            private,
            register,
            focus: focused,
            rejected,
        } => {
            let width = popup_body_width(area, POPUP);
            let toggle = |on: bool, label: &str, index: usize| {
                let line = choice_line(
                    &format!("[{}] {label}", if on { "x" } else { " " }),
                    "",
                    true,
                    false,
                    usize::from(width),
                );
                if *focused == index {
                    line.style(Style::default().add_modifier(Modifier::BOLD))
                } else {
                    line
                }
            };
            let destination = RepositoryRef::parse(repository.trim())
                .ok()
                .filter(|reference| !reference.is_wildcard())
                .and_then(|reference| {
                    warehouse::destination(&session.config, &reference, home).ok()
                })
                .map_or("…".to_owned(), |path| shown(&path, home));
            let mut lines = vec![
                Line::styled("repository", dim()),
                text_line(repository, *focused == 0, "github.com/owner/name"),
                Line::styled("description", dim()),
                text_line(description, *focused == 1, "none"),
                toggle(*private, "Private", 2),
                toggle(*register, "Register", 3),
            ];
            lines.extend(rejected_line(rejected));
            lines.push(Line::default());
            lines.extend(exact_field_lines(
                "clone to",
                &destination,
                Style::default(),
                width,
            ));
            lines.extend([
                Line::default(),
                choice_line("Create and clone", "", true, false, usize::from(width)),
            ]);
            (
                POPUP,
                "New repository on GitHub".to_owned(),
                lines,
                format!(
                    "Tab focus · Space toggle · {} · {}",
                    hint(Mode::Input, Action::Accept, "choose"),
                    hint(Mode::Input, Action::Cancel, "back")
                ),
            )
        }
        Dialog::Rename {
            reference,
            checkout,
            registration,
            text,
            rejected,
        } => {
            let width = popup_body_width(area, WIDE_POPUP);
            let (owner, _) = reference
                .path
                .split_once('/')
                .unwrap_or((&reference.path, ""));
            let new_path = format!("{owner}/{}", if text.is_empty() { "…" } else { text });
            let destination = RepositoryRef::parse(&format!("{}/{new_path}", reference.host))
                .ok()
                .filter(|_| remote::valid_name(text))
                .and_then(|renamed| warehouse::destination(&session.config, &renamed, home).ok());
            let mut lines = vec![Line::styled("name", dim()), text_line(text, true, "")];
            lines.extend(rejected_line(rejected));
            lines.push(Line::default());
            lines.extend(exact_field_lines(
                "GitHub",
                &escape(format!("{} → {new_path}", reference.path)),
                Style::default(),
                width,
            ));
            match checkout {
                Some(source) => {
                    let target = destination
                        .as_deref()
                        .map_or("…".to_owned(), |path| shown(path, home));
                    lines.extend(exact_field_lines(
                        "checkout",
                        &format!("{} → {target}", shown(source, home)),
                        Style::default(),
                        width,
                    ));
                    lines.extend(exact_field_lines(
                        "origin",
                        &escape(format!("repointed to {}/{new_path}", reference.host)),
                        Style::default(),
                        width,
                    ));
                }
                None => lines.extend(field_lines(
                    "checkout",
                    "none · nothing moves",
                    Style::default(),
                    width,
                )),
            }
            lines.extend(field_lines(
                "config",
                &match registration {
                    model::RegistrationState::Explicit => format!(
                        "declaration rewritten in {}",
                        shown(&session.config_path, home)
                    ),
                    model::RegistrationState::Wildcard { pattern } => {
                        format!("covered by {} · unchanged", escape(pattern))
                    }
                    _ => "not declared · unchanged".to_owned(),
                },
                Style::default(),
                width,
            ));
            lines.extend([
                Line::default(),
                choice_line("Rename", "", true, false, usize::from(width)),
            ]);
            (
                WIDE_POPUP,
                format!("Rename on {}", escape(&reference.host)),
                lines,
                format!(
                    "{} · {}",
                    hint(Mode::Input, Action::Accept, "choose"),
                    hint(Mode::Input, Action::Cancel, "back")
                ),
            )
        }
        Dialog::Delete {
            reference,
            checkout,
            registration,
            text,
            delete,
        } => {
            let width = popup_body_width(area, WIDE_POPUP);
            let mut lines = field_lines(
                "GITHUB",
                &escape(format!(
                    "{} deleted permanently, with issues, PRs and releases",
                    reference.identity()
                )),
                warn(),
                width,
            );
            match checkout {
                None => lines.extend(field_lines(
                    "CHECKOUT",
                    "none · nothing local to remove",
                    Style::default(),
                    width,
                )),
                Some((path, Ok(losses))) if losses.is_empty() => {
                    lines.extend(exact_field_lines(
                        "CHECKOUT",
                        &shown(path, home),
                        Style::default(),
                        width,
                    ));
                    lines.extend(field_lines(
                        "",
                        "nothing local would be lost",
                        focus(),
                        width,
                    ));
                }
                Some((path, Ok(losses))) => {
                    lines.extend(exact_field_lines(
                        "CHECKOUT",
                        &shown(path, home),
                        Style::default(),
                        width,
                    ));
                    for loss in losses {
                        lines.extend(field_lines("", &escape(loss), warn(), width));
                    }
                }
                Some((_, Err(error))) => {
                    lines.extend(field_lines("CHECKOUT", &escape(error), warn(), width))
                }
            }
            lines.extend(field_lines(
                "CONFIG",
                &match registration {
                    model::RegistrationState::Explicit => format!(
                        "declaration removed from {}",
                        shown(&session.config_path, home)
                    ),
                    model::RegistrationState::Wildcard { pattern } => {
                        format!("excluded from {}", escape(pattern))
                    }
                    _ => "not declared · unchanged".to_owned(),
                },
                Style::default(),
                width,
            ));
            let ready = deletable(reference, checkout, text);
            let reason = if text != &reference.path {
                "name does not match yet".to_owned()
            } else if matches!(checkout, Some((_, Err(_)))) {
                "checkout could not be checked · see CHECKOUT".to_owned()
            } else if !ready {
                "local work would be lost · lager remove --remote --force".to_owned()
            } else {
                "cannot be undone".to_owned()
            };
            lines.extend([
                Line::default(),
                Line::raw(format!("Type {} to confirm", escape(&reference.path))),
                text_line(text, true, ""),
                Line::default(),
                choice_line("Cancel", "", true, !delete, usize::from(width)),
                choice_line(
                    "Delete everywhere",
                    &reason,
                    ready,
                    *delete,
                    usize::from(width),
                ),
            ]);
            (
                WIDE_POPUP,
                format!("Delete everywhere / {}", escape(&reference.path)),
                lines,
                format!(
                    "Tab focus · {} · {}",
                    hint(Mode::Input, Action::Accept, "choose"),
                    hint(Mode::Input, Action::Cancel, "cancel")
                ),
            )
        }
        Dialog::Visibility {
            reference,
            target,
            confirm,
        } => {
            let width = usize::from(popup_body_width(area, POPUP));
            let label = visibility_label(*target);
            (
                POPUP,
                format!("{label} / {}", escape(&reference.path)),
                vec![
                    Line::raw(escape(remote::visibility_question(reference, *target))),
                    Line::default(),
                    choice_line("Cancel", "", true, !confirm, width),
                    choice_line(label, "", true, *confirm, width),
                ],
                format!(
                    "{} focus · {} · {}",
                    super::rows_hint(bindings, Mode::Confirmation, false),
                    hint(Mode::Confirmation, Action::Accept, "choose"),
                    hint(Mode::Confirmation, Action::Cancel, "cancel")
                ),
            )
        }
    };
    super::draw_popup_in(
        frame,
        area,
        cap,
        &title,
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        footer,
    );
}
