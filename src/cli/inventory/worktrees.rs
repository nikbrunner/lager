//! The Worktrees popup of one inventory row: the list, Add through a name and its plan, and
//! Remove through its blockers or a confirmation. Every rule comes from the worktree service.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::keys::{Action, Bindings, Mode};
use super::{
    InventorySession, POPUP, Selection, TerminalSession, View, WIDE_POPUP, abbreviate, bar,
    choice_line, dim, discard_input, edit_text, exact_field_lines, field_lines, first_key_hint,
    focus, handoff_line, home_relative, model, popup_body_width, popup_regions, popup_scroll_limit,
    rows_hint, tint, wait_for_enter, warn,
};
use crate::application::ports::Desktop;
use crate::application::sessions;
use crate::application::worktrees::{
    self, AddPlan, Blocker, Placement, RemovalInspection, RemoveError, Repository, WorktreeRecord,
    short_id,
};
use crate::cli::worktree::{branch_label, kept_label, removal_losses};
use crate::domain::config::resolve_worktree_root;
use crate::infrastructure::config::load_worktrees;
use crate::infrastructure::desktop::NativeDesktop;
use crate::infrastructure::filesystem::Filesystem;
use crate::infrastructure::git::NativeGit;
use crate::infrastructure::rex::Rex;
use crate::infrastructure::session_record::SessionFile;
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) struct Popup {
    /// The short name of the row the popup was opened on.
    name: String,
    /// Git reports real paths, so `~` stands for the real HOME here.
    home: PathBuf,
    repository: Repository,
    /// Git's list as of the last change, each working tree with its state label and the key
    /// of its session record.
    rows: Vec<(WorktreeRecord, String, PathBuf)>,
    faults: Vec<String>,
    cursor: usize,
    stage: Stage,
}

enum Stage {
    List,
    Input {
        text: String,
        rejected: Option<String>,
    },
    Plan {
        text: String,
        plan: AddPlan,
        /// The cursor is on "Back to name" instead of "Create worktree".
        back: bool,
        /// Whether the session opens after the add; `None` when sessions are off.
        open: Option<bool>,
    },
    Blocked {
        path: PathBuf,
        blockers: Vec<Blocker>,
        scroll: u16,
        /// The label of the worktree's live session.
        session: Option<String>,
    },
    Confirm {
        inspection: RemovalInspection,
        /// The cursor is on the removing choice instead of Cancel.
        remove: bool,
        /// The label of the worktree's live session, and whether it closes after the removal.
        close: Option<(String, bool)>,
    },
}

/// A confirmed native Git operation; it runs with the terminal handed to Git. Each carries
/// the session choice of its sheet.
pub(super) enum Native {
    Add(AddPlan, bool),
    Remove(RemovalInspection, Option<(String, bool)>),
}

/// The last component of a worktree path, as the popup names the worktree.
fn dir_name(path: &Path) -> String {
    escape(
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy(),
    )
}

fn reason(label: &str, reason: &Option<String>) -> String {
    match reason {
        Some(reason) if !reason.is_empty() => format!("{label}: {}", escape(reason)),
        _ => label.to_owned(),
    }
}

impl Popup {
    pub(super) fn view(&self) -> View {
        match self.stage {
            Stage::List => View::Worktrees,
            Stage::Input { .. } => View::WorktreeInput,
            Stage::Plan { .. } => View::WorktreePlan,
            Stage::Blocked { .. } => View::WorktreeBlocked,
            Stage::Confirm { .. } => View::WorktreeConfirm,
        }
    }

