//! The Worktrees popup of one inventory row: the list, Add through a name and its plan, and
//! Remove through its blockers or a confirmation. Every rule comes from the worktree service.

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
use crate::presentation::escape;

const GIT: NativeGit = NativeGit;
const FILES: Filesystem = Filesystem;

pub(super) struct Popup {
    /// The short name of the row the popup was opened on.
    name: String,
    /// Git reports real paths, so `~` stands for the real HOME here.
    home: PathBuf,
    repository: Repository,
    identity: Option<String>,
    /// Git's list as of the last change, each working tree with its state label.
    rows: Vec<(WorktreeRecord, String)>,
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
    },
    Blocked {
        path: PathBuf,
        blockers: Vec<Blocker>,
        scroll: u16,
    },
    Confirm {
        inspection: RemovalInspection,
        /// The cursor is on the removing choice instead of Cancel.
        remove: bool,
    },
}

/// A confirmed native Git operation; it runs with the terminal handed to Git.
pub(super) enum Native {
    Add(AddPlan),
    Remove(RemovalInspection),
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
        self.identity = listing
            .worktrees
            .first()
            .and_then(|record| record.repository_identity.clone());
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
                (record, state)
            })
            .collect();
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn shown(&self, path: &Path) -> String {
        home_relative(path, &self.home)
    }
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
                identity: None,
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
                .position(|(record, _)| record.is_primary)
                .unwrap_or(0);
            selection.worktrees = Some(popup);
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
        Stage::Blocked { path, blockers, .. } => popup_scroll_limit(
            area,
            WIDE_POPUP,
            &blocked_body(popup, path, blockers, popup_body_width(area, WIDE_POPUP)),
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
            let record = popup.rows.get(popup.cursor).map(|(record, _)| record);
            match (action, record) {
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
                (Action::Copy, Some(record)) => {
                    session.message = Some(match NativeDesktop.copy(record.path.as_os_str()) {
                        Ok(()) => format!("Copied path of {}", dir_name(&record.path)),
                        Err(reason) => format!("Copy unavailable: {}", escape(reason)),
                    });
                }
                (Action::Remove, Some(record)) if record.is_primary => {
                    session.message = Some(
                        "Remove unavailable: the primary checkout is removed from the table, not here"
                            .to_owned(),
                    );
                }
                (Action::Remove, Some(record)) => {
                    // Every guard runs here; nothing is deleted before the confirmation.
                    let inspection = worktrees::inspect_removal(
                        &GIT,
                        &FILES,
                        &record.path,
                        Some(&popup.repository.primary),
                        Some("in the worktree: git"),
                    );
                    popup.stage = if inspection.blockers.is_empty() {
                        selection.preview_fresh = true;
                        Stage::Confirm {
                            inspection,
                            remove: false,
                        }
                    } else {
                        Stage::Blocked {
                            path: inspection.path,
                            blockers: inspection.blockers,
                            scroll: 0,
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
        (Stage::Plan { text, plan, .. }, Some(action @ (Action::Up | Action::Down))) => {
            Stage::Plan {
                text,
                plan,
                back: action == Action::Down,
            }
        }
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
            },
            Some(Action::Accept),
        ) => {
            popup.stage = Stage::Plan {
                text,
                plan: plan.clone(),
                back: false,
            };
            return Some(Native::Add(plan));
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
            },
            Some(action @ (Action::Up | Action::Down | Action::PageUp | Action::PageDown)),
        ) => Stage::Blocked {
            path,
            blockers,
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
        (Stage::Confirm { inspection, .. }, Some(action @ (Action::Up | Action::Down))) => {
            Stage::Confirm {
                inspection,
                remove: action == Action::Down,
            }
        }
        (
            Stage::Confirm {
                inspection,
                remove: true,
            },
            Some(Action::Accept),
        ) => {
            popup.stage = Stage::Confirm {
                inspection: inspection.clone(),
                remove: true,
            };
            return Some(Native::Remove(inspection));
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
        Native::Add(plan) => {
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
                    session.message = Some(format!(
                        "Created worktree {destination} on {}branch {}",
                        if plan.start.is_some() { "new " } else { "" },
                        escape(&plan.branch)
                    ));
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
        Native::Remove(inspection) => {
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
            match result {
                Ok(()) => {
                    let kept = match &inspection.branch {
                        Some(branch) => format!("branch {} kept", branch_label(Some(branch), None)),
                        None => kept_label(&inspection),
                    };
                    session.message = Some(format!("Removed worktree {shown} · {}", escape(kept)));
                    None
                }
                Err(RemoveError::Stale) => {
                    session.message = Some(
                        "the worktree changed after it was inspected; nothing removed".to_owned(),
                    );
                    None
                }
                Err(RemoveError::Losses) => {
                    session.message = Some(format!(
                        "{} would be lost; nothing removed",
                        if inspection.git_state.is_empty() {
                            "local changes"
                        } else {
                            "the listed state"
                        }
                    ));
                    None
                }
                Err(RemoveError::Blocked(blockers)) => {
                    popup.stage = Stage::Blocked {
                        path: inspection.path.clone(),
                        blockers,
                        scroll: 0,
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
fn list_lines(popup: &Popup, width: usize) -> Vec<Line<'static>> {
    let cells = popup
        .rows
        .iter()
        .map(|(record, state)| {
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
    let path = widest(0, "path").min(width.saturating_sub(branch + state + 4));
    let state = width.saturating_sub(path + branch + 4);
    let row = |cells: [String; 3]| {
        let [first, second, third] = cells;
        [
            pad(abbreviate(&first, path, true), path + 2),
            pad(abbreviate(&second, branch, false), branch + 2),
            pad(abbreviate(&third, state, false), state),
        ]
    };
    let mut lines = vec![Line::styled(
        row(["path", "branch", "state"].map(str::to_owned)).concat(),
        dim(),
    )];
    lines.extend(cells.into_iter().enumerate().map(|(index, cells)| {
        let (record, state) = &popup.rows[index];
        if index == popup.cursor {
            return Line::styled(
                row(cells).concat(),
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
            Span::raw(path),
            Span::raw(branch),
            Span::styled(state, style),
        ])
    }));
    lines
}

fn list_footer(bindings: &Bindings) -> String {
    let hint = |action, label| first_key_hint(bindings, Mode::Worktrees, action, label);
    [
        format!("{} select", rows_hint(bindings, Mode::Worktrees, true)),
        hint(Action::Add, "add worktree…"),
        hint(Action::Remove, "remove"),
        hint(Action::Copy, "copy path"),
        hint(Action::Cancel, "close"),
    ]
    .into_iter()
    .filter(|hint| !hint.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

fn draw_list(frame: &mut ratatui::Frame<'_>, popup: &Popup, bindings: &Bindings, area: Rect) {
    let linked = popup.rows.len().saturating_sub(1);
    let mut intro = vec![Line::styled(
        format!(
            "{} · {linked} linked worktree{} · listed by Git, including ones placed elsewhere",
            popup
                .identity
                .as_deref()
                .map_or("No origin".to_owned(), escape),
            if linked == 1 { "" } else { "s" }
        ),
        dim(),
    )];
    intro.extend(
        popup
            .faults
            .iter()
            .map(|fault| Line::styled(format!("! the list is incomplete · {fault}"), warn())),
    );
    intro.push(Line::default());
    let list = list_lines(popup, usize::from(popup_body_width(area, WIDE_POPUP)));
    let mut tail = vec![Line::default()];
    if let Some((record, state)) = popup.rows.get(popup.cursor) {
        let name = dir_name(&record.path);
        let remove = bindings.keys(Mode::Worktrees, Action::Remove);
        let remove = remove.first();
        tail.push(Line::raw(if record.is_primary {
            format!("{name} · primary checkout")
        } else if record.locked || record.prunable {
            match remove {
                Some(key) => format!("{name} · {state} · {key} reports the blocker"),
                None => format!("{name} · {state}"),
            }
        } else {
            let kept = match (&record.branch, &record.head) {
                (Some(branch), _) => {
                    format!(
                        "branch {} is kept",
                        escape(branch_label(Some(branch), None))
                    )
                }
                (None, Some(head)) => format!("detached HEAD {}", short_id(head)),
                (None, None) => "HEAD is unknown".to_owned(),
            };
            match remove {
                Some(key) => {
                    format!("{name} · {key} removes it after listing its losses · {kept}")
                }
                None => format!("{name} · {kept}"),
            }
        }));
    }
    tail.push(Line::styled(
        "The primary checkout is removed from the table, not here, and stays blocked while linked worktrees exist.",
        dim(),
    ));
    let sizing = [intro.clone(), list.clone(), tail.clone()].concat();
    let [frame_area, title, content, footer] = popup_regions(
        area,
        WIDE_POPUP,
        &Paragraph::new(sizing).wrap(Wrap { trim: false }),
    );
    let intro = Paragraph::new(intro).wrap(Wrap { trim: false });
    let tail = Paragraph::new(tail).wrap(Wrap { trim: false });
    let [intro_area, list_area, tail_area] = Layout::vertical([
        Constraint::Length(intro.line_count(content.width) as u16),
        Constraint::Min(2),
        Constraint::Length(tail.line_count(content.width) as u16),
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
    frame.render_widget(tail, tail_area);
    frame.render_widget(Paragraph::new(list_footer(bindings)).style(dim()), footer);
}

/// Draws the popup's current stage inside `area`, which leaves the status line readable.
pub(super) fn draw(frame: &mut ratatui::Frame<'_>, popup: &Popup, bindings: &Bindings, area: Rect) {
    let (cap, title, lines, footer) = match &popup.stage {
        Stage::List => return draw_list(frame, popup, bindings, area),
        Stage::Input { text, rejected } => {
            let width = usize::from(popup_body_width(area, POPUP));
            let mut lines = vec![
                Line::raw("Worktree name"),
                Line::styled(format!("{}█", escape(text)), focus()),
            ];
            if let Some(reason) = rejected {
                lines.push(Line::styled(
                    format!("Rejected: {}", escape(reason)),
                    tint(Color::Red),
                ));
            }
            lines.extend([
                Line::styled(
                    "Letters, digits, `.`, `_` or `-`. A local branch of that name is checked out; otherwise a new branch of that name starts at the primary checkout's HEAD.",
                    dim(),
                ),
                Line::styled("Nothing is created yet.", dim()),
                Line::default(),
                choice_line(
                    "Preview plan",
                    &bindings.keys(Mode::Input, Action::Accept).join("/"),
                    true,
                    true,
                    width,
                ),
                choice_line(
                    "Cancel",
                    &bindings.keys(Mode::Input, Action::Cancel).join("/"),
                    true,
                    false,
                    width,
                ),
            ]);
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
        } => {
            let body = blocked_body(popup, path, blockers, popup_body_width(area, WIDE_POPUP));
            let scroll = (*scroll).min(popup_scroll_limit(area, WIDE_POPUP, &body));
            return super::draw_popup_in(
                frame,
                area,
                WIDE_POPUP,
                "Remove worktree / blocked",
                body.scroll((scroll, 0)),
                format!(
                    "{} scroll · {}",
                    rows_hint(bindings, Mode::Inspection, true),
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
    blockers: &[Blocker],
    width: u16,
) -> Paragraph<'static> {
    let mut lines = exact_field_lines("path", &popup.shown(path), Style::default(), width);
    lines.push(Line::default());
    for blocker in blockers {
        lines.push(Line::styled(
            format!("blocked: {}", escape(&blocker.message)),
            tint(Color::Red),
        ));
        if let Some(hint) = &blocker.hint {
            lines.push(Line::styled(format!("hint: {}", escape(hint)), dim()));
        }
    }
    lines.extend([
        Line::default(),
        Line::styled(
            "Nothing was removed. Accepting losses does not waive a blocker.",
            dim(),
        ),
    ]);
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
    let rows = rows_hint(bindings, Mode::Confirmation, true);
    match &popup.stage {
        Stage::Plan { plan, back, .. } => {
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
            Some(Sheet {
                title: "Add worktree / plan".to_owned(),
                required,
                optional: vec![
                    Line::styled(
                        "Git creates the worktree and prints its own output. Lager copies no files into it.",
                        dim(),
                    ),
                    Line::default(),
                ],
                choices: vec![
                    choice_line(
                        "Create worktree",
                        "default",
                        true,
                        !back,
                        usize::from(width),
                    ),
                    choice_line(
                        "Back to name",
                        &bindings.keys(Mode::Confirmation, Action::Cancel).join("/"),
                        true,
                        *back,
                        usize::from(width),
                    ),
                ],
                footer: format!(
                    "{rows} select · {} · {}",
                    hint(Action::Accept, "choose"),
                    hint(Action::Cancel, "back")
                ),
            })
        }
        Stage::Confirm { inspection, remove } => {
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
            Some(Sheet {
                title: format!("Remove worktree / {}", dir_name(&inspection.path)),
                required,
                optional: vec![Line::default()],
                choices: vec![
                    choice_line("Cancel", "default", true, !remove, usize::from(width)),
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
                ],
                footer: format!(
                    "{rows} focus · {} · {}",
                    hint(Action::Accept, "choose"),
                    hint(Action::Cancel, "cancel")
                ),
            })
        }
        _ => None,
    }
}