    /// Reads Git's list and every working tree's local changes once; frames draw this snapshot.
    fn reload(&mut self) {
        let listing = worktrees::list(&GIT, &FILES, &self.repository);
        self.faults = listing
            .errors
            .iter()
            .map(|fault| match &fault.path {
                Some(path) => format!(
                    "{}: {}",
                    home_relative(path, &self.home),
                    escape(&fault.error)
                ),
                None => escape(&fault.error),
            })
            .collect();
        self.rows = listing
            .worktrees
            .into_iter()
            .map(|record| {
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
                match worktrees::local_changes(&GIT, &record) {
                    Some(Ok(losses)) if losses.modified + losses.untracked == 0 => {
                        if !record.locked {
                            states.push("clean".to_owned());
                        }
                    }
                    Some(Ok(losses)) => states.push(
                        [
                            (losses.modified, "modified"),
                            (losses.untracked, "untracked"),
                        ]
                        .into_iter()
                        .filter(|(count, _)| *count > 0)
                        .map(|(count, noun)| format!("{count} {noun}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    ),
                    Some(Err(_)) => states.push("changes unknown".to_owned()),
                    None => {}
                }
                // Named Git state is a loss too, so such a row is never called clean.
                match worktrees::named_state_of(&GIT, &FILES, &record) {
                    Some(Ok(named)) if !named.is_empty() => {
                        states.retain(|state| state != "clean");
                        states.push(format!(
                            "{} Git state{}",
                            named.len(),
                            if named.len() == 1 { "" } else { "s" }
                        ));
                    }
                    Some(Err(_)) => {
                        states.retain(|state| state != "clean");
                        states.push("Git state unknown".to_owned());
                    }
                    _ => {}
                }
                let state = states.join(" · ");
                let key = crate::cli::session::key(&record.path);
                (record, state, key)
            })
            .collect();
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn shown(&self, path: &Path) -> String {
        home_relative(path, &self.home)
    }
}

/// The session label of the working tree at `path`, a linked worktree unless it is the
/// primary checkout Git lists.
fn session_label(popup: &Popup, path: &Path) -> String {
    let primary = popup
        .rows
        .iter()
        .find(|(record, ..)| record.is_primary)
        .map_or(popup.repository.primary.as_path(), |(record, ..)| {
            record.path.as_path()
        });
    crate::cli::session::label(primary, path)
}

/// Opens the popup on `row`, or reports why the row has no worktrees to show.
pub(super) fn open(
    session: &mut InventorySession,
    selection: &mut Selection,
    row: Option<&model::InventoryRow>,
    name: String,
) {
    let Some(row) = row else {
        return;
    };
    let opened = if session.running() {
        Err("a batch is running".to_owned())
    } else if session.github.running() {
        Err("a GitHub action is running".to_owned())
    } else {
        model::worktree_checkout(row)
            .map_err(str::to_owned)
            .and_then(|path| worktrees::resolve_repository(&GIT, &FILES, path, true))
    };
    match opened {
        Ok(repository) => {
            let mut popup = Popup {
                name,
                home: std::fs::canonicalize(&session.home).unwrap_or(session.home.clone()),
                repository,
                rows: Vec::new(),
                faults: Vec::new(),
                cursor: 0,
                stage: Stage::List,
            };
            popup.reload();
            // Rows sort by path; the cursor starts on the primary wherever it sorts.
            popup.cursor = popup
                .rows
                .iter()
                .position(|(record, ..)| record.is_primary)
                .unwrap_or(0);
            selection.worktrees = Some(popup);
            // A session made outside this inventory gets its mark and its close line.
            session.list_sessions();
        }
        Err(reason) => {
            session.message = Some(format!("Worktrees unavailable: {}", escape(reason)));
        }
    }
}

/// Where the configuration places new worktrees, read when a plan is asked for.
fn worktree_root(config_path: &Path, home: &Path) -> Result<Option<PathBuf>, String> {
    if std::fs::symlink_metadata(config_path).is_err() {
        return Ok(None);
    }
    let (_, value, _) = load_worktrees(config_path).map_err(|error| error.to_string())?;
    value
        .map(|value| resolve_worktree_root(&value, home))
        .transpose()
        .map_err(|error| error.to_string())
}

fn plan(popup: &Popup, session: &InventorySession, name: &str) -> Result<AddPlan, String> {
    let worktree_root = worktree_root(&session.config_path, &popup.home)?;
    let request = worktrees::request_named(&GIT, &popup.repository, name)?;
    worktrees::plan_add(
        &GIT,
        &FILES,
        &popup.repository,
        &request,
        &Placement {
            root: Some(&session.root),
            worktree_root: worktree_root.as_deref(),
        },
    )
    .map_err(|error| match error {
        worktrees::AddError::Usage(message) | worktrees::AddError::Refused(message) => message,
    })
}

/// Applies one key to the open popup. Returns the native operation a confirmation released.
pub(super) fn key(
    session: &mut InventorySession,
    selection: &mut Selection,
    action: Option<Action>,
    key: KeyEvent,
    area: Rect,
) -> Option<Native> {
    let popup = selection.worktrees.as_mut()?;
    let cancelled = |what: &str| Some(format!("{what} cancelled · nothing changed"));
    // A choice that changes something acts only while what it confirms is on screen.
    let readable = sheet(popup, &session.bindings, area).is_none_or(|sheet| sheet.regions(area).3);
    let scroll_limit = match &popup.stage {
        Stage::Blocked {
            path,
            blockers,
            session,
            ..
        } => popup_scroll_limit(
            area,
            WIDE_POPUP,
            &blocked_body(
                popup,
                path,
                session.as_deref(),
                blockers,
                popup_body_width(area, WIDE_POPUP),
            ),
        ),
        _ => 0,
    };
    let stage = std::mem::replace(&mut popup.stage, Stage::List);
    popup.stage = match (stage, action) {
        (Stage::List, Some(Action::Cancel | Action::Accept)) => {
            selection.worktrees = None;
            return None;
        }
        (Stage::List, Some(action)) => {
            let last = popup.rows.len().saturating_sub(1);
            let hosted = session
                .hosted
                .as_ref()
                .and_then(|hosted| hosted.as_ref().ok());
            match (action, popup.rows.get(popup.cursor)) {
                (Action::Up, _) => popup.cursor = popup.cursor.saturating_sub(1),
                (Action::Down, _) => popup.cursor = (popup.cursor + 1).min(last),
                (Action::PageUp, _) => popup.cursor = popup.cursor.saturating_sub(10),
                (Action::PageDown, _) => popup.cursor = (popup.cursor + 10).min(last),
                (Action::Add, _) => {
                    popup.stage = Stage::Input {
                        text: String::new(),
                        rejected: None,
                    };
                    return None;
                }
                (Action::Copy, Some((record, ..))) => {
                    session.message = Some(match NativeDesktop.copy(record.path.as_os_str()) {
                        Ok(()) => format!("Copied path of {}", dir_name(&record.path)),
                        Err(reason) => format!("Copy unavailable: {}", escape(reason)),
                    });
                }
                (Action::Session, Some((record, _, key))) if session.session_layout.is_some() => {
                    let refused = match &session.hosted {
                        Some(Err(reason)) => Some(reason.clone()),
                        _ if session.opening.is_some() => Some("a session is opening".to_owned()),
                        _ if !record.path.is_dir() => {
                            Some("no checkout at the destination".to_owned())
                        }
                        _ => None,
                    };
                    match refused {
                        Some(reason) => {
                            session.message = Some(format!("Open session unavailable: {reason}"));
                        }
                        None => {
                            let label = session_label(popup, &record.path);
                            let checkout = (key.clone(), label.clone());
                            session.open_checkout(label, None, move || Ok(checkout));
                        }
                    }
                }
                (Action::Remove, Some((record, ..))) if record.is_primary => {
                    session.message = Some(
                        "Remove unavailable: the primary checkout is removed from the table, not here"
                            .to_owned(),
                    );
                }
                (Action::Remove, Some((record, ..))) => {
                    // Every guard runs here; nothing is deleted before the confirmation.
                    let inspection = worktrees::inspect_removal(
                        &GIT,
                        &FILES,
                        &record.path,
                        Some(&popup.repository.primary),
                        Some("in the worktree: git"),
                    );
                    let live = hosted
                        .and_then(|hosted| hosted.get(&inspection.path))
                        .cloned();
                    popup.stage = if inspection.blockers.is_empty() {
                        selection.preview_fresh = true;
                        Stage::Confirm {
                            inspection,
                            remove: false,
                            close: live.map(|label| (label, true)),
                        }
                    } else {
                        Stage::Blocked {
                            path: inspection.path,
                            blockers: inspection.blockers,
                            scroll: 0,
                            session: live,
                        }
                    };
                    return None;
                }
                _ => {}
            }
            Stage::List
        }
        (Stage::Input { text, .. }, Some(Action::Accept)) => match plan(popup, session, &text) {
            Ok(plan) => {
                selection.preview_fresh = true;
                Stage::Plan {
                    text,
                    plan,
                    back: false,
                    open: (session.session_layout.is_some()
                        && !matches!(session.hosted, Some(Err(_))))
                    .then_some(true),
                }
            }
            Err(reason) => Stage::Input {
                text,
                rejected: Some(reason),
            },
        },
        (Stage::Input { .. }, Some(Action::Cancel)) => {
            session.message = cancelled("Add worktree");
            Stage::List
        }
        (Stage::Input { mut text, .. }, None) => {
            edit_text(&mut text, key);
            Stage::Input {
                text,
                rejected: None,
            }
        }
        (
            Stage::Plan {
                text, plan, open, ..
            },
            Some(action @ (Action::Up | Action::Down)),
        ) => Stage::Plan {
            text,
            plan,
            back: action == Action::Down,
            open,
        },
        (
            Stage::Plan {
                text,
                plan,
                back,
                open: Some(open),
            },
            Some(Action::Session),
        ) => Stage::Plan {
            text,
            plan,
            back,
            open: Some(!open),
        },
        (stage @ Stage::Plan { back: false, .. }, Some(Action::Accept)) if !readable => {
            session.message =
                Some("Add unavailable: enlarge the terminal to show the whole plan".to_owned());
            stage
        }
        (
            Stage::Plan {
                text,
                plan,
                back: false,
                open,
            },
            Some(Action::Accept),
        ) => {
            popup.stage = Stage::Plan {
                text,
                plan: plan.clone(),
                back: false,
                open,
            };
            return Some(Native::Add(plan, open == Some(true)));
        }
        (Stage::Plan { text, .. }, Some(Action::Accept | Action::Cancel)) => Stage::Input {
            text,
            rejected: None,
        },
        (Stage::Blocked { .. }, Some(Action::Accept | Action::Cancel)) => Stage::List,
        (
            Stage::Blocked {
                path,
                blockers,
                scroll,
                session,
            },
            Some(action @ (Action::Up | Action::Down | Action::PageUp | Action::PageDown)),
        ) => Stage::Blocked {
            path,
            blockers,
            session,
            scroll: match action {
                Action::Up => scroll.saturating_sub(1),
                Action::Down => scroll.saturating_add(1),
                Action::PageUp => scroll.saturating_sub(10),
                _ => scroll.saturating_add(10),
            }
            .min(scroll_limit),
        },
        (stage @ Stage::Confirm { remove: true, .. }, Some(Action::Accept)) if !readable => {
            session.message = Some(
                "Remove unavailable: enlarge the terminal to show the whole summary".to_owned(),
            );
            stage
        }
        (
            Stage::Confirm {
                inspection, close, ..
            },
            Some(action @ (Action::Up | Action::Down)),
        ) => Stage::Confirm {
            inspection,
            remove: action == Action::Down,
            close,
        },
        (
            Stage::Confirm {
                inspection,
                remove,
                close: Some((label, tick)),
            },
            Some(Action::Session),
        ) => Stage::Confirm {
            inspection,
            remove,
            close: Some((label, !tick)),
        },
        (
            Stage::Confirm {
                inspection,
                remove: true,
                close,
            },
            Some(Action::Accept),
        ) => {
            popup.stage = Stage::Confirm {
                inspection: inspection.clone(),
                remove: true,
                close: close.clone(),
            };
            return Some(Native::Remove(inspection, close));
        }
        (Stage::Confirm { .. }, Some(Action::Accept | Action::Cancel)) => {
            session.message = cancelled("Remove");
            Stage::List
        }
        (stage, _) => stage,
    };
    None
}

/// Runs a confirmed operation with the terminal handed to Git, then returns to the list.
/// Returns the exit code when the operation was interrupted.
pub(super) fn run(
    terminal: &mut TerminalSession,
    session: &mut InventorySession,
    selection: &mut Selection,
    native: Native,
    interrupted: &dyn Fn() -> Option<i32>,
) -> Option<i32> {
    let popup = selection.worktrees.as_mut()?;
    popup.stage = Stage::List;
    terminal.suspend();
    // Git ran, or a line was awaited: what was typed there is not for the inventory.
    let mut handed_over = true;
    let failed = match native {
        Native::Add(plan, open) => {
            let destination = popup.shown(&plan.destination);
            // Keys typed before Git starts must not answer it or its hooks.
            discard_input();
            eprintln!();
            handoff_line(&format!("worktree add · {} → {destination}", popup.name));
            handoff_line(
                "Git owns this terminal until it returns. Ctrl-C interrupts it and exits 130.",
            );
            eprintln!();
            match worktrees::add(&GIT, &FILES, &plan) {
                Ok(()) => {
                    let created = format!(
                        "Created worktree {destination} on {}branch {}",
                        if plan.start.is_some() { "new " } else { "" },
                        escape(&plan.branch)
                    );
                    if !open {
                        session.message = Some(created);
                    } else if session.opening.is_some() {
                        session.message = Some(format!(
                            "{created} · Open session unavailable: a session is opening"
                        ));
                    } else {
                        let label = session_label(popup, &plan.destination);
                        let opened = label.clone();
                        session.open_checkout(label, Some(created), move || {
                            Ok((crate::cli::session::key(&plan.destination), opened))
                        });
                    }
                    None
                }
                Err(failure) => {
                    let remaining = if failure.remaining.is_empty() {
                        vec!["remaining: nothing".to_owned()]
                    } else {
                        failure
                            .remaining
                            .iter()
                            .map(|left| format!("remaining: {}", escape(left)))
                            .collect()
                    };
                    Some((
                        "Worktree was not created",
                        failure.message,
                        failure.cancelled,
                        remaining,
                    ))
                }
            }
        }
        Native::Remove(inspection, close) => {
            let shown = popup.shown(&inspection.path);
            // Yes on a summary with losses accepts exactly the losses it listed.
            let result = worktrees::remove(
                &GIT,
                &FILES,
                &inspection,
                inspection.lossy(),
                Some(&popup.repository.primary),
            );
            handed_over = matches!(result, Ok(()) | Err(RemoveError::Failed(_)));
            let kept_open = if matches!(close, Some((_, true))) {
                " · the session stays open"
            } else {
                ""
            };
            match result {
                Ok(()) => {
                    let kept = match &inspection.branch {
                        Some(branch) => format!("branch {} kept", branch_label(Some(branch), None)),
                        None => kept_label(&inspection),
                    };
                    let removed = format!("Removed worktree {shown} · {}", escape(kept));
                    session.message = Some(match close {
                        // The record is keyed by the canonical path read before the removal.
                        Some((_, true)) => {
                            match sessions::close(&Rex, &SessionFile, &inspection.path) {
                                Ok(Some(recorded)) => {
                                    format!(
                                        "{removed} · Closed session {}",
                                        escape(&recorded.label)
                                    )
                                }
                                Ok(None) => removed,
                                Err(error) => match session.host_error(error) {
                                    Ok(reason) => {
                                        format!("{removed} · Close session unavailable: {reason}")
                                    }
                                    Err(reason) => {
                                        let failed = format!("Close session failed: {reason}");
                                        session.problems.record("session", failed.clone());
                                        format!("! {removed} · {failed}")
                                    }
                                },
                            }
                        }
                        _ => removed,
                    });
                    None
                }
                Err(RemoveError::Stale) => {
                    session.message = Some(format!(
                        "the worktree changed after it was inspected; nothing removed{kept_open}"
                    ));
                    None
                }
                Err(RemoveError::Losses) => {
                    session.message = Some(format!(
                        "{} would be lost; nothing removed{kept_open}",
                        if inspection.git_state.is_empty() {
                            "local changes"
                        } else {
                            "the listed state"
                        }
                    ));
                    None
                }
                Err(RemoveError::Blocked(blockers)) => {
                    session.message = Some("Worktree removal blocked · nothing removed".to_owned());
                    popup.stage = Stage::Blocked {
                        path: inspection.path.clone(),
                        blockers,
                        scroll: 0,
                        session: close.map(|(label, _)| label),
                    };
                    None
                }
                // Git may have deleted files before it failed; say what is left, not what was meant.
                Err(RemoveError::Failed(failure)) => {
                    let left = worktrees::removal_remains(&GIT, &FILES, &inspection);
                    Some((
                        "Worktree removal failed",
                        format!("git worktree remove failed: {}", failure.message),
                        failure.cancelled,
                        if left.is_empty() {
                            vec!["remaining: nothing".to_owned()]
                        } else {
                            left.iter()
                                .map(|left| format!("remaining: {}", escape(left)))
                                .collect()
                        },
                    ))
                }
            }
        }
    };
    if let Some((what, message, cancelled, remaining)) = failed {
        let message = escape(message);
        eprintln!();
        handoff_line(&format!("failed · {}: {message}", what.to_lowercase()));
        remaining.iter().for_each(|line| handoff_line(line));
        // The terminal stays with the shell: an interrupted child ends the session.
        if cancelled || interrupted().is_some() {
            session.worker.cancel();
            return Some(interrupted().unwrap_or(130));
        }
        discard_input();
        handoff_line("Enter returns to the inventory. Ctrl-C exits 130.");
        if let Some(code) = wait_for_enter(interrupted) {
            session.worker.cancel();
            return Some(code);
        }
        session.message = Some(format!("{what}: {message} · {}", remaining.join(" · ")));
    }
    popup.reload();
    if let Err(error) = terminal.resume(handed_over) {
        eprintln!(
            "lager: could not restore inventory terminal: {}",
            escape(error)
        );
        return Some(1);
    }
    super::refresh(session);
    None
}

fn pad(text: String, width: usize) -> String {
    let gap = width.saturating_sub(Line::raw(text.as_str()).width());
    text + &" ".repeat(gap)
}

/// The heading and one line per working tree, cut to `width`, with the cursor row reversed.
/// With `hosted`, a slot before the path marks each worktree with a live session.
fn list_lines(
    popup: &Popup,
    hosted: Option<&BTreeMap<PathBuf, String>>,
    width: usize,
) -> Vec<Line<'static>> {
    let slot = if hosted.is_some() { 2 } else { 0 };
    let cells = popup
        .rows
        .iter()
        .map(|(record, state, _)| {
            [
                popup.shown(&record.path),
                escape(branch_label(
                    record.branch.as_deref(),
                    record.head.as_deref(),
                )),
                state.clone(),
            ]
        })
        .collect::<Vec<_>>();
    let widest = |column: usize, heading: &str| {
        cells
            .iter()
            .map(|row| Line::raw(row[column].as_str()).width())
            .chain([heading.len()])
            .max()
            .unwrap_or(0)
    };
    let branch = widest(1, "branch").min(18);
    // The path keeps its tail and leaves the state room for a change summary.
    let state = widest(2, "state").min(24);
    let path = widest(0, "path").min(width.saturating_sub(branch + state + 4 + slot));
    let state = width.saturating_sub(path + branch + 4 + slot);
    let row = |cells: [String; 3]| {
        let [first, second, third] = cells;
        [
            pad(abbreviate(&first, path, true), path + 2),
            pad(abbreviate(&second, branch, false), branch + 2),
            pad(abbreviate(&third, state, false), state),
        ]
    };
    let mut lines = vec![Line::styled(
        " ".repeat(slot) + &row(["path", "branch", "state"].map(str::to_owned)).concat(),
        dim(),
    )];
    lines.extend(cells.into_iter().enumerate().map(|(index, cells)| {
        let (record, state, key) = &popup.rows[index];
        let mark = match hosted {
            Some(hosted) if hosted.contains_key(key) => "● ",
            Some(_) => "  ",
            None => "",
        };
        if index == popup.cursor {
            return Line::styled(
                mark.to_owned() + &row(cells).concat(),
                Style::default().add_modifier(Modifier::REVERSED),
            );
        }
        let style = if record.locked || record.prunable {
            warn()
        } else if state.ends_with("clean") {
            focus()
        } else {
            Style::default()
        };
        let [path, branch, state] = row(cells);
        Line::from(vec![
            Span::styled(mark, focus()),
            Span::raw(path),
            Span::raw(branch),
            Span::styled(state, style),
        ])
    }));
    lines
}

fn list_footer(bindings: &Bindings, sessions: bool) -> String {
    let hint = |action, label| first_key_hint(bindings, Mode::Worktrees, action, label);
    [
        format!("{} select", rows_hint(bindings, Mode::Worktrees)),
        if sessions {
            hint(Action::Session, "session")
        } else {
            String::new()
        },
        hint(Action::Add, "add"),
        hint(Action::Remove, "remove"),
        hint(Action::Copy, "copy"),
        hint(Action::Cancel, "close"),
    ]
    .into_iter()
    .filter(|hint| !hint.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

fn draw_list(
    frame: &mut ratatui::Frame<'_>,
    popup: &Popup,
    bindings: &Bindings,
    hosted: Option<&BTreeMap<PathBuf, String>>,
    area: Rect,
) {
    let mut intro = popup
        .faults
        .iter()
        .map(|fault| Line::styled(format!("! the list is incomplete · {fault}"), warn()))
        .collect::<Vec<_>>();
    if !intro.is_empty() {
        intro.push(Line::default());
    }
    let list = list_lines(
        popup,
        hosted,
        usize::from(popup_body_width(area, WIDE_POPUP)),
    );
    let sizing = [intro.clone(), list.clone()].concat();
    let [frame_area, title, content, footer] = popup_regions(
        area,
        WIDE_POPUP,
        &Paragraph::new(sizing).wrap(Wrap { trim: false }),
    );
    let intro = Paragraph::new(intro).wrap(Wrap { trim: false });
    let [intro_area, list_area] = Layout::vertical([
        Constraint::Length(intro.line_count(content.width) as u16),
        Constraint::Min(2),
    ])
    .areas(content);
    // The heading stays; the rows scroll so the cursor row is the last one in view at most.
    let room = usize::from(list_area.height.saturating_sub(1)).max(1);
    let skipped = (popup.cursor + 1).saturating_sub(room);
    let mut rows = list;
    rows.drain(1..1 + skipped.min(rows.len() - 1));
    frame.render_widget(Clear, frame_area);
    frame.render_widget(Block::bordered().border_style(focus()), frame_area);
    frame.render_widget(
        bar(&format!("Worktrees / {}", popup.name), "lager", title.width),
        title,
    );
    frame.render_widget(intro, intro_area);
    frame.render_widget(Paragraph::new(rows), list_area);
    frame.render_widget(
        Paragraph::new(list_footer(bindings, hosted.is_some())).style(dim()),
        footer,
    );
}

/// Draws the popup's current stage inside `area`, which leaves the status line readable.
/// `hosted` holds the live sessions once the host answered.
pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    popup: &Popup,
    bindings: &Bindings,
    hosted: Option<&BTreeMap<PathBuf, String>>,
    area: Rect,
) {
    let (cap, title, lines, footer) = match &popup.stage {
        Stage::List => return draw_list(frame, popup, bindings, hosted, area),
        Stage::Input { text, rejected } => {
            let mut lines = vec![
                Line::styled("name", dim()),
                Line::styled(format!("{}█", escape(text)), focus()),
            ];
            if let Some(reason) = rejected {
                lines.push(Line::styled(
                    format!("Rejected: {}", escape(reason)),
                    tint(Color::Red),
                ));
            }
            (
                POPUP,
                format!("Add worktree / {}", popup.name),
                lines,
                format!(
                    "{} · {}",
                    first_key_hint(bindings, Mode::Input, Action::Accept, "preview"),
                    first_key_hint(bindings, Mode::Input, Action::Cancel, "back")
                ),
            )
        }
        Stage::Blocked {
            path,
            blockers,
            scroll,
            session,
        } => {
            let body = blocked_body(
                popup,
                path,
                session.as_deref(),
                blockers,
                popup_body_width(area, WIDE_POPUP),
            );
            let scroll = (*scroll).min(popup_scroll_limit(area, WIDE_POPUP, &body));
            return super::draw_popup_in(
                frame,
                area,
                WIDE_POPUP,
                "Remove worktree / blocked",
                body.scroll((scroll, 0)),
                format!(
                    "{} scroll · {}",
                    rows_hint(bindings, Mode::Inspection),
                    first_key_hint(bindings, Mode::Inspection, Action::Cancel, "back")
                ),
            );
        }
        Stage::Plan { .. } | Stage::Confirm { .. } => {
            let Some(sheet) = sheet(popup, bindings, area) else {
                return;
            };
            let ([popup_area, title, _, footer], fields, choices, readable) = sheet.regions(area);
            frame.render_widget(Clear, popup_area);
            frame.render_widget(Block::bordered().border_style(focus()), popup_area);
            frame.render_widget(bar(&sheet.title, "lager", title.width), title);
            frame.render_widget(
                Paragraph::new([sheet.required, sheet.optional].concat())
                    .wrap(Wrap { trim: false }),
                fields,
            );
            frame.render_widget(Paragraph::new(sheet.choices), choices);
            let cancel = first_key_hint(bindings, Mode::Confirmation, Action::Cancel, "cancel");
            frame.render_widget(
                if readable {
                    Paragraph::new(sheet.footer).style(dim())
                } else {
                    Paragraph::new(format!(
                        "Too small to choose here · enlarge the terminal · {cancel}"
                    ))
                    .style(warn())
                },
                footer,
            );
            return;
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

fn blocked_body(
    popup: &Popup,
    path: &Path,
    session: Option<&str>,
    blockers: &[Blocker],
    width: u16,
) -> Paragraph<'static> {
    let mut lines = exact_field_lines("path", &popup.shown(path), Style::default(), width);
    if let Some(label) = session {
        lines.extend(open_session_line(label, width));
    }
    for blocker in blockers {
        lines.extend(field_lines(
            "blocked",
            &escape(&blocker.message),
            tint(Color::Red),
            width,
        ));
        if let Some(hint) = &blocker.hint {
            lines.extend(exact_field_lines("hint", &escape(hint), dim(), width));
        }
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// A plan or a removal summary: the lines its changing choice needs on screen, the lines that
/// give way first, and the choices pinned below them.
struct Sheet {
    title: String,
    required: Vec<Line<'static>>,
    optional: Vec<Line<'static>>,
    choices: Vec<Line<'static>>,
    footer: String,
}

impl Sheet {
    /// The popup regions, the fields and choices areas, and whether the required lines and
    /// every choice are in view.
    fn regions(&self, area: Rect) -> ([Rect; 4], Rect, Rect, bool) {
        let wrapped =
            |lines: &[Line<'static>]| Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false });
        let sizing = [
            self.required.clone(),
            self.optional.clone(),
            self.choices.clone(),
        ]
        .concat();
        let regions = popup_regions(area, WIDE_POPUP, &wrapped(&sizing));
        let content = regions[2];
        let [fields, choices] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length((self.choices.len() as u16).min(content.height)),
        ])
        .areas(content);
        let readable = usize::from(choices.height) == self.choices.len()
            && wrapped(&self.required).line_count(fields.width) <= usize::from(fields.height);
        (regions, fields, choices, readable)
    }
}

fn sheet(popup: &Popup, bindings: &Bindings, area: Rect) -> Option<Sheet> {
    let width = popup_body_width(area, WIDE_POPUP);
    let hint = |action, label| first_key_hint(bindings, Mode::Confirmation, action, label);
    let rows = rows_hint(bindings, Mode::Confirmation);
    let session_hint = |offered: bool| {
        let hint = hint(Action::Session, "session");
        if offered && !hint.is_empty() {
            format!(" · {hint}")
        } else {
            String::new()
        }
    };
    let tick = |label: &str, ticked: bool| format!("[{}] {label}", if ticked { "x" } else { " " });
    match &popup.stage {
        Stage::Plan {
            plan, back, open, ..
        } => {
            let branch = match &plan.start {
                Some(start) => format!("{} · new, at {}", escape(&plan.branch), short_id(start)),
                None => format!("{} · existing", escape(&plan.branch)),
            };
            let mut required = exact_field_lines(
                "path",
                &popup.shown(&plan.destination),
                Style::default(),
                width,
            );
            required.extend(exact_field_lines(
                "branch",
                &branch,
                Style::default(),
                width,
            ));
            let mut choices = Vec::new();
            if let Some(open) = open {
                required.extend(exact_field_lines(
                    "session",
                    &escape(session_label(popup, &plan.destination)),
                    Style::default(),
                    width,
                ));
                choices.push(choice_line(
                    &tick("Open session", *open),
                    "",
                    true,
                    false,
                    usize::from(width),
                ));
            }
            choices.extend([
                choice_line("Create worktree", "", true, !back, usize::from(width)),
                choice_line("Back to name", "", true, *back, usize::from(width)),
            ]);
            Some(Sheet {
                title: "Add worktree / plan".to_owned(),
                required,
                optional: vec![Line::default()],
                choices,
                footer: format!(
                    "{rows} select{} · {} · {}",
                    session_hint(open.is_some()),
                    hint(Action::Accept, "choose"),
                    hint(Action::Cancel, "back")
                ),
            })
        }
        Stage::Confirm {
            inspection,
            remove,
            close,
        } => {
            let lossy = inspection.lossy();
            // The losses, the exact path in full and the kept branch all have to be in view.
            let mut required = Vec::new();
            for (index, loss) in removal_losses(inspection).iter().enumerate() {
                required.extend(field_lines(
                    if index == 0 { "losses" } else { "" },
                    loss,
                    if lossy { warn() } else { focus() },
                    width,
                ));
            }
            required.extend(exact_field_lines(
                "path",
                &popup.shown(&inspection.path),
                Style::default(),
                width,
            ));
            required.extend(exact_field_lines(
                "branch",
                &escape(kept_label(inspection)),
                Style::default(),
                width,
            ));
            let mut choices = Vec::new();
            if let Some((label, ticked)) = close {
                required.extend(open_session_line(label, width));
                choices.push(choice_line(
                    &tick("Close session", *ticked),
                    "ends its programs",
                    true,
                    false,
                    usize::from(width),
                ));
            }
            choices.extend([
                choice_line("Cancel", "", true, !remove, usize::from(width)),
                choice_line(
                    if lossy {
                        "Accept these losses and remove this worktree"
                    } else {
                        "Remove this worktree"
                    },
                    "cannot be undone",
                    true,
                    *remove,
                    usize::from(width),
                ),
            ]);
            Some(Sheet {
                title: format!("Remove worktree / {}", dir_name(&inspection.path)),
                required,
                optional: vec![Line::default()],
                choices,
                footer: format!(
                    "{rows} focus{} · {} · {}",
                    session_hint(close.is_some()),
                    hint(Action::Accept, "choose"),
                    hint(Action::Cancel, "cancel")
                ),
            })
        }
        _ => None,
    }
}

/// The `session` field of a worktree whose session is open.
fn open_session_line(label: &str, width: u16) -> Vec<Line<'static>> {
    exact_field_lines(
        "session",
        &format!("{} · open", escape(label)),
        Style::default(),
        width,
    )
}
