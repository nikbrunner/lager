use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
};

use crate::application::batch::{
    self, AddChoice, AddPreview, Batch, BatchAction, BatchTarget, Eligibility, Outcome,
};
use crate::application::inventory::{
    self as model, CopyField, InventoryFilesystem, InventoryReport, LocalScan, Marks,
    SessionProblems,
};
use crate::application::ports::{Desktop, EnsureEvent, EnsureReporter, HookRunner};
use crate::domain::repository::validate_reference_safety;
use crate::infrastructure::config::{CommandConfigStore, load_inventory};
use crate::infrastructure::desktop::NativeDesktop;
use crate::infrastructure::filesystem::Filesystem;
use crate::infrastructure::git::NativeGit;
use crate::infrastructure::inventory::{
    LocalInventoryFilesystem, ScanContext, ScanEvent, ScanHandle, start_local_scan_with_context,
};
use crate::infrastructure::shell::{Shell, ShellError};
use crate::presentation::escape;

use super::args::InventoryArgs;

mod keys;
use keys::{Action, Bindings, Mode};

pub fn run(config_path: &Path, args: InventoryArgs, interrupted: impl Fn() -> Option<i32>) -> i32 {
    if args.include_archived && !args.remote {
        eprintln!("lager: --include-archived requires --remote");
        return 2;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("lager: inventory requires a TTY on stdin and stdout");
        return 2;
    }
    if args.remote {
        eprintln!("lager: inventory remote discovery is unavailable in this build");
        return 2;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let (config, overrides, warnings) = match load_inventory(config_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let bindings = match Bindings::load(overrides) {
        Ok(bindings) => bindings,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let root = match config.resolve_root(&home) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let (mut terminal, mut session) = match start_with_loading(
        TerminalSession::enter,
        TerminalSession::draw_loading,
        || {
            let mut session = InventorySession::new(config, config_path.to_path_buf(), home, root);
            session.bindings = bindings;
            session.notice = warning_notice(warnings);
            session
        },
    ) {
        Ok(started) => started,
        Err(StartupError::Enter(error)) => {
            eprintln!(
                "lager: could not start inventory terminal: {}",
                escape(error)
            );
            return 1;
        }
        Err(StartupError::Draw(error)) => {
            eprintln!("lager: terminal render failed: {}", escape(error));
            return 1;
        }
    };
    terminal.run(&mut session, &interrupted)
}

struct InventorySession {
    config: crate::domain::config::Config,
    config_path: PathBuf,
    home: PathBuf,
    root: PathBuf,
    scan: LocalScan,
    worker: ScanHandle,
    generation: u64,
    complete: bool,
    problems: SessionProblems,
    marks: Marks,
    bindings: Bindings,
    notice: Option<String>,
    /// The outcome of the last action; the next key press clears it.
    message: Option<String>,
    cached_report: Option<InventoryReport>,
    /// The running or last finished batch; a new batch replaces it.
    batch: Option<Batch>,
}

impl InventorySession {
    fn new(
        config: crate::domain::config::Config,
        config_path: PathBuf,
        home: PathBuf,
        root: PathBuf,
    ) -> Self {
        let worker = start_local_scan_with_context(ScanContext {
            generation: 1,
            config_revision: LocalInventoryFilesystem.config_revision(&config_path),
            root: root.clone(),
        });
        Self {
            config,
            config_path,
            home,
            root,
            scan: LocalScan::default(),
            worker,
            generation: 1,
            complete: false,
            problems: SessionProblems::default(),
            marks: Marks::default(),
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            message: None,
            cached_report: None,
            batch: None,
        }
    }

    /// Inventory rows are a snapshot of scan events, not a reason to probe the filesystem again
    /// for every terminal frame. Refreshing and receiving an event invalidate this cache.
    fn report(&mut self) -> Result<InventoryReport, String> {
        if self.cached_report.is_none() {
            let report = model::build_inventory(
                &LocalInventoryFilesystem,
                &self.config,
                &self.config_path,
                &self.home,
                self.scan.clone(),
            )?;
            self.cached_report = Some(report);
        }
        Ok(self
            .cached_report
            .as_ref()
            .expect("report was cached")
            .clone())
    }

    fn event_is_current(&self, context: &ScanContext) -> bool {
        context.generation == self.generation
            && context.root == self.root
            && context.config_revision
                == LocalInventoryFilesystem.config_revision(&self.config_path)
    }

    fn receive_events(&mut self) {
        let mut received = false;
        while let Ok(event) = self.worker.receiver.try_recv() {
            received = true;
            match &event {
                ScanEvent::Failure { path, message } => {
                    let (field, error) = message.split_once(": ").unwrap_or(("status", message));
                    self.problems.record(
                        "scan",
                        format!(
                            "{}: {} · {field} not observed",
                            home_relative(path, &self.home),
                            escape(model::problem_reason(error))
                        ),
                    )
                }
                ScanEvent::Diagnostic { diagnostic, .. } if diagnostic.incomplete => {
                    self.problems.record(
                        "scan",
                        format!(
                            "{}: {} · the scan is incomplete",
                            home_relative(&diagnostic.path, &self.home),
                            escape(model::problem_reason(&diagnostic.message))
                        ),
                    )
                }
                _ => {}
            }
            match event {
                ScanEvent::Discovered {
                    path,
                    context,
                    target,
                } if self.event_is_current(&context) && target.matches_current() => {
                    self.scan.pending_paths.push(path)
                }
                ScanEvent::Checkout {
                    checkout,
                    context,
                    target,
                    content_current,
                } if self.event_is_current(&context)
                    && content_current
                    && target.matches_current() =>
                {
                    self.scan
                        .checkouts
                        .retain(|previous| previous.path != checkout.path);
                    self.scan.checkouts.push(checkout)
                }
                ScanEvent::Checkout {
                    context, target, ..
                } if self.event_is_current(&context) => self.reject_changed_target(target.path),
                ScanEvent::Diagnostic {
                    diagnostic,
                    context,
                } if self.event_is_current(&context) => {
                    self.scan.incomplete |= diagnostic.incomplete;
                    self.scan.diagnostics.push(diagnostic);
                }
                ScanEvent::Complete {
                    incomplete,
                    context,
                } if self.event_is_current(&context) => {
                    self.complete = true;
                    self.scan.complete = true;
                    self.scan.incomplete |= incomplete;
                    if !self.scan.incomplete {
                        self.scan
                            .checkouts
                            .retain(|checkout| self.scan.pending_paths.contains(&checkout.path));
                    }
                }
                _ => {} // Generation/config/target changed while a background event was in flight.
            }
        }
        if received {
            self.cached_report = None;
        }
    }

    fn reject_changed_target(&mut self, path: PathBuf) {
        self.scan.pending_paths.retain(|pending| pending != &path);
        self.scan.checkouts.retain(|checkout| checkout.path != path);
        if !self.scan.stale_paths.contains(&path) {
            self.scan.stale_paths.push(path);
        }
    }

    fn refresh(&mut self) -> Result<(), String> {
        let (config, overrides, warnings) =
            load_inventory(&self.config_path).map_err(|error| error.to_string())?;
        let bindings = Bindings::load(overrides)?;
        let root = config
            .resolve_root(&self.home)
            .map_err(|error| error.to_string())?;
        // Do not install a new config while the old worker can still emit against it.
        self.worker.cancel_and_join();
        self.receive_events();
        self.generation += 1;
        self.config = config;
        self.bindings = bindings;
        self.notice = warning_notice(warnings);
        self.cached_report = None;
        self.root = root.clone();
        self.scan = stale_scan(&self.scan);
        self.worker = start_local_scan_with_context(ScanContext {
            generation: self.generation,
            config_revision: LocalInventoryFilesystem.config_revision(&self.config_path),
            root,
        });
        self.complete = false;
        Ok(())
    }

    /// Runs the next batch target, records its blocker or failure, and refreshes once the batch
    /// is finished.
    /// Returns whether the step started a native child.
    fn step_batch(&mut self) -> bool {
        let Some(batch) = self.batch.as_mut() else {
            return false;
        };
        let store = CommandConfigStore::quiet();
        let mut started = false;
        let stepped = if batch.action.clones() {
            let active = batch.active();
            let mut banner = HandoffBanner {
                position: active.map_or(0, |index| index + 1),
                total: batch.targets.len(),
                identity: active
                    .and_then(|index| batch.targets[index].identity.as_deref())
                    .map(escape),
                home: &self.home,
                started: &mut started,
            };
            batch.step_clone(
                &store,
                &NativeGit,
                &HandoffHooks,
                &self.config_path,
                &self.home,
                &mut banner,
            )
        } else if batch.action.removes() {
            batch.step_remove(
                &store,
                &NativeGit,
                &Filesystem,
                &self.config_path,
                &self.home,
            )
        } else {
            batch.step(&store, &NativeGit, &self.config_path)
        };
        let Some(index) = stepped else {
            return started;
        };
        let (action, target, result) = (batch.action, &batch.targets[index], &batch.results[index]);
        if matches!(result.outcome, Outcome::Blocked | Outcome::Failed)
            && batch.shared_with(index).is_none()
        {
            self.problems.record(
                format!("batch · {}", target.name),
                format!(
                    "{} {}: {}",
                    action.label(),
                    outcome_label(result.outcome).to_lowercase(),
                    escape(&result.reason)
                ),
            );
        }
        if batch.finished() {
            let counts = [
                Outcome::Succeeded,
                Outcome::Noop,
                Outcome::Failed,
                Outcome::Blocked,
                Outcome::Cancelled,
            ]
            .into_iter()
            .filter_map(|outcome| {
                let count = batch
                    .results
                    .iter()
                    .filter(|result| result.outcome == outcome)
                    .count();
                (count > 0).then(|| format!("{count} {}", outcome_label(outcome).to_lowercase()))
            })
            .collect::<Vec<_>>();
            self.message = Some(format!("{}: {}", action.label(), counts.join(", ")));
            refresh(self);
        }
        started
    }

    fn running(&self) -> bool {
        self.batch.as_ref().is_some_and(|batch| !batch.finished())
    }

    /// Stops a running batch before its next target and refreshes what it changed.
    fn cancel_batch(&mut self) {
        let Some(batch) = self.batch.as_mut().filter(|batch| !batch.finished()) else {
            return;
        };
        batch.cancel();
        self.message = Some(format!(
            "Batch cancelled · {} unattempted · completed changes remain",
            batch.targets.len() - batch.processed()
        ));
        refresh(self);
    }

    fn batch_needs_terminal(&self) -> bool {
        self.batch.as_ref().is_some_and(Batch::needs_terminal)
    }

    /// The targets from `first` on that failed, one line each for the normal screen.
    fn failures_since(&self, first: usize) -> Vec<String> {
        self.batch
            .iter()
            .flat_map(|batch| {
                batch
                    .targets
                    .iter()
                    .zip(&batch.results)
                    .skip(first)
                    .filter(|(_, result)| result.outcome == Outcome::Failed)
                    .map(|(target, result)| {
                        // The reason names the failing part unless it is the config write.
                        let failed = if result.config == "write failed" {
                            "registration failed"
                        } else {
                            "failed"
                        };
                        format!("{failed} · {} · {}", target.name, escape(&result.reason))
                    })
            })
            .collect()
    }

    fn batch_cancelled(&self) -> bool {
        self.batch.as_ref().is_some_and(|batch| {
            batch
                .results
                .iter()
                .any(|result| result.outcome == Outcome::Cancelled)
        })
    }
}

/// Lager's own lines above native Git output, printed just before Git starts.
struct HandoffBanner<'a> {
    position: usize,
    total: usize,
    identity: Option<String>,
    home: &'a Path,
    started: &'a mut bool,
}

/// Runs a hook through the shell once input typed before it started is gone.
struct HandoffHooks;

impl HookRunner for HandoffHooks {
    type Error = ShellError;

    fn is_cancelled(&self, error: &Self::Error) -> bool {
        Shell.is_cancelled(error)
    }

    fn run_hook(&self, command: &str, directory: &Path) -> Result<(), Self::Error> {
        discard_input();
        Shell.run_hook(command, directory)
    }
}

/// One of Lager's own lines on the normal screen while the inventory is suspended.
fn handoff_line(text: &str) {
    if no_color() {
        eprintln!("lager: {text}");
    } else {
        eprintln!("\x1b[2mlager: {text}\x1b[0m");
    }
}

/// Drops terminal input nobody has read, so keys typed at one reader never reach the next.
fn discard_input() {
    // crossterm's tty source reports nothing for a zero timeout, even with parsed keys queued.
    while event::poll(Duration::from_millis(1)).unwrap_or(false) && event::read().is_ok() {}
    // SAFETY: tcflush only takes a descriptor; inventory requires stdin to be the terminal.
    #[cfg(unix)]
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

/// Waits on the normal screen and returns once a line is entered, or with the exit code when
/// a signal arrives first. What is left of the line is dropped by `resume`.
fn wait_for_enter(interrupted: &dyn Fn() -> Option<i32>) -> Option<i32> {
    loop {
        if let Some(code) = interrupted() {
            return Some(code);
        }
        match event::poll(Duration::from_millis(50)) {
            Ok(false) => {}
            Ok(true) if matches!(event::read(), Ok(Event::Resize(..))) => {}
            Ok(true) | Err(_) => return None,
        }
    }
}

impl EnsureReporter for HandoffBanner<'_> {
    fn report(&mut self, event: EnsureEvent<'_>) {
        if let EnsureEvent::CloneStarted {
            reference,
            destination,
        } = event
        {
            // Keys typed before Git starts must not answer its prompts.
            discard_input();
            *self.started = true;
            eprintln!();
            handoff_line(&format!(
                "clone {} of {} · {} → {}",
                self.position,
                self.total,
                self.identity.clone().unwrap_or_else(|| escape(reference)),
                home_relative(destination, self.home)
            ));
            handoff_line(
                "Git owns this terminal until it returns. Ctrl-C interrupts the batch and exits 130.",
            );
            eprintln!();
        }
    }
}

fn warning_notice(warnings: Vec<String>) -> Option<String> {
    (!warnings.is_empty()).then(|| format!("warning: {}", warnings.join(" · ")))
}

fn stale_scan(previous: &LocalScan) -> LocalScan {
    LocalScan {
        checkouts: previous
            .checkouts
            .iter()
            .cloned()
            .map(|mut checkout| {
                checkout.branch = stale_observation(checkout.branch);
                checkout.changes = stale_observation(checkout.changes);
                checkout
            })
            .collect(),
        ..LocalScan::default()
    }
}

fn stale_observation(field: model::ObservationField) -> model::ObservationField {
    match field {
        model::ObservationField::Known(value)
        | model::ObservationField::Dirty(value)
        | model::ObservationField::Error(value)
        | model::ObservationField::Stale(value) => model::ObservationField::Stale(value),
        model::ObservationField::Clean => model::ObservationField::Stale("known clean".to_owned()),
        model::ObservationField::Pending | model::ObservationField::NotApplicable => field,
    }
}

enum StartupError {
    Enter(String),
    Draw(String),
}

fn start_with_loading<T, O, E, D, S>(
    enter: E,
    draw_loading: D,
    start_scan: S,
) -> Result<(T, O), StartupError>
where
    E: FnOnce() -> Result<T, String>,
    D: FnOnce(&mut T) -> Result<(), String>,
    S: FnOnce() -> O,
{
    let mut terminal = enter().map_err(StartupError::Enter)?;
    draw_loading(&mut terminal).map_err(StartupError::Draw)?;
    let output = start_scan();
    Ok((terminal, output))
}

fn enter_after_restore<R, T, A, C>(acquire: A, construct: C) -> Result<(R, T), String>
where
    A: FnOnce() -> Result<R, String>,
    C: FnOnce() -> Result<T, String>,
{
    let restore = acquire()?;
    let terminal = construct()?;
    Ok((restore, terminal))
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restore: TerminalRestore,
}

struct TerminalRestore {
    raw: bool,
    alternate: bool,
}

impl TerminalRestore {
    fn begin() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| error.to_string())?;
        let mut restore = Self {
            raw: true,
            alternate: false,
        };
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            return Err({
                drop(restore);
                error.to_string()
            });
        }
        restore.alternate = true;
        Ok(restore)
    }

    fn restore(&mut self) {
        if self.alternate {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            self.alternate = false;
        }
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        self.restore();
    }
}

impl TerminalSession {
    fn enter() -> Result<Self, String> {
        let (restore, terminal) = enter_after_restore(TerminalRestore::begin, || {
            let backend = CrosstermBackend::new(io::stdout());
            Terminal::new(backend).map_err(|error| error.to_string())
        })?;
        Ok(Self { terminal, restore })
    }

    /// Hands the terminal to a native child: cooked mode, main screen, cursor shown.
    fn suspend(&mut self) {
        let _ = self.terminal.show_cursor();
        self.restore.restore();
    }

    /// Takes the terminal back. After a child ran or a line was awaited, what was typed there
    /// and left unread is dropped; otherwise every key was meant for the inventory and stays.
    fn resume(&mut self, discard: bool) -> Result<(), String> {
        self.restore = TerminalRestore::begin()?;
        if discard {
            discard_input();
        }
        // The frame ratatui remembers was drawn on a screen that is gone.
        self.terminal.clear().map_err(|error| error.to_string())
    }

    fn draw_loading(&mut self) -> Result<(), String> {
        self.terminal
            .draw(|frame| {
                let [header, body] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                        .areas(frame.area());
                frame.render_widget(bar("LAGER / inventory", "", header.width), header);
                frame.render_widget(
                    Paragraph::new("Scanning repository roots…\nGit status appears as observations complete. Remote discovery is unavailable.")
                        .wrap(Wrap { trim: false }),
                    body.inner(Margin { horizontal: 2, vertical: 1 }),
                );
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn run(
        &mut self,
        session: &mut InventorySession,
        interrupted: &dyn Fn() -> Option<i32>,
    ) -> i32 {
        let mut selection = Selection::default();
        // An event was read in the last pass. Keys queued behind it are handled before the
        // batch takes its next step, so cancelling reaches the batch between two targets.
        let mut typed = false;
        // The key typed right after an Esc that the terminal delivered as one Alt+key.
        let mut held: Option<KeyEvent> = None;
        // The keys being read were typed while a batch target held this loop.
        let mut queued = false;
        loop {
            if let Some(code) = interrupted() {
                session.worker.cancel_and_join();
                return code;
            }
            session.receive_events();
            // The batch waits while the user decides whether to cancel it.
            let deciding = selection.quit;
            if !deciding && session.batch_needs_terminal() {
                self.suspend();
                let first = session.batch.as_ref().map_or(0, Batch::processed);
                let mut ran = false;
                while session.batch_needs_terminal() {
                    ran |= session.step_batch();
                    // The terminal stays with the shell: a cancelled child ends the session.
                    if session.batch_cancelled() || interrupted().is_some() {
                        session.worker.cancel_and_join();
                        eprintln!("\nlager: batch interrupted; completed changes remain");
                        return interrupted().unwrap_or(130);
                    }
                }
                // A failure's native output stays on screen until the user has read it.
                let failures = session.failures_since(first);
                if !failures.is_empty() {
                    eprintln!();
                    failures.iter().for_each(|line| handoff_line(line));
                    discard_input();
                    handoff_line("Enter returns to the inventory. Ctrl-C exits 130.");
                    if let Some(code) = wait_for_enter(interrupted) {
                        session.worker.cancel_and_join();
                        return code;
                    }
                }
                if let Err(error) = self.resume(ran || !failures.is_empty()) {
                    eprintln!(
                        "lager: could not restore inventory terminal: {}",
                        escape(error)
                    );
                    return 1;
                }
            } else if !deciding && !typed {
                let processed = session.batch.as_ref().map_or(0, Batch::processed);
                session.step_batch();
                queued |= session.batch.as_ref().map_or(0, Batch::processed) != processed;
            }
            let report = match session.report() {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("lager: {}", escape(error));
                    return 1;
                }
            };
            let dropped = session.marks.reconcile(&report.rows, session.complete);
            if dropped > 0 {
                let removed = if dropped == 1 {
                    "Removed 1 mark: its entry vanished".to_owned()
                } else {
                    format!("Removed {dropped} marks: their entries vanished")
                };
                session.message = Some(match session.message.take() {
                    Some(message) => format!("{message} · {removed}"),
                    None => removed,
                });
            }
            let visible_rows = matching_rows(&report, &selection.query);
            selection.reconcile(&visible_rows, session.complete);
            // A confirmation is answered only by keys typed once it is on screen.
            if matches!(selection.view(), View::Preview | View::AddPreview)
                && std::mem::take(&mut selection.preview_fresh)
            {
                discard_input();
                held = None;
            }
            if let Err(error) = self.draw_report(&report, &visible_rows, session, &mut selection) {
                eprintln!("lager: terminal render failed: {}", escape(error));
                return 1;
            }
            // crossterm's tty source reports nothing for a zero timeout, even with keys queued.
            let ready = if held.is_some() {
                Ok(true)
            } else {
                event::poll(Duration::from_millis(if session.running() && !deciding {
                    1
                } else {
                    50
                }))
            };
            if let Some(code) = interrupted() {
                session.worker.cancel_and_join();
                return code;
            }
            typed = matches!(ready, Ok(true));
            queued &= typed;
            match ready {
                Ok(true) => match held
                    .take()
                    .map_or_else(event::read, |key| Ok(Event::Key(key)))
                {
                    Ok(Event::Key(mut key)) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL
                        {
                            session.worker.cancel_and_join();
                            return 130;
                        }
                        session.message = None;
                        let view = selection.view();
                        let mode = view.mode();
                        // Esc and the key typed after it while a target held the loop arrive
                        // together, and the terminal reads that as one Alt+key. They are the
                        // two keys that were typed. Arrows and function keys encode Alt
                        // themselves and stay chords.
                        if queued
                            && key.modifiers.contains(KeyModifiers::ALT)
                            && matches!(
                                key.code,
                                KeyCode::Char(_)
                                    | KeyCode::Enter
                                    | KeyCode::Tab
                                    | KeyCode::Backspace
                            )
                        {
                            held = Some(KeyEvent::new(
                                key.code,
                                key.modifiers.difference(KeyModifiers::ALT),
                            ));
                            key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                        }
                        match session.bindings.action(mode, key) {
                            Some(Action::Quit) if view == View::QuitBatch => {}
                            Some(Action::Quit) if session.running() => {
                                selection.ask_quit();
                            }
                            Some(Action::Quit) => return quit(session),
                            Some(action @ (Action::Up | Action::Down))
                                if view == View::QuitBatch =>
                            {
                                selection.quit_cancel = action == Action::Down;
                            }
                            Some(Action::Accept) if view == View::QuitBatch => {
                                if selection.quit_cancel {
                                    session.cancel_batch();
                                    return quit(session);
                                }
                                selection.quit = false;
                            }
                            Some(Action::Cancel) if view == View::QuitBatch => {
                                selection.quit = false;
                            }
                            Some(Action::Add) if view == View::Normal => {
                                selection.add = Some(AddForm::default());
                            }
                            Some(Action::Remove) if view == View::Normal => {
                                let row = selection.selected_row(&visible_rows);
                                confirm_removal(
                                    session,
                                    &mut selection,
                                    &report,
                                    &visible_rows,
                                    row,
                                    BatchAction::Remove,
                                );
                            }
                            Some(Action::Accept) if view == View::AddInput => {
                                let form = selection.add.as_mut().expect("Add is open");
                                match batch::add_preview(
                                    &form.text,
                                    &session.config,
                                    &session.root,
                                    &report.rows,
                                    session.complete,
                                    &LocalInventoryFilesystem,
                                ) {
                                    Ok(preview) => {
                                        form.choice =
                                            match preview.eligibility(AddChoice::CloneRegister) {
                                                Eligibility::Unavailable(_) => 0,
                                                _ => 2,
                                            };
                                        form.preview = Some(preview);
                                        form.rejected = None;
                                        selection.preview_fresh = true;
                                    }
                                    Err(reason) => {
                                        if validate_reference_safety(&form.text).is_err() {
                                            form.text.clear();
                                        }
                                        form.rejected = Some(reason);
                                    }
                                }
                            }
                            Some(Action::Cancel) if view == View::AddInput => {
                                selection.add = None;
                                session.message =
                                    Some("Add cancelled · nothing changed".to_owned());
                            }
                            Some(Action::Cancel) if view == View::AddPreview => {
                                if let Some(form) = selection.add.as_mut() {
                                    form.preview = None;
                                }
                            }
                            Some(action @ (Action::Up | Action::Down))
                                if view == View::AddPreview =>
                            {
                                if let Some(form) = selection.add.as_mut() {
                                    form.choice = if action == Action::Up {
                                        form.choice.saturating_sub(1)
                                    } else {
                                        (form.choice + 1).min(ADD_CHOICES.len())
                                    };
                                }
                            }
                            Some(Action::Accept) if view == View::AddPreview => {
                                choose_add(session, &mut selection);
                            }
                            None if view == View::AddInput => {
                                if let Some(form) = selection.add.as_mut() {
                                    edit_text(&mut form.text, key);
                                    form.rejected = None;
                                }
                            }
                            Some(Action::Accept) if view == View::Menu => {
                                if let Some(code) = activate_menu_entry(
                                    session,
                                    &mut selection,
                                    &report,
                                    &visible_rows,
                                ) {
                                    return code;
                                }
                            }
                            Some(Action::Refresh) => {
                                selection.menu.clear();
                                refresh(session);
                            }
                            Some(Action::Menu) if view == View::Normal => {
                                selection.open_menu(Menu::Root, &visible_rows);
                            }
                            Some(Action::Search) => {
                                selection.menu.clear();
                                selection.search = true;
                            }
                            Some(Action::ClearSearch) => {
                                selection.menu.clear();
                                selection.query.clear();
                            }
                            Some(Action::Help) => {
                                if !selection.help {
                                    selection.help_mode = mode;
                                }
                                selection.help = !selection.help;
                                selection.help_scroll = 0;
                            }
                            Some(Action::Cancel | Action::Accept) if view == View::Help => {
                                selection.help = false;
                            }
                            Some(Action::Cancel) if view == View::Menu => {
                                selection.menu.pop();
                                selection.menu_scroll = 0;
                            }
                            Some(Action::Cancel | Action::Accept) if view == View::Problems => {
                                selection.problems = false;
                            }
                            Some(Action::Acknowledge) if view == View::Preview => {
                                if let Some(batch) = selection.preview.as_mut()
                                    && acknowledgment(batch).is_some()
                                {
                                    batch.acknowledged = !batch.acknowledged;
                                }
                            }
                            Some(Action::Accept)
                                if view == View::Preview
                                    && selection.preview.as_ref().is_some_and(|batch| {
                                        selection.preview_focus
                                            < usize::from(acknowledgment(batch).is_some())
                                    }) =>
                            {
                                if let Some(batch) = selection.preview.as_mut() {
                                    batch.acknowledged = !batch.acknowledged;
                                }
                            }
                            Some(Action::Accept) if view == View::Preview => {
                                let batch = selection.preview.take();
                                let run = batch.as_ref().is_some_and(|batch| {
                                    selection.preview_focus
                                        > usize::from(acknowledgment(batch).is_some())
                                });
                                if !run {
                                    session.message = batch.map(|batch| {
                                        format!(
                                            "{} cancelled · nothing changed",
                                            batch.action.label()
                                        )
                                    });
                                } else if session
                                    .batch
                                    .as_ref()
                                    .is_some_and(|batch| !batch.finished())
                                {
                                    session.message = Some("A batch is already running".to_owned());
                                } else if let Some(batch) = batch {
                                    start_batch(
                                        session,
                                        &mut selection,
                                        Batch {
                                            previewed: true,
                                            ..batch
                                        },
                                    );
                                }
                            }
                            Some(Action::Cancel) if view == View::Preview => {
                                session.message = selection.preview.take().map(|batch| {
                                    format!("{} cancelled · nothing changed", batch.action.label())
                                });
                            }
                            Some(action @ (Action::Up | Action::Down)) if view == View::Preview => {
                                let last = selection.preview.as_ref().map_or(1, |batch| {
                                    1 + usize::from(acknowledgment(batch).is_some())
                                });
                                selection.preview_focus = if action == Action::Up {
                                    selection.preview_focus.saturating_sub(1)
                                } else {
                                    (selection.preview_focus + 1).min(last)
                                };
                            }
                            Some(Action::PageUp) if view == View::Preview => {
                                selection.preview_scroll =
                                    selection.preview_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if view == View::Preview => {
                                selection.preview_scroll = selection
                                    .preview_scroll
                                    .saturating_add(10)
                                    .min(self.preview_scroll_limit(&selection, session));
                            }
                            Some(Action::Accept)
                                if view == View::Batch
                                    && selection.batch_cancel
                                    && session.running() =>
                            {
                                session.cancel_batch();
                            }
                            // In the popup of a running batch a quit key asks, unless it is
                            // the key the popup shows for returning.
                            Some(Action::Cancel)
                                if view == View::Batch
                                    && session.running()
                                    && session.bindings.action(Mode::Normal, key)
                                        == Some(Action::Quit)
                                    && !session.bindings.is_first(
                                        Mode::Inspection,
                                        Action::Cancel,
                                        key,
                                    ) =>
                            {
                                selection.ask_quit();
                            }
                            Some(Action::Cancel | Action::Accept) if view == View::Batch => {
                                selection.batch = false;
                            }
                            Some(action @ (Action::Up | Action::Down))
                                if view == View::Batch && session.running() =>
                            {
                                selection.batch_cancel = action == Action::Down;
                            }
                            Some(
                                action @ (Action::Up
                                | Action::Down
                                | Action::PageUp
                                | Action::PageDown),
                            ) if view == View::Batch => {
                                let limit = self.batch_scroll_limit(session);
                                let scroll = selection.batch_scroll;
                                selection.batch_scroll = match action {
                                    Action::Up => scroll.saturating_sub(1),
                                    Action::Down => scroll.saturating_add(1).min(limit),
                                    Action::PageUp => scroll.saturating_sub(10),
                                    _ => scroll.saturating_add(10).min(limit),
                                };
                            }
                            Some(Action::Cancel | Action::Accept) if view == View::Inspect => {
                                selection.close_inspection();
                            }
                            Some(Action::Cancel | Action::Accept) if view == View::Search => {
                                selection.search = false;
                            }
                            Some(Action::Inspect) if view != View::Search => {
                                selection.menu.clear();
                                selection.open_inspection(&visible_rows);
                            }
                            Some(Action::Copy) if view == View::Inspect => {
                                selection.menu = vec![(Menu::Copy, 0)];
                                selection.menu_target = selection.inspection_key.clone();
                                selection.menu_scroll = 0;
                            }
                            Some(Action::Mark) if view == View::Normal => {
                                if let Some(row) = selection.selected_row(&visible_rows) {
                                    toggle_mark(session, row, &report.rows);
                                }
                            }
                            Some(Action::Cancel | Action::Accept) => {}
                            Some(Action::Up) if view == View::Help => {
                                selection.help_scroll = selection.help_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if view == View::Help => {
                                selection.help_scroll = selection
                                    .help_scroll
                                    .saturating_add(1)
                                    .min(self.help_scroll_limit(session, selection.help_mode));
                            }
                            Some(Action::PageUp) if view == View::Help => {
                                selection.help_scroll = selection.help_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if view == View::Help => {
                                selection.help_scroll = selection
                                    .help_scroll
                                    .saturating_add(10)
                                    .min(self.help_scroll_limit(session, selection.help_mode));
                            }
                            Some(Action::Up) if view == View::Problems => {
                                selection.problems_scroll =
                                    selection.problems_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if view == View::Problems => {
                                selection.problems_scroll = selection
                                    .problems_scroll
                                    .saturating_add(1)
                                    .min(self.problems_scroll_limit(session));
                            }
                            Some(Action::PageUp) if view == View::Problems => {
                                selection.problems_scroll =
                                    selection.problems_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if view == View::Problems => {
                                selection.problems_scroll = selection
                                    .problems_scroll
                                    .saturating_add(10)
                                    .min(self.problems_scroll_limit(session));
                            }
                            Some(Action::Up) if view == View::Inspect => {
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if view == View::Inspect => {
                                let limit = selection
                                    .inspected_row(&visible_rows)
                                    .map(|row| {
                                        self.inspection_scroll_limit(
                                            row,
                                            &report.rows,
                                            &session.home,
                                            &session.config,
                                        )
                                    })
                                    .unwrap_or(0);
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_add(1).min(limit);
                            }
                            Some(Action::PageUp) if view == View::Inspect => {
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if view == View::Inspect => {
                                let limit = selection
                                    .inspected_row(&visible_rows)
                                    .map(|row| {
                                        self.inspection_scroll_limit(
                                            row,
                                            &report.rows,
                                            &session.home,
                                            &session.config,
                                        )
                                    })
                                    .unwrap_or(0);
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_add(10).min(limit);
                            }
                            Some(Action::Up) if view == View::Menu => selection.move_menu_by(-1),
                            Some(Action::Down) if view == View::Menu => selection.move_menu_by(1),
                            Some(Action::PageUp) if view == View::Menu => {
                                selection.move_menu_by(-10)
                            }
                            Some(Action::PageDown) if view == View::Menu => {
                                selection.move_menu_by(10)
                            }
                            Some(Action::Up) => selection.move_by(&visible_rows, -1),
                            Some(Action::Down) => selection.move_by(&visible_rows, 1),
                            None if view == View::Search => edit_text(&mut selection.query, key),
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("lager: terminal input failed: {}", escape(error));
                        return 1;
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    eprintln!("lager: terminal input failed: {}", escape(error));
                    return 1;
                }
            }
        }
    }

    fn area(&self) -> Rect {
        self.terminal
            .size()
            .map(|size| Rect::new(0, 0, size.width, size.height))
            .unwrap_or_default()
    }

    fn help_scroll_limit(&self, session: &InventorySession, mode: Mode) -> u16 {
        popup_scroll_limit(
            self.area(),
            WIDE_POPUP,
            &help_widget(&session.bindings, mode),
        )
    }

    fn inspection_scroll_limit(
        &self,
        row: &model::InventoryRow,
        rows: &[model::InventoryRow],
        home: &Path,
        config: &crate::domain::config::Config,
    ) -> u16 {
        let area = self.area();
        popup_scroll_limit(
            area,
            WIDE_POPUP,
            &inspection_widget(
                row,
                short_names(rows).remove(&row.key).unwrap_or_default(),
                rows,
                home,
                config,
                popup_body_width(area, WIDE_POPUP),
            ),
        )
    }

    fn problems_scroll_limit(&self, session: &InventorySession) -> u16 {
        popup_scroll_limit(self.area(), WIDE_POPUP, &problems_widget(session))
    }

    fn preview_scroll_limit(&self, selection: &Selection, session: &InventorySession) -> u16 {
        selection.preview.as_ref().map_or(0, |batch| {
            let lines = preview_lines(batch, &session.home);
            let choices = preview_choices(batch, selection.preview_focus, &session.bindings);
            let (_, list, _) = preview_regions(self.area(), &lines, choices.len() as u16);
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .line_count(list.width)
                .saturating_sub(usize::from(list.height.max(1)))
                .min(usize::from(u16::MAX)) as u16
        })
    }

    fn batch_scroll_limit(&self, session: &InventorySession) -> u16 {
        session.batch.as_ref().map_or(0, |batch| {
            let area = self.area();
            popup_scroll_limit(
                area,
                WIDE_POPUP,
                &batch_widget(
                    batch,
                    &session.home,
                    popup_body_width(area, WIDE_POPUP),
                    false,
                ),
            )
        })
    }

    fn draw_report(
        &mut self,
        report: &InventoryReport,
        visible_rows: &[&model::InventoryRow],
        session: &InventorySession,
        selection: &mut Selection,
    ) -> Result<(), String> {
        let complete = session.complete;
        selection.help_scroll = selection
            .help_scroll
            .min(self.help_scroll_limit(session, selection.help_mode));
        selection.problems_scroll = selection
            .problems_scroll
            .min(self.problems_scroll_limit(session));
        selection.batch_scroll = selection.batch_scroll.min(self.batch_scroll_limit(session));
        selection.preview_scroll = selection
            .preview_scroll
            .min(self.preview_scroll_limit(selection, session));
        if let Some(row) = selection.inspected_row(visible_rows) {
            selection.inspection_scroll =
                selection
                    .inspection_scroll
                    .min(self.inspection_scroll_limit(
                        row,
                        &report.rows,
                        &session.home,
                        &session.config,
                    ));
        }
        let names = short_names(&report.rows);
        let menu = selection.menu.last().map(|(menu, _)| {
            let row = selection.menu_target_row(&report.rows);
            let name = row.map(|row| names[&row.key].clone()).unwrap_or_default();
            let lines = menu_lines(*menu, row, &name, session, visible_rows, &report.rows);
            (menu_title(*menu, &name, session.marks.len()), lines)
        });
        if let Some((_, lines)) = &menu {
            let entries = lines.iter().filter(|line| line.entry.is_some()).count();
            let selected = selection.clamp_menu_index(entries);
            let line = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.entry.is_some())
                .nth(selected)
                .map_or(0, |(index, _)| index);
            let body = menu_widget(lines, line, popup_body_width(self.area(), POPUP));
            let [_, _, content, _] = popup_regions(self.area(), POPUP, &body);
            let viewport = content.height.max(1);
            selection.clamp_menu_scroll(
                (lines.len() as u16).saturating_sub(viewport),
                viewport,
                line as u16,
            );
        }
        let bindings = &session.bindings;
        let view = selection.view();
        let compact = self.area().width < WIDE_TABLE;
        // A running batch is the current state unless an action just reported its outcome.
        let progress = session
            .batch
            .as_ref()
            .filter(|batch| !batch.finished() && view != View::Preview)
            .map(|batch| {
                format!(
                    "Batch: {} / {} processed{}",
                    batch.processed(),
                    batch.targets.len(),
                    bindings
                        .keys(Mode::Normal, Action::Menu)
                        .first()
                        .map(|key| format!(" · {key} → cancel batch"))
                        .unwrap_or_default()
                )
            });
        let status = status_lines(
            report,
            complete,
            session.notice.as_deref(),
            session.message.as_deref().or(progress.as_deref()),
            view,
            compact,
            bindings,
        );
        let acknowledges = selection
            .preview
            .as_ref()
            .is_some_and(|batch| acknowledgment(batch).is_some());
        let home = session.home.as_path();
        let shown = &*selection;
        let mut table_state = shown.table.clone();
        let problems = match session.problems.entries().len() {
            0 => String::new(),
            1 => " · 1 problem".to_owned(),
            count => format!(" · {count} problems"),
        };
        let marked = session.marks.len();
        let counter = (marked > 0).then(|| {
            format!(
                "{marked} marked · {} hidden",
                session.marks.hidden(visible_rows)
            )
        });
        let result = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                let footer =
                    footer_widget(bindings, view, area.width.saturating_sub(4), acknowledges);
                let footer_height = footer.line_count(area.width.saturating_sub(4)) as u16;
                let [header, source, search, _, table, status_area, footer_area] =
                    Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(status.len() as u16),
                        Constraint::Length(footer_height),
                    ])
                    .areas(area);
                frame.render_widget(
                    bar(
                        "LAGER / inventory",
                        &format!(
                            "{} · {} {}{problems}",
                            view.label(),
                            visible_rows.len(),
                            if visible_rows.len() == 1 { "row" } else { "rows" }
                        ),
                        header.width,
                    ),
                    header,
                );
                frame.render_widget(
                    Paragraph::new("Local + declarations · remote off").style(dim()),
                    padded(source),
                );
                let query = escape(&shown.query);
                let editing = view == View::Search;
                let mut query_line = vec![Span::raw("/ ")];
                if editing {
                    query_line.extend([Span::raw(query), Span::raw("█")]);
                } else if query.is_empty() {
                    query_line.push(Span::styled("Search repositories, paths, origins…", dim()));
                } else {
                    query_line.push(Span::raw(query));
                }
                let search_style = match (editing, no_color()) {
                    (false, _) => Style::default(),
                    (true, true) => Style::default().add_modifier(Modifier::BOLD),
                    (true, false) => Style::default().add_modifier(Modifier::REVERSED),
                };
                frame.render_widget(Paragraph::new("").style(search_style), search);
                frame.render_widget(
                    Paragraph::new(Line::from(query_line)).style(search_style),
                    padded(search),
                );
                if let Some(counter) = &counter {
                    frame.render_widget(
                        Paragraph::new(Span::styled(counter.clone(), warn()))
                            .alignment(Alignment::Right)
                            .style(search_style),
                        padded(search),
                    );
                }
                if visible_rows.is_empty() {
                    frame.render_widget(
                        Paragraph::new(if !report.rows.is_empty() && !shown.query.is_empty() {
                            "No matching repositories."
                        } else if !complete {
                            "Discovering local repositories…"
                        } else if report.incomplete {
                            "No repositories found so far; local scan is incomplete."
                        } else {
                            "No repositories found."
                        }),
                        padded(table),
                    );
                } else {
                    frame.render_stateful_widget(
                        repository_table(
                            visible_rows,
                            &names,
                            &session.marks,
                            home,
                            table.width,
                            table_state.selected(),
                        ),
                        table,
                        &mut table_state,
                    );
                }
                frame.render_widget(Paragraph::new(status.clone()), padded(status_area));
                frame.render_widget(Paragraph::new("").style(footer_style()), footer_area);
                frame.render_widget(footer.style(footer_style()), padded(footer_area));
                let close = first_key_hint(bindings, Mode::Inspection, Action::Cancel, "close");
                match view {
                    View::Help => draw_popup(
                        frame,
                        WIDE_POPUP,
                        "Keyboard / effective bindings",
                        help_widget(bindings, shown.help_mode).scroll((shown.help_scroll, 0)),
                        bindings
                            .keys(Mode::Inspection, Action::Cancel)
                            .first()
                            .map(|key| format!("{key} close"))
                            .unwrap_or_default(),
                    ),
                    View::Problems => draw_popup(
                        frame,
                        WIDE_POPUP,
                        &format!("Session problems / {}", session.problems.entries().len()),
                        problems_widget(session).scroll((shown.problems_scroll, 0)),
                        format!(
                            "{} scroll · {close}",
                            rows_hint(bindings, Mode::Inspection, true)
                        ),
                    ),
                    View::Inspect => {
                        let footer = [
                            format!("{} scroll", rows_hint(bindings, Mode::Inspection, true)),
                            first_key_hint(bindings, Mode::Inspection, Action::Copy, "copy a value…"),
                            close,
                        ]
                        .into_iter()
                        .filter(|hint| !hint.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                        match shown.inspected_row(visible_rows) {
                            Some(row) => draw_popup(
                                frame,
                                WIDE_POPUP,
                                &format!("Inspect / {}", names[&row.key]),
                                inspection_widget(
                                    row,
                                    names[&row.key].clone(),
                                    &report.rows,
                                    home,
                                    &session.config,
                                    popup_body_width(area, WIDE_POPUP),
                                )
                                .scroll((shown.inspection_scroll, 0)),
                                footer,
                            ),
                            None => draw_popup(
                                frame,
                                WIDE_POPUP,
                                "Inspect / unavailable",
                                Paragraph::new(
                                    "Inspected target is unavailable or stale. Close this popup and inspect a current row.",
                                )
                                .wrap(Wrap { trim: false }),
                                footer,
                            ),
                        }
                    }
                    View::Menu => {
                        let (title, lines) = menu.as_ref().expect("an open menu has lines");
                        let line = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, line)| line.entry.is_some())
                            .nth(shown.menu.last().map_or(0, |(_, index)| *index))
                            .map_or(0, |(index, _)| index);
                        draw_popup(
                            frame,
                            POPUP,
                            title,
                            menu_widget(lines, line, popup_body_width(area, POPUP))
                                .scroll((shown.menu_scroll, 0)),
                            format!(
                                "{} select · {} · {}",
                                rows_hint(bindings, Mode::Menu, true),
                                first_key_hint(bindings, Mode::Menu, Action::Accept, "choose"),
                                first_key_hint(bindings, Mode::Menu, Action::Cancel, "back")
                            ),
                        )
                    }
                    View::Preview => {
                        if let Some(batch) = &shown.preview {
                            let lines = preview_lines(batch, home);
                            let choice_lines =
                                preview_choices(batch, shown.preview_focus, bindings);
                            let ([popup, title, _, footer], list, choices) =
                                preview_regions(area, &lines, choice_lines.len() as u16);
                            let pages = paired_keys(
                                &bindings.keys(Mode::Confirmation, Action::PageUp),
                                &bindings.keys(Mode::Confirmation, Action::PageDown),
                            );
                            frame.render_widget(Clear, popup);
                            frame.render_widget(Block::bordered().border_style(focus()), popup);
                            frame.render_widget(
                                bar(
                                    &match batch.action {
                                        BatchAction::Remove => {
                                            "Remove checkouts / keep registration".to_owned()
                                        }
                                        BatchAction::RemoveUnregister => {
                                            "Remove checkouts / unregister".to_owned()
                                        }
                                        action => format!("{} / preview", action.label()),
                                    },
                                    "lager",
                                    title.width,
                                ),
                                title,
                            );
                            frame.render_widget(
                                Paragraph::new(lines)
                                    .wrap(Wrap { trim: false })
                                    .scroll((shown.preview_scroll, 0)),
                                list,
                            );
                            frame.render_widget(Paragraph::new(choice_lines), choices);
                            let hint = |action, label| {
                                first_key_hint(bindings, Mode::Confirmation, action, label)
                            };
                            let focus_hint =
                                format!("{} focus", rows_hint(bindings, Mode::Confirmation, true));
                            let scroll = format!("{pages} scroll");
                            let (choose, cancel) =
                                (hint(Action::Accept, "choose"), hint(Action::Cancel, "cancel"));
                            // A narrow popup cuts the end of the line: the exit routes of a
                            // removal confirmation come before the scroll hint.
                            let hints = if acknowledges {
                                let acknowledge = hint(Action::Acknowledge, "acknowledge");
                                [focus_hint, acknowledge, choose, cancel, scroll]
                            } else {
                                [focus_hint, scroll, choose, cancel, String::new()]
                            };
                            frame.render_widget(
                                Paragraph::new(
                                    hints
                                        .into_iter()
                                        .filter(|hint| !hint.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                )
                                .style(dim()),
                                footer,
                            );
                        }
                    }
                    View::Batch => {
                        if let Some(batch) = &session.batch {
                            draw_popup(
                                frame,
                                WIDE_POPUP,
                                &format!("Batch / {}", batch.action.label()),
                                batch_widget(
                                    batch,
                                    home,
                                    popup_body_width(area, WIDE_POPUP),
                                    shown.batch_cancel,
                                )
                                .scroll((shown.batch_scroll, 0)),
                                if batch.finished() {
                                    format!(
                                        "{} scroll · {}",
                                        rows_hint(bindings, Mode::Inspection, true),
                                        first_key_hint(
                                            bindings,
                                            Mode::Inspection,
                                            Action::Cancel,
                                            "return"
                                        )
                                    )
                                } else {
                                    let back = bindings.keys(Mode::Inspection, Action::Cancel);
                                    let quit = bindings
                                        .keys(Mode::Normal, Action::Quit)
                                        .into_iter()
                                        .find(|key| {
                                            back.first() != Some(key) && back.contains(key)
                                        })
                                        .map(|key| format!(" · {key} quit"))
                                        .unwrap_or_default();
                                    format!(
                                        "{} focus · {} · {}{quit}",
                                        rows_hint(bindings, Mode::Inspection, true),
                                        first_key_hint(
                                            bindings,
                                            Mode::Inspection,
                                            Action::Accept,
                                            "choose"
                                        ),
                                        first_key_hint(
                                            bindings,
                                            Mode::Inspection,
                                            Action::Cancel,
                                            "return"
                                        )
                                    )
                                },
                            )
                        }
                    }
                    View::QuitBatch => {
                        if let Some(batch) = &session.batch {
                            draw_popup(
                                frame,
                                POPUP,
                                "A batch is running",
                                quit_widget(
                                    batch,
                                    shown.quit_cancel,
                                    popup_body_width(area, POPUP),
                                ),
                                format!(
                                    "{} · {}",
                                    first_key_hint(
                                        bindings,
                                        Mode::Confirmation,
                                        Action::Accept,
                                        "choose"
                                    ),
                                    first_key_hint(
                                        bindings,
                                        Mode::Confirmation,
                                        Action::Cancel,
                                        "return"
                                    )
                                ),
                            )
                        }
                    }
                    View::AddInput => {
                        if let Some(form) = &shown.add {
                            draw_popup(
                                frame,
                                POPUP,
                                "Add / URL or reference",
                                add_input_widget(
                                    form,
                                    session.marks.len(),
                                    bindings,
                                    popup_body_width(area, POPUP),
                                ),
                                format!(
                                    "{} · {}",
                                    first_key_hint(bindings, Mode::Input, Action::Accept, "preview"),
                                    first_key_hint(bindings, Mode::Input, Action::Cancel, "back")
                                ),
                            )
                        }
                    }
                    View::AddPreview => {
                        if let Some((form, preview)) = shown
                            .add
                            .as_ref()
                            .and_then(|form| Some((form, form.preview.as_ref()?)))
                        {
                            let (fields, choices) = add_preview_lines(
                                preview,
                                form.choice,
                                session.marks.len(),
                                home,
                                bindings,
                                popup_body_width(area, WIDE_POPUP),
                            );
                            let mut sizing = fields.clone();
                            sizing.extend(choices.iter().cloned());
                            let [popup, title, content, footer] = popup_regions(
                                area,
                                WIDE_POPUP,
                                &Paragraph::new(sizing).wrap(Wrap { trim: false }),
                            );
                            let fields = Paragraph::new(fields).wrap(Wrap { trim: false });
                            let choices = Paragraph::new(choices).wrap(Wrap { trim: false });
                            // The note under the choices gives way first, then the fields.
                            let room = content
                                .height
                                .saturating_sub(fields.line_count(content.width) as u16);
                            let [fields_area, choices_area] = Layout::vertical([
                                Constraint::Min(0),
                                Constraint::Length(
                                    (choices.line_count(content.width) as u16)
                                        .min(room)
                                        .max(ADD_CHOICES_HEIGHT.min(content.height)),
                                ),
                            ])
                            .areas(content);
                            frame.render_widget(Clear, popup);
                            frame.render_widget(Block::bordered().border_style(focus()), popup);
                            frame.render_widget(bar("Add / preview", "lager", title.width), title);
                            frame.render_widget(fields, fields_area);
                            frame.render_widget(choices, choices_area);
                            frame.render_widget(
                                Paragraph::new(format!(
                                    "{} select · {} · {}",
                                    rows_hint(bindings, Mode::Confirmation, true),
                                    first_key_hint(
                                        bindings,
                                        Mode::Confirmation,
                                        Action::Accept,
                                        "choose"
                                    ),
                                    first_key_hint(
                                        bindings,
                                        Mode::Confirmation,
                                        Action::Cancel,
                                        "back"
                                    )
                                ))
                                .style(dim()),
                                footer,
                            );
                        }
                    }
                    View::Normal | View::Search => {}
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string());
        selection.table = table_state;
        result
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Normal,
    Search,
    Inspect,
    Menu,
    Problems,
    Preview,
    Batch,
    QuitBatch,
    Help,
    AddInput,
    AddPreview,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Search => "SEARCH",
            Self::Inspect => "INSPECT",
            Self::Menu => "MENU",
            Self::Problems => "PROBLEMS",
            Self::Preview | Self::QuitBatch => "CONFIRM",
            Self::Batch => "RESULTS",
            Self::Help => "HELP",
            Self::AddInput | Self::AddPreview => "INPUT",
        }
    }

    fn mode(self) -> Mode {
        match self {
            Self::Normal => Mode::Normal,
            Self::Search => Mode::Search,
            Self::Menu => Mode::Menu,
            Self::Inspect | Self::Problems | Self::Batch | Self::Help => Mode::Inspection,
            Self::Preview | Self::AddPreview | Self::QuitBatch => Mode::Confirmation,
            Self::AddInput => Mode::Input,
        }
    }
}

const POPUP: u16 = 66;
const WIDE_POPUP: u16 = 100;
/// Terminal width at which the table shows all seven columns of the wide grid.
const WIDE_TABLE: u16 = 132;
/// Reserved for the mark column; the repository text starts after it on every row.
const MARK_SLOT: usize = 4;
const INSPECT_KEY_COLUMN: usize = 19;

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// Colors are terminal-relative ANSI colors so the terminal theme decides light or dark.
fn tint(color: Color) -> Style {
    if no_color() {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn warn() -> Style {
    tint(Color::Yellow)
}

fn focus() -> Style {
    tint(Color::Green)
}

/// Bars take reverse video only with color; without color reverse video marks the selection alone.
fn bar_style() -> Style {
    if no_color() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    }
}

fn footer_style() -> Style {
    if no_color() {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    }
}

fn padded(area: Rect) -> Rect {
    area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    })
}

fn bar(left: &str, right: &str, width: u16) -> Paragraph<'static> {
    let gap = usize::from(width)
        .saturating_sub(4 + Line::raw(left).width() + Line::raw(right).width())
        .max(1);
    Paragraph::new(format!("  {left}{}{right}  ", " ".repeat(gap))).style(bar_style())
}

/// Pairs the n-th key of `first` with the n-th key of `second`, as in `j/k ↑/↓`.
fn paired_keys(first: &[&str], second: &[&str]) -> String {
    (0..first.len().max(second.len()))
        .map(|index| match (first.get(index), second.get(index)) {
            (Some(&"Down"), Some(&"Up")) => "↑/↓".to_owned(),
            (Some(a), Some(b)) => format!("{a}/{b}"),
            (Some(key), None) | (None, Some(key)) => (*key).to_owned(),
            (None, None) => String::new(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn rows_hint(bindings: &Bindings, mode: Mode, arrows: bool) -> String {
    let down = bindings.keys(mode, Action::Down);
    let up = bindings.keys(mode, Action::Up);
    let mut hint = [down.first(), up.first()]
        .into_iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if arrows && down.contains(&"Down") && up.contains(&"Up") && down.first() != Some(&"Down") {
        hint.push_str(" ↑/↓");
    }
    hint
}

fn first_key_hint(bindings: &Bindings, mode: Mode, action: Action, label: &str) -> String {
    bindings
        .keys(mode, action)
        .first()
        .map(|key| format!("{key} {label}"))
        .unwrap_or_default()
}

/// One line of hints; the least needed drop out when the line is too narrow.
fn footer_widget(
    bindings: &Bindings,
    view: View,
    width: u16,
    acknowledges: bool,
) -> Paragraph<'static> {
    let first = |mode: Mode, action: Action| {
        bindings
            .keys(mode, action)
            .first()
            .map(|key| (*key).to_owned())
            .unwrap_or_default()
    };
    let items: Vec<(String, &str)> = match view {
        View::Search => vec![
            (first(Mode::Search, Action::Accept), "accept"),
            (first(Mode::Search, Action::Cancel), "cancel"),
            ("Backspace".to_owned(), "delete"),
        ],
        View::Menu => vec![
            (rows_hint(bindings, Mode::Menu, false), "select"),
            (first(Mode::Menu, Action::Accept), "choose"),
            (first(Mode::Menu, Action::Cancel), "back"),
        ],
        View::AddInput => vec![
            (first(Mode::Input, Action::Accept), "preview"),
            (first(Mode::Input, Action::Cancel), "back"),
        ],
        View::AddPreview => vec![
            (rows_hint(bindings, Mode::Confirmation, false), "select"),
            (first(Mode::Confirmation, Action::Accept), "choose"),
            (
                first(Mode::Confirmation, Action::Cancel),
                "back to reference",
            ),
        ],
        View::QuitBatch => vec![
            (rows_hint(bindings, Mode::Confirmation, false), "focus"),
            (first(Mode::Confirmation, Action::Accept), "choose"),
            (first(Mode::Confirmation, Action::Cancel), "return"),
        ],
        View::Preview => vec![
            (rows_hint(bindings, Mode::Confirmation, false), "focus"),
            (
                if acknowledges {
                    first(Mode::Confirmation, Action::Acknowledge)
                } else {
                    String::new()
                },
                "acknowledge",
            ),
            (
                paired_keys(
                    &bindings.keys(Mode::Confirmation, Action::PageUp),
                    &bindings.keys(Mode::Confirmation, Action::PageDown),
                ),
                "scroll",
            ),
            (first(Mode::Confirmation, Action::Accept), "choose"),
            (first(Mode::Confirmation, Action::Cancel), "cancel"),
        ],
        View::Help | View::Problems | View::Batch => vec![
            (rows_hint(bindings, Mode::Inspection, false), "scroll"),
            (first(Mode::Inspection, Action::Cancel), "close"),
        ],
        View::Normal | View::Inspect => {
            let mut items = vec![(rows_hint(bindings, Mode::Normal, false), "rows")];
            items.extend(
                [
                    (Action::Search, "search"),
                    (Action::Mark, "mark"),
                    (Action::Add, "add"),
                    (Action::Remove, "remove"),
                    (Action::Inspect, "inspect"),
                    (Action::Refresh, "refresh"),
                    (Action::Menu, "menu"),
                    (Action::Help, "help"),
                    (Action::Quit, "quit"),
                ]
                .map(|(action, label)| (first(Mode::Normal, action), label)),
            );
            items
        }
    };
    let mut items = items
        .into_iter()
        .filter(|(keys, _)| !keys.is_empty())
        .collect::<Vec<_>>();
    let width_of = |items: &[(String, &str)]| {
        items
            .iter()
            .map(|(keys, label)| Line::raw(format!("{keys} {label}")).width() + 2)
            .sum::<usize>()
            .saturating_sub(2)
    };
    // Least needed first; exit, help and menu routes always stay visible.
    for dropped in [
        "scroll", "mark", "remove", "add", "rows", "refresh", "search", "inspect", "delete",
    ] {
        if width_of(&items) <= usize::from(width) {
            break;
        }
        items.retain(|(_, label)| *label != dropped);
    }
    let mut spans = Vec::new();
    for (keys, label) in items {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            keys,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {label}")));
    }
    Paragraph::new(Line::from(spans))
}

fn status_lines(
    report: &InventoryReport,
    complete: bool,
    notice: Option<&str>,
    message: Option<&str>,
    view: View,
    compact: bool,
    bindings: &Bindings,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(notice) = notice {
        lines.push(Line::styled(format!("! {notice}"), warn()));
    }
    if report.incomplete {
        lines.push(Line::styled(
            format!(
                "! partial scan: {} diagnostics; one or more paths could not be read",
                report.diagnostics.len()
            ),
            warn(),
        ));
    }
    let excluded = report
        .diagnostics
        .iter()
        .filter(|diagnostic| !diagnostic.incomplete)
        .map(|diagnostic| {
            format!(
                "{}: {}",
                escape(
                    diagnostic
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                escape(&diagnostic.message)
            )
        })
        .collect::<Vec<_>>();
    if !excluded.is_empty() {
        lines.push(Line::styled(
            format!("ignored repositories: {}", excluded.join(", ")),
            dim(),
        ));
    } else if let Some(diagnostic) = report.diagnostics.first() {
        lines.push(Line::styled(
            format!(
                "diagnostic: {}: {}",
                escape(diagnostic.path.to_string_lossy()),
                escape(&diagnostic.message)
            ),
            dim(),
        ));
    }
    let state = if complete {
        "No active operation"
    } else {
        "Scanning local repositories…"
    };
    let refresh = first_key_hint(
        bindings,
        Mode::Normal,
        Action::Refresh,
        "refreshes local/config state only",
    );
    let inspect = bindings.keys(Mode::Normal, Action::Inspect);
    let operation = match view {
        View::Search => format!(
            "{} / {} retain filter · action letters are text",
            bindings.hint(Mode::Search, Action::Accept),
            bindings.hint(Mode::Search, Action::Cancel)
        ),
        _ if message.is_some() && complete => message.unwrap_or_default().to_owned(),
        _ if message.is_some() => format!("{state} · {}", message.unwrap_or_default()),
        View::Preview => "Nothing changes until you confirm".to_owned(),
        View::AddInput => "Ordinary keys type text · nothing is saved yet".to_owned(),
        View::AddPreview => "Nothing changes until you choose".to_owned(),
        View::Menu | View::Help | View::Problems | View::Batch | View::QuitBatch => {
            state.to_owned()
        }
        View::Normal | View::Inspect if compact && complete && !inspect.is_empty() => format!(
            "Path and origin: {} inspects, menu copies the full value",
            inspect[0]
        ),
        View::Normal | View::Inspect if refresh.is_empty() => state.to_owned(),
        View::Normal | View::Inspect => format!("{state} · {refresh}"),
    };
    lines.push(Line::styled(operation, dim()));
    lines
}

/// Returns the popup, its title row, body and footer row. The popup fits its body up to the
/// screen height, so scroll limits and drawing share this one calculation.
fn popup_regions(area: Rect, cap: u16, body: &Paragraph) -> [Rect; 4] {
    let width = area.width.saturating_sub(2).min(cap);
    let lines = body
        .line_count(width.saturating_sub(6))
        .min(usize::from(u16::MAX)) as u16;
    let height = lines.saturating_add(5).min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let [title, _, content, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    }));
    [popup, title, padded(content), padded(footer)]
}

fn popup_body_width(area: Rect, cap: u16) -> u16 {
    area.width.saturating_sub(2).min(cap).saturating_sub(6)
}

fn popup_scroll_limit(area: Rect, cap: u16, body: &Paragraph) -> u16 {
    let [_, _, content, _] = popup_regions(area, cap, body);
    body.line_count(content.width)
        .saturating_sub(usize::from(content.height.max(1)))
        .min(usize::from(u16::MAX)) as u16
}

fn draw_popup(
    frame: &mut ratatui::Frame<'_>,
    cap: u16,
    title: &str,
    body: Paragraph<'static>,
    footer: String,
) {
    let [popup, title_area, content, footer_area] = popup_regions(frame.area(), cap, &body);
    frame.render_widget(Clear, popup);
    frame.render_widget(Block::bordered().border_style(focus()), popup);
    frame.render_widget(bar(title, "lager", title_area.width), title_area);
    frame.render_widget(body, content);
    frame.render_widget(Paragraph::new(footer).style(dim()), footer_area);
}

fn help_widget(bindings: &Bindings, mode: Mode) -> Paragraph<'static> {
    use Action::*;
    let keys = |action| bindings.keys(mode, action);
    let pair = |first, second| paired_keys(&keys(first), &keys(second));
    let single = |action| keys(action).join("/");
    let help_quit = || {
        [single(Help), single(Quit)]
            .into_iter()
            .filter(|keys| !keys.is_empty())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let entries: Vec<(String, &str)> = match mode {
        Mode::Normal => vec![
            (pair(Down, Up), "previous / next row"),
            (single(Search), "live fuzzy search"),
            (single(Mark), "toggle mark"),
            (single(Add), "add a repository by reference"),
            (single(Remove), "remove the checkout, keep registration"),
            (single(ClearSearch), "clear filter"),
            (single(Inspect), "Inspect repository"),
            (single(Refresh), "refresh local/config state"),
            (single(Menu), "all actions"),
            (help_quit(), "help / quit"),
        ],
        Mode::Menu => vec![
            (pair(Down, Up), "previous / next entry"),
            (pair(PageUp, PageDown), "page up / down"),
            (single(Accept), "choose"),
            (single(Cancel), "back"),
            (single(Search), "live fuzzy search"),
            (single(ClearSearch), "clear filter"),
            (single(Inspect), "Inspect repository"),
            (single(Refresh), "refresh local/config state"),
            (help_quit(), "help / quit"),
        ],
        Mode::Search => vec![
            (pair(Accept, Cancel).replace('/', " / "), "keep the filter"),
            (help_quit(), "help / quit"),
        ],
        Mode::Inspection => vec![
            (pair(Down, Up), "scroll"),
            (pair(PageUp, PageDown), "page up / down"),
            (single(Copy), "copy a value…"),
            (single(Accept), "close"),
            (single(Cancel), "close"),
            (help_quit(), "help / quit"),
        ],
        Mode::Confirmation => vec![
            (pair(Down, Up), "move focus"),
            (pair(PageUp, PageDown), "page up / down"),
            (single(Acknowledge), "acknowledge the listed losses"),
            (single(Accept), "choose"),
            (single(Cancel), "cancel"),
            (help_quit(), "help / quit"),
        ],
        Mode::Input => vec![
            (single(Accept), "preview the reference"),
            (single(Cancel), "back"),
            (help_quit(), "help / quit"),
        ],
    };
    let entries = entries
        .into_iter()
        .filter(|(keys, _)| !keys.is_empty())
        .collect::<Vec<_>>();
    let column = entries
        .iter()
        .map(|(keys, _)| Line::raw(keys.clone()).width())
        .max()
        .unwrap_or(0)
        .clamp(12, 24)
        + 2;
    let title = match mode {
        Mode::Normal => "Normal mode",
        Mode::Menu => "Menu mode",
        Mode::Search => "Search mode",
        Mode::Input => "Input mode",
        Mode::Confirmation => "Confirmation mode",
        Mode::Inspection => "Inspect mode",
    };
    let mut lines = vec![Line::styled(title, dim())];
    lines.extend(entries.into_iter().map(|(keys, description)| {
        let padding = column
            .saturating_sub(Line::raw(keys.clone()).width())
            .max(1);
        Line::from(vec![
            Span::styled(keys, focus()),
            Span::raw(format!("{}{description}", " ".repeat(padding))),
        ])
    }));
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

fn identity(row: &model::InventoryRow) -> Option<&str> {
    match &row.key {
        model::RowKey::Checkout { identity, .. }
        | model::RowKey::Declaration { identity, .. }
        | model::RowKey::Pattern(identity) => Some(identity),
        model::RowKey::Path(_) => None,
    }
}

/// The last `depth` segments of `group`, or all of it when it has no more.
fn trailing(group: &str, separator: char, depth: usize) -> &str {
    group
        .rmatch_indices(separator)
        .nth(depth - 1)
        .map(|(index, _)| &group[index + 1..])
        .filter(|name| !name.is_empty())
        .unwrap_or(group)
}

/// Names every row for the table: the last identity segment (`owner/*` for patterns, the
/// directory for path-identified checkouts), widened to `owner/name` when another repository
/// shares that last segment, then one leading segment at a time while another repository of
/// the same kind (identity, pattern or path) still shares the name. A checkout reads
/// `name (copy)`, `name (copy 2)`, … when its identity already has a cloned declaration or a
/// checkout at an earlier path.
fn short_names(rows: &[model::InventoryRow]) -> HashMap<model::RowKey, String> {
    let groups = rows
        .iter()
        .map(|row| match &row.key {
            model::RowKey::Checkout { identity, .. }
            | model::RowKey::Declaration { identity, .. } => (0, identity.clone(), '/'),
            model::RowKey::Pattern(pattern) => (1, pattern.clone(), '/'),
            model::RowKey::Path(path) => (
                2,
                path.to_string_lossy().into_owned(),
                std::path::MAIN_SEPARATOR,
            ),
        })
        .collect::<Vec<_>>();
    let mut depths = groups
        .iter()
        .map(|(kind, ..)| if *kind == 1 { 2 } else { 1 })
        .collect::<Vec<usize>>();
    loop {
        let name = |index: usize| {
            let (kind, group, separator) = &groups[index];
            let depth = depths[index];
            (
                (depth > 1).then_some(*kind),
                trailing(group, *separator, depth),
            )
        };
        let mut sharing: HashMap<_, HashSet<&str>> = HashMap::new();
        for (index, (_, group, _)) in groups.iter().enumerate() {
            sharing.entry(name(index)).or_default().insert(group);
        }
        let shared = (0..groups.len())
            .filter(|index| name(*index).1 != groups[*index].1 && sharing[&name(*index)].len() > 1)
            .collect::<Vec<_>>();
        if shared.is_empty() {
            break;
        }
        for index in shared {
            depths[index] += 1;
        }
    }
    rows.iter()
        .zip(groups.iter().zip(&depths))
        .map(|(row, ((_, group, separator), depth))| {
            let mut name = trailing(group, *separator, *depth).to_owned();
            if let model::RowKey::Checkout { identity, path } = &row.key {
                let earlier = rows
                    .iter()
                    .filter(|other| match &other.key {
                        model::RowKey::Declaration {
                            identity: other_identity,
                            ..
                        } => {
                            other_identity == identity
                                && other.checkout == model::CheckoutState::Cloned
                        }
                        model::RowKey::Checkout {
                            identity: other_identity,
                            path: other_path,
                        } => other_identity == identity && other_path < path,
                        _ => false,
                    })
                    .count();
                match earlier {
                    0 => {}
                    1 => name.push_str(" (copy)"),
                    count => name.push_str(&format!(" (copy {count})")),
                }
            }
            (row.key.clone(), escape(name))
        })
        .collect()
}

fn home_relative(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => escape(format!("~/{}", rest.to_string_lossy())),
        Err(_) => escape(path.to_string_lossy()),
    }
}

fn origin_text(row: &model::InventoryRow) -> String {
    escape(identity(row).unwrap_or(&row.origin))
}

fn registration_text(state: &model::RegistrationState) -> &'static str {
    match state {
        model::RegistrationState::Pattern => "wildcard",
        other => other.label(),
    }
}

fn checkout_text(state: &model::CheckoutState) -> &'static str {
    match state {
        model::CheckoutState::Pattern => "—",
        other => other.label(),
    }
}

fn observation_text(field: &model::ObservationField) -> String {
    match field {
        model::ObservationField::Clean => "clean".to_owned(),
        model::ObservationField::NotApplicable => "—".to_owned(),
        other => escape(other.label()),
    }
}

/// Splits `value` into pieces at most `room` cells wide, so a column can wrap in place.
fn column_chunks(value: &str, room: usize) -> Vec<String> {
    let room = room.max(1);
    let mut chunks = vec![String::new()];
    let mut used = 0;
    for character in value.chars() {
        let size = Span::raw(character.to_string()).width();
        if used + size > room && used > 0 {
            chunks.push(String::new());
            used = 0;
        }
        chunks.last_mut().expect("one chunk").push(character);
        used += size;
    }
    chunks
}

/// Lays out `name value` with the value in its own column; long values wrap inside it.
fn field_lines(name: &str, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    column_chunks(value, usize::from(width).saturating_sub(INSPECT_KEY_COLUMN))
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let label = if index == 0 { name } else { "" };
            Line::from(vec![
                Span::styled(format!("{label:<INSPECT_KEY_COLUMN$}"), dim()),
                Span::styled(chunk, style),
            ])
        })
        .collect()
}

fn inspection_widget(
    row: &model::InventoryRow,
    name: String,
    rows: &[model::InventoryRow],
    home: &Path,
    config: &crate::domain::config::Config,
    width: u16,
) -> Paragraph<'static> {
    let mut lines = Vec::new();
    let mut field = |name: &str, value: String, style: Style| {
        lines.extend(field_lines(name, &value, style, width));
    };
    field("Repository", name, Style::default());
    if let Some(identity) = identity(row) {
        field("Identity", escape(identity), Style::default());
    }
    if let Some(path) = &row.observed_path {
        field("Found path", home_relative(path, home), Style::default());
    }
    if let Some(path) = &row.configured_destination {
        field(
            "Configured path",
            home_relative(path, home),
            Style::default(),
        );
    }
    if row.observed_path.is_none() && row.configured_destination.is_none() {
        field(
            "Path",
            home_relative(Path::new(&row.path), home),
            Style::default(),
        );
    }
    let registration = match &row.registration {
        model::RegistrationState::Explicit => "explicit".to_owned(),
        model::RegistrationState::Wildcard { pattern } => {
            format!("wildcard · matches {}", escape(pattern))
        }
        model::RegistrationState::Excluded { pattern } => {
            format!("excluded · by {}", escape(pattern))
        }
        model::RegistrationState::Unregistered => {
            "unregistered · this path satisfies no declaration".to_owned()
        }
        model::RegistrationState::Pattern => "wildcard · declaration pattern".to_owned(),
    };
    field(
        "Registration",
        registration,
        registration_style(&row.registration),
    );
    if let Some(declaration) = &row.declaration {
        let kind = if declaration.contains('*') {
            "wildcard"
        } else {
            "explicit"
        };
        let target = rows
            .iter()
            .find(|other| {
                matches!(&other.key, model::RowKey::Declaration { .. })
                    && identity(other) == identity(row)
            })
            .map(|declared| match declared.checkout {
                model::CheckoutState::Cloned => format!(
                    ", satisfied by {}",
                    home_relative(Path::new(&declared.path), home)
                ),
                ref state => format!(", configured path {}", state.label()),
            })
            .unwrap_or_default();
        field(
            "Declaration",
            format!("{} ({kind}){target}", escape(declaration)),
            Style::default(),
        );
    }
    field(
        "Checkout",
        checkout_text(&row.checkout).to_owned(),
        checkout_style(&row.checkout),
    );
    field("Origin", escape(&row.origin), Style::default());
    for (name, observation, gloss) in [
        ("Branch", &row.branch, "not observed yet"),
        ("Changes", &row.changes, "status not observed yet"),
    ] {
        let value = match observation {
            model::ObservationField::Pending => format!("pending · {gloss}"),
            other => observation_text(other),
        };
        field(name, value, observation_style(observation));
    }
    if !row.warnings.is_empty() {
        lines.push(Line::default());
        lines.extend(
            row.warnings
                .iter()
                .map(|warning| Line::styled(escape(warning), warn())),
        );
    }
    lines.push(Line::default());
    for action in [BatchAction::Register, BatchAction::Unregister] {
        let state = match batch::eligibility(row, action, config) {
            Eligibility::Available => "available".to_owned(),
            Eligibility::Noop(reason) => format!("no-op · {reason}"),
            Eligibility::Unavailable(reason) => format!("unavailable · {reason}"),
        };
        lines.extend(field_lines(action.label(), &state, dim(), width));
    }
    let clone = config
        .resolve_root(home)
        .map_err(|_| "configured root is unavailable")
        .and_then(|root| {
            batch::clone_preview(row, config, &root, rows, true, &LocalInventoryFilesystem)
        });
    let clone = match clone.as_ref().map(|preview| {
        (
            preview.eligibility(AddChoice::Clone),
            row.observed_path
                .as_ref()
                .is_some_and(|found| *found != preview.destination),
            home_relative(&preview.destination, home),
        )
    }) {
        Ok((Eligibility::Available, true, destination)) => {
            format!("available · clones at {destination}, not at the found path")
        }
        Ok((Eligibility::Available, ..)) => "available".to_owned(),
        Ok((Eligibility::Noop(reason), ..)) => format!("no-op · {reason}"),
        Ok((Eligibility::Unavailable(reason), ..)) | Err(&reason) => {
            format!("unavailable · {reason}")
        }
    };
    lines.extend(field_lines("Clone", &clone, dim(), width));
    let remove = match (
        batch::removal_eligibility(row, BatchAction::Remove, config),
        batch::removal_eligibility(row, BatchAction::RemoveUnregister, config),
    ) {
        (Eligibility::Unavailable(reason), Eligibility::Available) => {
            format!("unavailable · {reason} · Remove + unregister is available")
        }
        (Eligibility::Unavailable(reason), _) => format!("unavailable · {reason}"),
        _ => "available · removes the found path".to_owned(),
    };
    lines.extend(field_lines("Remove", &remove, dim(), width));
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Root,
    Repository,
    Copy,
    Selection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Open(Menu),
    Inspect,
    OpenOrigin,
    Refresh,
    Problems,
    Add,
    Help,
    Quit,
    /// Reports the message instead of acting.
    Unavailable(&'static str),
    Copy(CopyField),
    ToggleMark,
    MarkVisible,
    ClearMarks,
    Run(BatchAction),
    CancelBatch,
}

/// One popup line: a group heading or spacer without an entry, or an entry with its hint.
struct MenuLine {
    entry: Option<Entry>,
    label: String,
    hint: String,
    enabled: bool,
}

fn menu_title(menu: Menu, name: &str, marked: usize) -> String {
    match menu {
        Menu::Root => "All actions".to_owned(),
        Menu::Repository => format!("Repository actions / {name}"),
        Menu::Copy => format!("Copy value / {name}"),
        Menu::Selection => format!("Selection / {marked} marked"),
    }
}

/// The menu's own key for an action, else the normal-mode shortcut that reaches it.
fn menu_key(bindings: &Bindings, action: Action) -> String {
    let menu = bindings.keys(Mode::Menu, action);
    let normal = if action.available(Mode::Normal) {
        bindings.keys(Mode::Normal, action)
    } else {
        Vec::new()
    };
    menu.first()
        .or(normal.first())
        .map(|key| (*key).to_owned())
        .unwrap_or_default()
}

fn menu_lines(
    menu: Menu,
    row: Option<&model::InventoryRow>,
    name: &str,
    session: &InventorySession,
    visible: &[&model::InventoryRow],
    rows: &[model::InventoryRow],
) -> Vec<MenuLine> {
    let heading = |label: String| MenuLine {
        entry: None,
        label,
        hint: String::new(),
        enabled: false,
    };
    let item = |entry, label: &str, hint: String, enabled| MenuLine {
        entry: Some(entry),
        label: label.to_owned(),
        hint,
        enabled,
    };
    let on_row = |entry, label: &str, hint: String| match row {
        Some(_) => item(entry, label, hint, true),
        None => item(entry, label, "no row".to_owned(), false),
    };
    let key = |action| menu_key(&session.bindings, action);
    // Unregistering changes every checkout of an identity; all of them must be known.
    let scanning = |action| {
        action == BatchAction::RemoveUnregister && (!session.complete || session.scan.incomplete)
    };
    match menu {
        Menu::Root => {
            let problems = session.problems.entries().len();
            let mark = key(Action::Mark);
            vec![
                heading(format!(
                    "This row · {}",
                    if row.is_some() { name } else { "—" }
                )),
                on_row(
                    Entry::Open(Menu::Repository),
                    "Repository actions…",
                    "clone, register, unregister, remove".to_owned(),
                ),
                on_row(Entry::Inspect, "Inspect repository", key(Action::Inspect)),
                on_row(Entry::Open(Menu::Copy), "Copy value…", String::new()),
                on_row(Entry::OpenOrigin, "Open origin in browser", String::new()),
                on_row(
                    Entry::Unavailable("Worktrees are unavailable in this build"),
                    "Worktrees…",
                    String::new(),
                ),
                heading(String::new()),
                heading(format!("Selection · {} marked", session.marks.len())),
                item(
                    Entry::Open(Menu::Selection),
                    "Selection…",
                    if mark.is_empty() {
                        mark
                    } else {
                        format!("{mark} marks the row")
                    },
                    true,
                ),
                heading(String::new()),
                heading("Session".to_owned()),
                item(Entry::Add, "Add repository", key(Action::Add), true),
                item(
                    Entry::Unavailable("Remote discovery is unavailable in this build"),
                    "Remote discovery…",
                    "off".to_owned(),
                    true,
                ),
                item(
                    Entry::Refresh,
                    "Refresh local / config",
                    key(Action::Refresh),
                    true,
                ),
                item(
                    Entry::Problems,
                    "Session problems…",
                    match problems {
                        0 => "none".to_owned(),
                        1 => "1 problem".to_owned(),
                        count => format!("{count} problems"),
                    },
                    problems > 0,
                ),
                match session.batch.as_ref().filter(|batch| !batch.finished()) {
                    Some(batch) => item(
                        Entry::CancelBatch,
                        "Cancel active batch",
                        format!("{} / {} processed", batch.processed(), batch.targets.len()),
                        true,
                    ),
                    None => item(
                        Entry::Unavailable("No batch is active"),
                        "Cancel active batch",
                        "no batch".to_owned(),
                        false,
                    ),
                },
                item(Entry::Help, "Help / keybindings", key(Action::Help), true),
                item(Entry::Quit, "Quit", key(Action::Quit), true),
            ]
        }
        Menu::Repository => [
            BatchAction::Register,
            BatchAction::Unregister,
            BatchAction::Clone,
            BatchAction::CloneRegister,
            BatchAction::Remove,
            BatchAction::RemoveUnregister,
        ]
        .into_iter()
        .map(|action| {
            let entry = Entry::Run(action);
            let label = action.label();
            let Some(row) = row.filter(|_| session.marks.is_empty()) else {
                return if session.marks.is_empty() {
                    on_row(entry, label, String::new())
                } else if scanning(action) {
                    item(entry, label, "the scan is not complete".to_owned(), false)
                } else {
                    item(
                        entry,
                        label,
                        format!("{} marked", session.marks.len()),
                        true,
                    )
                };
            };
            let mut elsewhere = None;
            let eligibility = if action.clones() {
                match batch::clone_preview(
                    row,
                    &session.config,
                    &session.root,
                    rows,
                    session.complete,
                    &LocalInventoryFilesystem,
                ) {
                    Ok(preview) => {
                        elsewhere = row
                            .observed_path
                            .as_ref()
                            .is_some_and(|found| *found != preview.destination)
                            .then(|| home_relative(&preview.destination, &session.home));
                        preview.eligibility(if action == BatchAction::Clone {
                            AddChoice::Clone
                        } else {
                            AddChoice::CloneRegister
                        })
                    }
                    Err(reason) => Eligibility::Unavailable(reason),
                }
            } else if scanning(action) {
                Eligibility::Unavailable("the scan is not complete")
            } else if action.removes() {
                batch::removal_eligibility(row, action, &session.config)
            } else {
                batch::eligibility(row, action, &session.config)
            };
            match (eligibility, elsewhere) {
                (Eligibility::Available, Some(destination)) => {
                    item(entry, label, format!("clones at {destination}"), true)
                }
                (Eligibility::Available, None) => item(entry, label, String::new(), true),
                (Eligibility::Noop(reason), _) => {
                    item(entry, label, format!("no-op · {reason}"), true)
                }
                (Eligibility::Unavailable(reason), _) => {
                    item(entry, label, reason.to_owned(), false)
                }
            }
        })
        .collect(),
        Menu::Copy => CopyField::ALL
            .into_iter()
            .map(|field| {
                let entry = Entry::Copy(field);
                match row.map(|row| model::copy_value(row, field)) {
                    Some(Ok(value)) => {
                        let hint = match field {
                            CopyField::Path => home_relative(Path::new(&value), &session.home),
                            _ => escape(value.to_string_lossy()),
                        };
                        item(entry, field.label(), hint, true)
                    }
                    Some(Err(reason)) => item(entry, field.label(), reason.to_owned(), false),
                    None => on_row(entry, field.label(), String::new()),
                }
            })
            .collect(),
        Menu::Selection => {
            let marked = session.marks.len();
            let markable = row.is_some_and(|row| row.markable);
            vec![
                item(
                    Entry::ToggleMark,
                    if row.is_some_and(|row| session.marks.contains(&row.key)) {
                        "Unmark this row"
                    } else {
                        "Mark this row"
                    },
                    match row {
                        Some(_) if markable => key(Action::Mark),
                        Some(_) => "patterns are inspect-only".to_owned(),
                        None => "no row".to_owned(),
                    },
                    markable,
                ),
                item(
                    Entry::MarkVisible,
                    "Mark visible concrete rows",
                    format!(
                        "{} visible",
                        visible.iter().filter(|row| row.markable).count()
                    ),
                    true,
                ),
                item(
                    Entry::ClearMarks,
                    "Clear all marks",
                    if marked == 0 {
                        "none marked".to_owned()
                    } else {
                        format!("{marked} marked")
                    },
                    marked > 0,
                ),
            ]
        }
    }
}

fn menu_widget(lines: &[MenuLine], selected: usize, width: u16) -> Paragraph<'static> {
    let width = usize::from(width);
    Paragraph::new(
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if line.entry.is_none() {
                    return Line::styled(line.label.clone(), dim());
                }
                choice_line(
                    &line.label,
                    &line.hint,
                    line.enabled,
                    index == selected,
                    width,
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// A choice with its hint at the right edge; the focused choice is reversed, a disabled one dim.
fn choice_line(
    label: &str,
    hint: &str,
    enabled: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let label_width = Line::raw(label.to_owned()).width();
    let hint = abbreviate(hint, width.saturating_sub(label_width + 2), false);
    let gap = width
        .saturating_sub(label_width + Line::raw(hint.as_str()).width())
        .max(1);
    let (label_style, hint_style) = if focused {
        let reversed = Style::default().add_modifier(Modifier::REVERSED);
        (reversed, reversed)
    } else if enabled {
        (Style::default(), dim())
    } else {
        (dim(), dim())
    };
    Line::from(vec![
        Span::styled(format!("{label}{}", " ".repeat(gap)), label_style),
        Span::styled(hint, hint_style),
    ])
}

/// Runs the selected menu entry. Returns the exit code when the entry quits.
fn activate_menu_entry(
    session: &mut InventorySession,
    selection: &mut Selection,
    report: &InventoryReport,
    visible_rows: &[&model::InventoryRow],
) -> Option<i32> {
    let (menu, index) = *selection.menu.last()?;
    let names = short_names(&report.rows);
    let row = selection.menu_target_row(&report.rows);
    let name = row.map(|row| names[&row.key].clone()).unwrap_or_default();
    let line = menu_lines(menu, row, &name, session, visible_rows, &report.rows)
        .into_iter()
        .filter(|line| line.entry.is_some())
        .nth(index)?;
    let entry = line.entry.filter(|_| line.enabled)?;
    match entry {
        Entry::Open(menu) => {
            selection.menu.push((menu, 0));
            selection.menu_scroll = 0;
            return None;
        }
        Entry::Help => {
            selection.help_mode = Mode::Menu;
            selection.help = true;
            selection.help_scroll = 0;
            return None;
        }
        Entry::Quit if session.running() => {
            selection.menu.clear();
            selection.ask_quit();
            return None;
        }
        Entry::Quit => return Some(quit(session)),
        _ => selection.menu.clear(),
    }
    session.message = match (entry, row) {
        (Entry::Inspect, _) => {
            selection.open_inspection(visible_rows);
            None
        }
        (Entry::Refresh, _) => {
            refresh(session);
            None
        }
        (Entry::Add, _) => {
            selection.add = Some(AddForm::default());
            None
        }
        (Entry::Problems, _) => {
            selection.problems = true;
            selection.problems_scroll = 0;
            None
        }
        (Entry::Unavailable(message), _) => Some(message.to_owned()),
        (Entry::OpenOrigin, Some(row)) => Some(
            match model::web_url(row, &session.config)
                .map_err(str::to_owned)
                .and_then(|url| NativeDesktop.open(&url).map(|()| url))
            {
                Ok(url) => format!("Opening {} in the browser", escape(url)),
                Err(reason) => format!("Open origin unavailable: {}", escape(reason)),
            },
        ),
        (Entry::Copy(field), Some(row)) => Some(
            match model::copy_value(row, field)
                .map_err(str::to_owned)
                .and_then(|value| NativeDesktop.copy(&value))
            {
                Ok(()) => format!("Copied {} of {name}", field.label().to_lowercase()),
                Err(reason) => format!("Copy unavailable: {}", escape(reason)),
            },
        ),
        (Entry::ToggleMark, Some(row)) => Some(if session.marks.toggle(row) {
            format!("Marked {name}")
        } else {
            format!("Unmarked {name}")
        }),
        (Entry::MarkVisible, _) => {
            session.marks.mark_all(visible_rows.iter().copied());
            Some(format!(
                "Marked visible concrete rows · {} marked",
                session.marks.len()
            ))
        }
        (Entry::ClearMarks, _) => Some(match session.marks.clear() {
            1 => "Cleared 1 mark".to_owned(),
            count => format!("Cleared {count} marks"),
        }),
        (Entry::CancelBatch, _) => {
            session.cancel_batch();
            // The outcomes show what was completed and what was left.
            selection.batch = true;
            selection.batch_cancel = false;
            return None;
        }
        (Entry::Run(_), _) if session.running() => Some("A batch is already running".to_owned()),
        (Entry::Run(action), row) if action.removes() => {
            confirm_removal(session, selection, report, visible_rows, row, action);
            return None;
        }
        (Entry::Run(action), _) => {
            let keys = if session.marks.is_empty() {
                row.map(|row| row.key.clone()).into_iter().collect()
            } else {
                session.marks.keys().cloned().collect::<Vec<_>>()
            };
            let targets = keys
                .into_iter()
                .map(|key| {
                    let hidden = !visible_rows.iter().any(|row| row.key == key);
                    match report.rows.iter().find(|row| row.key == key) {
                        Some(row) if action.clones() => BatchTarget::freeze_clone(
                            row,
                            names[&row.key].clone(),
                            hidden,
                            action,
                            &session.config,
                            &session.root,
                            &report.rows,
                            session.complete,
                            &LocalInventoryFilesystem,
                        ),
                        Some(row) => BatchTarget::freeze(
                            row,
                            names[&row.key].clone(),
                            hidden,
                            action,
                            &session.config,
                        ),
                        None => unlisted_target(key, hidden, &session.home),
                    }
                })
                .collect::<Vec<_>>();
            // A clone that lands away from the found checkout is previewed even for one row.
            let direct =
                session.marks.is_empty() && targets.iter().all(|target| target.elsewhere.is_none());
            let batch = Batch::new(action, targets);
            if direct {
                start_batch(session, selection, batch);
            } else {
                selection.preview = Some(batch);
                selection.preview_fresh = true;
                selection.preview_focus = 0;
                selection.preview_scroll = 0;
            }
            None
        }
        _ => None,
    };
    None
}

/// A marked entry the inventory no longer lists: it stays in the batch, blocked.
fn unlisted_target(key: model::RowKey, hidden: bool, home: &Path) -> BatchTarget {
    BatchTarget {
        // Kept so a marked checkout that is not removed still holds back its unregister.
        identity: match &key {
            model::RowKey::Declaration { identity, .. }
            | model::RowKey::Checkout { identity, .. } => Some(identity.clone()),
            model::RowKey::Path(_) | model::RowKey::Pattern(_) => None,
        },
        name: match &key {
            model::RowKey::Path(path) => home_relative(path, home),
            model::RowKey::Declaration { identity, .. }
            | model::RowKey::Checkout { identity, .. }
            | model::RowKey::Pattern(identity) => escape(identity),
        },
        key,
        path: None,
        found: false,
        reference: None,
        hidden,
        blocked: Some("the entry is no longer listed".to_owned()),
        noop: None,
        registration: Default::default(),
        elsewhere: None,
        removal: Default::default(),
    }
}

/// Opens the removal confirmation for the marked rows, or for `row` when none are marked.
/// Every target is frozen here with the answer of the removal guards; nothing is deleted
/// before the confirmation is accepted.
fn confirm_removal(
    session: &mut InventorySession,
    selection: &mut Selection,
    report: &InventoryReport,
    visible_rows: &[&model::InventoryRow],
    row: Option<&model::InventoryRow>,
    action: BatchAction,
) {
    if session.running() {
        session.message = Some("A batch is already running".to_owned());
        return;
    }
    // Unregistering changes every checkout of an identity; all of them must be known.
    if action == BatchAction::RemoveUnregister && (!session.complete || report.incomplete) {
        session.message = Some(format!(
            "{} unavailable: the scan is not complete, other checkouts of an identity are not known yet",
            action.label()
        ));
        return;
    }
    let keys = if session.marks.is_empty() {
        let Some(row) = row else {
            return;
        };
        if let Eligibility::Unavailable(reason) =
            batch::removal_eligibility(row, action, &session.config)
        {
            session.message = Some(format!("{} unavailable: {reason}", action.label()));
            return;
        }
        vec![row.key.clone()]
    } else {
        session.marks.keys().cloned().collect::<Vec<_>>()
    };
    let names = short_names(&report.rows);
    let targets = keys
        .iter()
        .map(|key| {
            let hidden = !visible_rows.iter().any(|row| row.key == *key);
            match report.rows.iter().find(|row| row.key == *key) {
                Some(row) => BatchTarget::freeze_removal(
                    row,
                    names[&row.key].clone(),
                    hidden,
                    action,
                    &session.config,
                    &session.root,
                    &report.rows,
                    &keys,
                    &NativeGit,
                    &Filesystem,
                ),
                None => unlisted_target(key.clone(), hidden, &session.home),
            }
        })
        .collect();
    let batch = Batch::new(action, targets);
    // Cancel is the default; the acknowledgment sits above it.
    selection.preview_focus = usize::from(acknowledgment(&batch).is_some());
    selection.preview_scroll = 0;
    selection.preview_fresh = true;
    selection.preview = Some(batch);
}

/// The names of the targets whose listed losses the confirmation asks to acknowledge.
fn acknowledgment(batch: &Batch) -> Option<Vec<&str>> {
    let names = batch
        .targets
        .iter()
        .filter(|target| target.blocked.is_none() && !target.removal.losses.is_empty())
        .map(|target| target.name.as_str())
        .collect::<Vec<_>>();
    (batch.action.removes() && !names.is_empty()).then_some(names)
}

/// The Add form: the typed reference, why the last one was rejected, and its preview with the
/// cursor on one of the preview's choices.
#[derive(Default)]
struct AddForm {
    text: String,
    rejected: Option<String>,
    preview: Option<AddPreview>,
    choice: usize,
}

/// A spacer and the four preview choices, kept in view before the note and the fields.
const ADD_CHOICES_HEIGHT: u16 = 5;

/// The preview's choices in board order; the cursor index past them is "Back to reference".
const ADD_CHOICES: [(AddChoice, &str); 3] = [
    (AddChoice::Register, "configuration only"),
    (AddChoice::Clone, "files only"),
    (AddChoice::CloneRegister, "files, then configuration"),
];

fn add_input_widget(
    form: &AddForm,
    marked: usize,
    bindings: &Bindings,
    width: u16,
) -> Paragraph<'static> {
    let width = usize::from(width);
    let mut lines = vec![
        Line::raw("Repository reference"),
        Line::styled(format!("{}█", escape(&form.text)), focus()),
    ];
    if let Some(reason) = &form.rejected {
        lines.push(Line::styled(
            format!("Rejected: {}", escape(reason)),
            tint(Color::Red),
        ));
    }
    lines.extend([
        Line::styled(
            "Shorthand, canonical identity or a supported clone URL. One concrete repository, no wildcards. Nothing is saved yet.",
            dim(),
        ),
        Line::default(),
        choice_line(
            "Preview destination",
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
    if marked > 0 {
        lines.extend([
            Line::default(),
            Line::styled(
                if marked == 1 {
                    "1 marked row is not a target of Add.".to_owned()
                } else {
                    format!("{marked} marked rows are not targets of Add.")
                },
                dim(),
            ),
        ]);
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// The preview's fields, and its choices with the closing note; the choices stay in view when
/// long values leave too little room.
fn add_preview_lines(
    preview: &AddPreview,
    choice: usize,
    marked: usize,
    home: &Path,
    bindings: &Bindings,
    width: u16,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let checkout = match preview.checkout {
        model::CheckoutState::Missing => "missing · verified just now",
        model::CheckoutState::Cloned => "cloned · matching checkout at the destination",
        model::CheckoutState::Conflict => "conflict · another checkout is at the destination",
        model::CheckoutState::Unreadable => "unreadable · the state is unknown",
        _ if preview.pending => "unknown · not observed yet",
        _ => "unknown · no checkout observed here",
    };
    let checkout_style = match preview.checkout {
        model::CheckoutState::Cloned => Style::default(),
        model::CheckoutState::Missing => warn(),
        _ => tint(Color::Red),
    };
    let fields = [
        ("Reference", escape(&preview.reference), Style::default()),
        ("Identity", escape(&preview.identity), Style::default()),
        (
            "Destination",
            home_relative(&preview.destination, home),
            Style::default(),
        ),
        ("Checkout", checkout.to_owned(), checkout_style),
        (
            "Registration",
            preview.registration.label().to_owned(),
            dim(),
        ),
        (
            "Post-clone hook",
            preview
                .hook
                .as_deref()
                .map_or_else(|| "none declared".to_owned(), escape),
            dim(),
        ),
        (
            "Existing marks",
            format!("{marked} · unaffected by this action"),
            Style::default(),
        ),
    ]
    .into_iter()
    .flat_map(|(name, value, style)| field_lines(name, &value, style, width))
    .collect::<Vec<_>>();
    let mut lines = vec![Line::default()];
    let width = usize::from(width);
    for (index, (action, hint)) in ADD_CHOICES.into_iter().enumerate() {
        let (hint, enabled) = match preview.eligibility(action) {
            Eligibility::Available => (hint.to_owned(), true),
            Eligibility::Noop(reason) => (format!("no-op · {reason}"), true),
            Eligibility::Unavailable(reason) => (reason.to_owned(), false),
        };
        lines.push(choice_line(
            action.label(),
            &hint,
            enabled,
            index == choice,
            width,
        ));
    }
    lines.extend([
        choice_line(
            "Back to reference",
            &bindings.keys(Mode::Confirmation, Action::Cancel).join("/"),
            true,
            choice == ADD_CHOICES.len(),
            width,
        ),
        Line::default(),
        Line::styled(
            "An unknown checkout state disables Clone until it is observed. It is never shown as missing.",
            dim(),
        ),
    ]);
    (fields, lines)
}

/// Each choice runs as a one-target batch of its action.
fn choose_add(session: &mut InventorySession, selection: &mut Selection) {
    let Some(form) = selection.add.as_mut() else {
        return;
    };
    let Some(preview) = &form.preview else {
        return;
    };
    let Some((choice, _)) = ADD_CHOICES.get(form.choice).copied() else {
        form.preview = None;
        return;
    };
    let label = choice.label();
    session.message = Some(match preview.eligibility(choice) {
        Eligibility::Unavailable(reason) => format!("{label} unavailable: {reason}"),
        _ if session
            .batch
            .as_ref()
            .is_some_and(|batch| !batch.finished()) =>
        {
            "A batch is already running".to_owned()
        }
        _ => {
            let name = escape(&preview.identity);
            let (action, target) = match choice {
                AddChoice::Register => (BatchAction::Register, preview.target(name)),
                AddChoice::Clone => (
                    BatchAction::Clone,
                    preview.clone_target(BatchAction::Clone, name),
                ),
                AddChoice::CloneRegister => (
                    BatchAction::CloneRegister,
                    preview.clone_target(BatchAction::CloneRegister, name),
                ),
            };
            selection.add = None;
            start_batch(
                session,
                selection,
                Batch {
                    previewed: true,
                    ..Batch::new(action, vec![target])
                },
            );
            return;
        }
    });
}

/// What an action touches, under its preview and its outcomes.
fn batch_note(action: BatchAction) -> &'static str {
    match action {
        BatchAction::Register | BatchAction::Unregister => {
            "Registration changes configuration only. Checkouts and their files stay as they are."
        }
        BatchAction::Clone => {
            "Clone only creates checkouts at configured destinations. Configuration stays as it is. Git owns the terminal while it clones."
        }
        BatchAction::CloneRegister => {
            "A target registers only after its clone succeeded or was a no-op. A failed configuration write does not undo a checkout. Git owns the terminal while it clones."
        }
        BatchAction::Remove => {
            "Registration and exclusions stay unchanged. ensure can recreate these checkouts."
        }
        BatchAction::RemoveUnregister => {
            "An identity is unregistered only after every selected checkout of it is gone. A failed configuration write does not undo a deletion."
        }
    }
}

fn start_batch(session: &mut InventorySession, selection: &mut Selection, batch: Batch) {
    session.batch = Some(batch);
    selection.batch = true;
    selection.batch_cancel = false;
    selection.batch_scroll = 0;
}

fn toggle_mark(
    session: &mut InventorySession,
    row: &model::InventoryRow,
    rows: &[model::InventoryRow],
) {
    if row.markable {
        session.marks.toggle(row);
    } else {
        session.message = Some(format!(
            "{} cannot be marked: patterns are inspect-only",
            short_names(rows)[&row.key]
        ));
    }
}

fn refresh(session: &mut InventorySession) {
    if let Err(error) = session.refresh() {
        session.problems.record(
            "refresh",
            format!(
                "{} · previous configuration kept",
                escape(model::problem_reason(&error))
            ),
        );
        session.notice = Some(format!("refresh failed: {}", escape(error)));
    }
}

fn quit(session: &mut InventorySession) -> i32 {
    session.worker.cancel_and_join();
    session.receive_events();
    session.problems.exit_code()
}

fn problems_widget(session: &InventorySession) -> Paragraph<'static> {
    let line =
        |time: &str, source: &str, problem: &str| format!("{time:<10} {source:<15} {problem}");
    let mut lines = vec![Line::styled(line("time", "source", "problem"), dim())];
    lines.extend(
        session
            .problems
            .entries()
            .iter()
            .map(|problem| Line::raw(line(&clock(problem.at), &problem.source, &problem.message))),
    );
    lines.extend([
        Line::default(),
        Line::styled(
            "Quit returns 1 because problems occurred in this session. A successful retry stays listed.",
            dim(),
        ),
        Line::default(),
        Line::styled(
            "Declined confirmations, cancellations and missing optional tools are not problems.",
            dim(),
        ),
    ]);
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Succeeded => "SUCCEEDED",
        Outcome::Noop => "NO-OP",
        Outcome::Failed => "FAILED",
        Outcome::Blocked => "BLOCKED",
        Outcome::Cancelled => "CANCELLED",
        Outcome::Unattempted => "UNATTEMPTED",
    }
}

/// The outcomes of a batch. While it runs, two choices follow them: return to browsing, the
/// default, and `cancel` focuses cancelling the remaining work.
fn batch_widget(batch: &Batch, home: &Path, width: u16, cancel: bool) -> Paragraph<'static> {
    let active = batch.active();
    let hidden = batch.targets.iter().filter(|target| target.hidden).count();
    let mut summary = format!("{} / {} processed", batch.processed(), batch.targets.len());
    if hidden > 0 {
        summary.push_str(&format!(" · {hidden} hidden by filter"));
    }
    if let Some(path) = active.and_then(|index| batch.targets[index].path.as_ref()) {
        summary.push_str(&format!(" · active: {}", home_relative(path, home)));
    }
    let name_width = batch
        .targets
        .iter()
        .map(|target| Line::raw(target.name.clone()).width())
        .max()
        .unwrap_or(0)
        .max(11);
    let cells = |name: &str, disk: &str, config: &str| {
        let padding = name_width.saturating_sub(Line::raw(name.to_owned()).width());
        format!("{name}{} {disk:<9} {config:<13} ", " ".repeat(padding))
    };
    let mut lines = vec![
        Line::raw(summary),
        Line::default(),
        Line::styled(
            format!(
                "{:<12} {}reason",
                "outcome",
                cells("repository", "disk", "config")
            ),
            dim(),
        ),
    ];
    let reason_column = 13 + Line::raw(cells("", "", "")).width();
    for (index, (target, result)) in batch.targets.iter().zip(&batch.results).enumerate() {
        let (label, style) = match result.outcome {
            _ if Some(index) == active => ("ACTIVE", warn()),
            Outcome::Succeeded => ("SUCCEEDED", focus()),
            Outcome::Failed | Outcome::Blocked => (outcome_label(result.outcome), tint(Color::Red)),
            outcome => (outcome_label(outcome), dim()),
        };
        let reason = [
            escape(&result.reason),
            batch
                .shared_with(index)
                .map(|earlier| format!("shared with {}", batch.targets[earlier].name))
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        // A name too long to leave a reason column moves the reason below its row.
        let below = usize::from(width).saturating_sub(reason_column) < 20;
        let indent = if below { 13 } else { reason_column };
        let mut chunks = column_chunks(&reason, usize::from(width).saturating_sub(indent));
        let first = if below || chunks.is_empty() {
            String::new()
        } else {
            chunks.remove(0)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<12} "), style),
            Span::raw(cells(&target.name, &result.disk, &result.config)),
            Span::styled(first, dim()),
        ]));
        lines.extend(
            chunks
                .into_iter()
                .filter(|chunk| !chunk.is_empty())
                .map(|chunk| {
                    Line::from(vec![
                        Span::raw(" ".repeat(indent)),
                        Span::styled(chunk, dim()),
                    ])
                }),
        );
    }
    lines.extend([
        Line::default(),
        Line::styled(batch_note(batch.action), dim()),
    ]);
    if !batch.finished() {
        let width = usize::from(width);
        lines.extend([
            Line::default(),
            choice_line(
                "Return to browsing",
                "batch continues",
                true,
                !cancel,
                width,
            ),
            choice_line(
                "Cancel remaining work",
                "completed changes remain",
                true,
                cancel,
                width,
            ),
        ]);
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// The prompt shown when quitting while a batch runs. Returning is the default.
fn quit_widget(batch: &Batch, cancel: bool, width: u16) -> Paragraph<'static> {
    let left = batch
        .targets
        .iter()
        .zip(&batch.results)
        .filter(|(_, result)| result.outcome == Outcome::Unattempted)
        .map(|(target, _)| target.name.as_str())
        .collect::<Vec<_>>();
    let names = match left.as_slice() {
        [] => "nothing".to_owned(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [first, second, rest @ ..] => format!("{first}, {second} and {} more", rest.len()),
    };
    let problems = batch
        .results
        .iter()
        .filter(|result| matches!(result.outcome, Outcome::Failed | Outcome::Blocked))
        .count();
    let mut consequence = format!("Cancelling leaves {names} unattempted.");
    if problems > 0 {
        consequence.push_str(&format!(
            " The session exits 1 because {problems} {} failed or were blocked.",
            if problems == 1 { "target" } else { "targets" }
        ));
    }
    let width = usize::from(width);
    Paragraph::new(vec![
        Line::raw(format!(
            "{} of {} targets processed. Completed changes remain.",
            batch.processed(),
            batch.targets.len()
        )),
        Line::default(),
        Line::styled(consequence, dim()),
        Line::default(),
        choice_line("Return to inventory", "default", true, !cancel, width),
        choice_line("Cancel remaining work and quit", "", true, cancel, width),
    ])
    .wrap(Wrap { trim: false })
}

/// Every frozen target before the batch runs: its exact path and identity, and whether it is
/// eligible, a no-op or blocked, with the reason.
fn preview_lines(batch: &Batch, home: &Path) -> Vec<Line<'static>> {
    let hidden = batch.targets.iter().filter(|target| target.hidden).count();
    let mut summary = format!(
        "{} exact {}",
        batch.targets.len(),
        if batch.targets.len() == 1 {
            "target"
        } else {
            "targets"
        }
    );
    if hidden > 0 {
        summary.push_str(&format!(" · {hidden} hidden by filter"));
    }
    let mut lines = vec![
        Line::raw(summary),
        Line::styled(batch_note(batch.action), dim()),
    ];
    for target in &batch.targets {
        let (label, style, reason) = match (&target.blocked, &target.noop) {
            (Some(reason), _) => ("BLOCKED", tint(Color::Red), Some(reason)),
            (None, Some(reason)) => ("NO-OP", dim(), Some(reason)),
            (None, None) if !target.removal.losses.is_empty() => {
                ("LOSS ACKNOWLEDGMENT REQUIRED", warn(), None)
            }
            (None, None) => ("ELIGIBLE", focus(), None),
        };
        let mut heading = vec![
            Span::styled(label, style),
            Span::raw(format!(" {}", target.name)),
        ];
        if target.hidden {
            heading.push(Span::styled(" (hidden by filter)", dim()));
        }
        let path = target
            .path
            .as_ref()
            .map_or_else(|| "no path".to_owned(), |path| home_relative(path, home));
        let mut detail = vec![
            match &target.elsewhere {
                Some(found) => {
                    format!("found at {} · clones at {path}", home_relative(found, home))
                }
                None => path,
            },
            target
                .identity
                .as_deref()
                .map_or_else(|| "no identity".to_owned(), escape),
        ];
        detail.extend(reason.map(escape));
        if batch.action.removes() {
            detail.push(if target.blocked.is_some() {
                "force cannot waive this".to_owned()
            } else if !target.found {
                "known missing · absence is rechecked first".to_owned()
            } else if target.removal.losses.is_empty() {
                "clean".to_owned()
            } else {
                escape(target.removal.losses.join(", "))
            });
        }
        lines.extend([
            Line::default(),
            Line::from(heading),
            Line::styled(detail.join(" · "), dim()),
        ]);
        lines.extend(target.removal.occupies.iter().map(|identity| {
            Line::styled(
                format!(
                    "sits at the configured destination of {} · that declaration stays",
                    escape(identity)
                ),
                warn(),
            )
        }));
        lines.extend(target.removal.shared.iter().map(|path| {
            Line::styled(
                format!(
                    "also unregisters {} · not selected, its files stay",
                    home_relative(path, home)
                ),
                warn(),
            )
        }));
    }
    lines
}

/// The preview's choices with one cursor at `focus`: the loss acknowledgment when a removal
/// lists losses, then Cancel, then the action.
fn preview_choices(batch: &Batch, focus: usize, bindings: &Bindings) -> Vec<Line<'static>> {
    let count = |noop: bool| {
        batch
            .targets
            .iter()
            .filter(|target| target.blocked.is_none() && target.noop.is_some() == noop)
            .count()
    };
    let (changing, noop) = (count(false), count(true));
    let blocked = batch.targets.len() - changing - noop;
    let unchanged = [(noop, "no-op"), (blocked, "blocked")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>();
    let choice = |label: String, hint: String, focused: bool| {
        let style = if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(label, style),
            Span::styled(format!("  {hint}"), dim()),
        ])
    };
    let mut lines = Vec::new();
    if let Some(names) = acknowledgment(batch) {
        lines.push(choice(
            format!(
                "[{}] Also remove {} and accept {} listed losses",
                if batch.acknowledged { 'x' } else { ' ' },
                match names.as_slice() {
                    [name] => (*name).to_owned(),
                    names => format!("{} checkouts", names.len()),
                },
                if names.len() == 1 { "its" } else { "their" }
            ),
            bindings
                .keys(Mode::Confirmation, Action::Acknowledge)
                .first()
                .map(|key| (*key).to_owned())
                .unwrap_or_default(),
            focus == 0,
        ));
    }
    let cancel = lines.len();
    lines.push(choice(
        "Cancel".to_owned(),
        "default".to_owned(),
        focus == cancel,
    ));
    lines.push(if batch.action.removes() {
        let run = batch
            .targets
            .iter()
            .filter(|target| batch.removable(target))
            .collect::<Vec<_>>();
        let found = run.iter().filter(|target| target.found).count();
        choice(
            if batch.action == BatchAction::Remove {
                format!(
                    "Remove {found} {}",
                    if found == 1 { "checkout" } else { "checkouts" }
                )
            } else {
                format!(
                    "Remove + unregister {} {}",
                    run.len(),
                    if run.len() == 1 { "target" } else { "targets" }
                )
            },
            if run.is_empty() {
                "nothing is eligible".to_owned()
            } else {
                format!(
                    "{} · cannot be undone",
                    run.iter()
                        .map(|target| target.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            focus > cancel,
        )
    } else {
        choice(
            format!(
                "{} {changing} {}",
                batch.action.label(),
                if changing == 1 { "target" } else { "targets" }
            ),
            if unchanged.is_empty() {
                match batch.action {
                    BatchAction::Clone => "files only",
                    BatchAction::CloneRegister => "files, then configuration",
                    _ => "configuration only",
                }
                .to_owned()
            } else {
                format!("{} stay unchanged", unchanged.join(" · "))
            },
            focus > cancel,
        )
    });
    lines
}

/// The preview popup's regions, its scrolling target list, and the choices pinned below it.
fn preview_regions(area: Rect, list: &[Line<'static>], choices: u16) -> ([Rect; 4], Rect, Rect) {
    let mut sizing = list.to_vec();
    sizing.extend(std::iter::repeat_n(
        Line::default(),
        usize::from(choices) + 1,
    ));
    let regions = popup_regions(
        area,
        WIDE_POPUP,
        &Paragraph::new(sizing).wrap(Wrap { trim: false }),
    );
    let [list, _, choices] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(choices),
    ])
    .areas(regions[2]);
    (regions, list, choices)
}

/// Local wall-clock time of day, or UTC where the platform offers no local time zone.
fn clock(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    #[cfg(unix)]
    {
        let time = seconds as libc::time_t;
        // SAFETY: `localtime_r` writes only to the zeroed struct it is given.
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&time, &mut local) }.is_null() {
            return format!(
                "{:02}:{:02}:{:02}",
                local.tm_hour, local.tm_min, local.tm_sec
            );
        }
    }
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn registration_style(state: &model::RegistrationState) -> Style {
    match state {
        model::RegistrationState::Explicit => tint(Color::Green),
        model::RegistrationState::Wildcard { .. } | model::RegistrationState::Pattern => {
            tint(Color::Blue)
        }
        model::RegistrationState::Excluded { .. } => warn(),
        model::RegistrationState::Unregistered => dim(),
    }
}

fn checkout_style(state: &model::CheckoutState) -> Style {
    match state {
        model::CheckoutState::Cloned | model::CheckoutState::Pattern => Style::default(),
        model::CheckoutState::Missing | model::CheckoutState::Unknown => warn(),
        model::CheckoutState::Conflict | model::CheckoutState::Unreadable => tint(Color::Red),
    }
}

fn observation_style(field: &model::ObservationField) -> Style {
    match field {
        model::ObservationField::Clean => tint(Color::Green),
        model::ObservationField::Pending | model::ObservationField::Stale(_) => warn(),
        model::ObservationField::Error(_) => tint(Color::Red),
        model::ObservationField::Known(_)
        | model::ObservationField::Dirty(_)
        | model::ObservationField::NotApplicable => Style::default(),
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        self.restore.restore();
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RepositoryPath {
    identity: String,
    path: PathBuf,
}

#[derive(Default)]
struct Selection {
    key: Option<model::RowKey>,
    repository_path: Option<RepositoryPath>,
    pending_path: Option<PathBuf>,
    table: TableState,
    search: bool,
    query: String,
    /// Open menus, innermost last, each with its entry cursor.
    menu: Vec<(Menu, usize)>,
    /// The row the open menu acts on, fixed when the menu opens.
    menu_target: Option<model::RowKey>,
    menu_scroll: u16,
    problems: bool,
    problems_scroll: u16,
    batch: bool,
    /// The outcomes popup of a running batch focuses "Cancel remaining work".
    batch_cancel: bool,
    batch_scroll: u16,
    /// Quitting was asked for while a batch runs; the prompt is open.
    quit: bool,
    /// The quit prompt focuses "Cancel remaining work and quit" instead of returning.
    quit_cancel: bool,
    /// Targets frozen for a batch that has not started; refreshes never change them.
    preview: Option<Batch>,
    /// The preview has not been drawn yet; input typed before it is discarded.
    preview_fresh: bool,
    /// The focused preview choice, counted from the top one.
    preview_focus: usize,
    preview_scroll: u16,
    add: Option<AddForm>,
    help: bool,
    help_mode: Mode,
    help_scroll: u16,
    inspection: bool,
    inspection_key: Option<model::RowKey>,
    inspection_scroll: u16,
}

impl Selection {
    fn reconcile(&mut self, rows: &[&model::InventoryRow], complete: bool) {
        let matched = self
            .key
            .as_ref()
            .and_then(|key| rows.iter().position(|row| &row.key == key))
            .or_else(|| {
                self.repository_path.as_ref().and_then(|selected| {
                    rows.iter().position(|row| {
                        repository_path(row)
                            .as_ref()
                            .is_some_and(|candidate| candidate == selected)
                    })
                })
            })
            // Pending rows have no identity yet, so this transition remains path-only.
            .or_else(|| {
                self.pending_path.as_ref().and_then(|path| {
                    rows.iter()
                        .position(|row| row.observed_path.as_ref() == Some(path))
                })
            });
        let index = matched.or_else(|| {
            (!rows.is_empty()).then_some(
                self.table
                    .selected()
                    .unwrap_or(0)
                    .min(rows.len().saturating_sub(1)),
            )
        });
        self.table.select(index);
        // During a scan, a provisional row must not replace the remembered target.
        if let Some(index) = index
            && (matched.is_some() || complete || self.key.is_none())
        {
            self.remember(Some(rows[index]));
        }
    }

    fn selected_row<'a>(
        &self,
        rows: &'a [&model::InventoryRow],
    ) -> Option<&'a model::InventoryRow> {
        self.table
            .selected()
            .and_then(|index| rows.get(index).copied())
    }

    /// Inspection is anchored to the rendered row key, not the table index. Background updates
    /// and refreshes can reorder rows, but must never retarget an open inspection.
    fn inspected_row<'a>(
        &self,
        rows: &'a [&model::InventoryRow],
    ) -> Option<&'a model::InventoryRow> {
        self.inspection_key
            .as_ref()
            .and_then(|key| rows.iter().find(|row| row.key == *key).copied())
    }

    fn open_inspection(&mut self, rows: &[&model::InventoryRow]) {
        let Some(row) = self.selected_row(rows) else {
            return;
        };
        self.inspection = true;
        self.inspection_key = Some(row.key.clone());
        self.inspection_scroll = 0;
    }

    fn close_inspection(&mut self) {
        self.inspection = false;
        self.inspection_key = None;
        self.inspection_scroll = 0;
    }

    /// Opens the quit prompt with returning to the inventory focused.
    fn ask_quit(&mut self) {
        self.quit = true;
        self.quit_cancel = false;
    }

    fn view(&self) -> View {
        if self.help {
            View::Help
        } else if self.quit {
            View::QuitBatch
        } else if !self.menu.is_empty() {
            View::Menu
        } else if let Some(add) = &self.add {
            if add.preview.is_some() {
                View::AddPreview
            } else {
                View::AddInput
            }
        } else if self.preview.is_some() {
            View::Preview
        } else if self.batch {
            View::Batch
        } else if self.problems {
            View::Problems
        } else if self.inspection {
            View::Inspect
        } else if self.search {
            View::Search
        } else {
            View::Normal
        }
    }

    fn open_menu(&mut self, menu: Menu, rows: &[&model::InventoryRow]) {
        self.menu = vec![(menu, 0)];
        self.menu_target = self.selected_row(rows).map(|row| row.key.clone());
        self.menu_scroll = 0;
    }

    fn menu_target_row<'a>(
        &self,
        rows: &'a [model::InventoryRow],
    ) -> Option<&'a model::InventoryRow> {
        let key = self.menu_target.as_ref()?;
        rows.iter().find(|row| &row.key == key)
    }

    /// Moves the cursor; drawing clamps it to the entries of the open menu.
    fn move_menu_by(&mut self, delta: isize) {
        if let Some((_, index)) = self.menu.last_mut() {
            *index = index.saturating_add_signed(delta);
        }
    }

    fn clamp_menu_index(&mut self, entries: usize) -> usize {
        let Some((_, index)) = self.menu.last_mut() else {
            return 0;
        };
        *index = (*index).min(entries.saturating_sub(1));
        *index
    }

    fn clamp_menu_scroll(&mut self, limit: u16, viewport: u16, selected: u16) {
        self.menu_scroll = self.menu_scroll.min(limit);
        if selected < self.menu_scroll {
            self.menu_scroll = selected;
        } else if selected >= self.menu_scroll.saturating_add(viewport) {
            self.menu_scroll = selected.saturating_add(1).saturating_sub(viewport);
        }
    }

    fn remember(&mut self, row: Option<&model::InventoryRow>) {
        self.key = row.map(|row| row.key.clone());
        self.repository_path = row.and_then(repository_path);
        self.pending_path = row
            .filter(|row| {
                matches!(row.key, model::RowKey::Path(_))
                    && row.repository == row.path
                    && row.origin == "pending"
            })
            .and_then(|row| row.observed_path.clone());
    }

    fn move_by(&mut self, rows: &[&model::InventoryRow], delta: isize) {
        if rows.is_empty() {
            return;
        }
        let index = self
            .table
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(rows.len() - 1);
        self.table.select(Some(index));
        self.remember(Some(rows[index]));
    }
}

fn edit_text(text: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            text.push(character);
        }
        _ => {}
    }
}

/// Connects the checkout and declaration forms of the same repository without matching another
/// identity that happens to use the same path.
fn repository_path(row: &model::InventoryRow) -> Option<RepositoryPath> {
    match &row.key {
        model::RowKey::Declaration {
            identity,
            destination,
        } => Some(RepositoryPath {
            identity: identity.clone(),
            path: destination.clone(),
        }),
        model::RowKey::Checkout { identity, path } => Some(RepositoryPath {
            identity: identity.clone(),
            path: path.clone(),
        }),
        model::RowKey::Path(_) | model::RowKey::Pattern(_) => None,
    }
}

fn matching_rows<'a>(report: &'a InventoryReport, query: &str) -> Vec<&'a model::InventoryRow> {
    report
        .rows
        .iter()
        .filter(|row| row_matches_query(row, query))
        .collect()
}

fn row_matches_query(row: &model::InventoryRow, query: &str) -> bool {
    query.is_empty()
        || fuzzy_subsequence(&row.repository, query)
        || fuzzy_subsequence(&row.path, query)
        || fuzzy_subsequence(&row.origin, query)
        || fuzzy_subsequence(row.registration.label(), query)
        || fuzzy_subsequence(row.checkout.label(), query)
        || fuzzy_subsequence(&row.branch.label(), query)
        || fuzzy_subsequence(&row.changes.label(), query)
        || fuzzy_subsequence(registration_text(&row.registration), query)
        || fuzzy_subsequence(checkout_text(&row.checkout), query)
        || fuzzy_subsequence(&observation_text(&row.branch), query)
        || fuzzy_subsequence(&observation_text(&row.changes), query)
}

fn fuzzy_subsequence(value: &str, query: &str) -> bool {
    let mut query = query.chars().flat_map(char::to_lowercase);
    let mut wanted = query.next();
    for character in value.chars().flat_map(char::to_lowercase) {
        if Some(character) == wanted {
            wanted = query.next();
        }
        if wanted.is_none() {
            return true;
        }
    }
    wanted.is_none()
}

fn repository_table(
    rows: &[&model::InventoryRow],
    names: &HashMap<model::RowKey, String>,
    marks: &Marks,
    home: &Path,
    width: u16,
    selected: Option<usize>,
) -> Table<'static> {
    let compact = width < WIDE_TABLE;
    // Column widths from the canvas grid (term.css `g7` and `g5`), padding included.
    let (headings, mut widths, flexible): (&[&str], Vec<u16>, &[usize]) = if compact {
        (
            &[
                "repository",
                "registration",
                "checkout",
                "branch",
                "changes",
            ],
            vec![23, 15, 13, 13, 14],
            &[0],
        )
    } else {
        (
            &[
                "repository",
                "path",
                "registration",
                "checkout",
                "origin",
                "branch",
                "changes",
            ],
            vec![25, 27, 15, 13, 24, 13, 13],
            &[1, 4],
        )
    };
    let slot = |column: usize| if column == 0 { MARK_SLOT } else { 0 };
    let cells = rows
        .iter()
        .map(|row| {
            let pattern = matches!(row.key, model::RowKey::Pattern(_));
            let mut cells = vec![(names[&row.key].clone(), Style::default())];
            if !compact {
                cells.push((
                    if pattern {
                        "—".to_owned()
                    } else {
                        home_relative(Path::new(&row.path), home)
                    },
                    Style::default(),
                ));
            }
            cells.extend([
                (
                    registration_text(&row.registration).to_owned(),
                    registration_style(&row.registration),
                ),
                (
                    checkout_text(&row.checkout).to_owned(),
                    checkout_style(&row.checkout),
                ),
            ]);
            if !compact {
                cells.push((origin_text(row), Style::default()));
            }
            cells.extend([
                (
                    observation_text(&row.branch),
                    observation_style(&row.branch),
                ),
                (
                    observation_text(&row.changes),
                    observation_style(&row.changes),
                ),
            ]);
            cells
        })
        .collect::<Vec<_>>();
    // Width beyond the grid first lets truncated cells grow, then goes to the flexible columns.
    let mut spare = width
        .saturating_sub(2)
        .saturating_sub(widths.iter().sum::<u16>());
    for column in 0..widths.len() {
        let need = cells
            .iter()
            .map(|row| Line::raw(row[column].0.clone()).width() + slot(column) + 2)
            .max()
            .unwrap_or(0) as u16;
        let grow = need.saturating_sub(widths[column]).min(spare);
        widths[column] += grow;
        spare -= grow;
    }
    for (position, column) in flexible.iter().enumerate() {
        let share = spare / flexible.len() as u16;
        widths[*column] += if position == 0 {
            spare - share * (flexible.len() as u16 - 1)
        } else {
            share
        };
    }
    let available = width.saturating_sub(2);
    while widths.iter().map(|width| u32::from(*width)).sum::<u32>() > u32::from(available) {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(index, value)| **value > headings[*index].len() as u16 + 2)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }
    let table_rows = cells
        .into_iter()
        .enumerate()
        .map(|(index, cells)| {
            Row::new(
                cells
                    .into_iter()
                    .enumerate()
                    .map(|(column, (value, style))| {
                        // Path cells keep their tail: it is what tells duplicate checkouts apart.
                        let room = usize::from(widths[column]).saturating_sub(2 + slot(column));
                        let text = if column == 0 {
                            abbreviate_name(&value, room)
                        } else {
                            abbreviate(&value, room, !compact && column == 1)
                        };
                        // Reverse video carries the selection, so the selected row drops its tints.
                        let style = if selected == Some(index) {
                            Style::default()
                        } else {
                            style
                        };
                        let mark = match column {
                            0 if !rows[index].markable => "    ",
                            0 if marks.contains(&rows[index].key) => "[x] ",
                            0 => "[ ] ",
                            _ => "",
                        };
                        Cell::from(Span::styled(format!(" {mark}{text}"), style))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    Table::new(table_rows, widths)
        .header(
            Row::new(
                headings
                    .iter()
                    .map(|heading| Cell::from(format!(" {heading}")))
                    .collect::<Vec<_>>(),
            )
            .style(dim()),
        )
        .column_spacing(0)
        .highlight_symbol(" ")
        .highlight_spacing(HighlightSpacing::Always)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
}

/// The longest run of `characters` that fits `width` cells.
fn fitting(characters: impl Iterator<Item = char>, width: usize) -> String {
    let mut used = 0;
    characters
        .take_while(|character| {
            used += Span::raw(character.to_string()).width();
            used <= width
        })
        .collect()
}

fn fitting_tail(value: &str, width: usize) -> String {
    let tail = fitting(value.chars().rev(), width);
    tail.chars().rev().collect()
}

fn abbreviate(value: &str, width: usize, keep_tail: bool) -> String {
    if Line::raw(value).width() <= width {
        value.to_owned()
    } else if width == 0 {
        String::new()
    } else if keep_tail {
        format!("…{}", fitting_tail(value, width - 1))
    } else {
        format!("{}…", fitting(value.chars(), width - 1))
    }
}

/// Repository names keep both ends and the whole copy suffix: either can be the only part
/// that tells two rows apart.
fn abbreviate_name(name: &str, width: usize) -> String {
    if Line::raw(name).width() <= width {
        return name.to_owned();
    }
    let (base, suffix) = match name.rfind(" (copy") {
        Some(index) if name.ends_with(')') => name.split_at(index),
        _ => (name, ""),
    };
    let room = width.saturating_sub(Line::raw(suffix).width());
    if room == 0 {
        return abbreviate(name, width, false);
    }
    let head = fitting(base.chars(), room / 2);
    let tail = fitting_tail(base, room - 1 - Line::raw(head.as_str()).width());
    format!("{head}…{tail}{suffix}")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use super::{
        Bindings, InventorySession, Selection, StartupError, TerminalRestore, enter_after_restore,
        start_with_loading,
    };
    use crate::application::inventory::{
        CheckoutState, InventoryFilesystem, InventoryRow, LocalCheckout, LocalScan,
        ObservationField, OriginFact, RegistrationState, RowKey,
    };
    use crate::domain::config::Config;
    use crate::infrastructure::inventory::LocalInventoryFilesystem;
    use crate::infrastructure::inventory::{ScanContext, ScanEvent, ScanHandle, TargetSnapshot};

    #[derive(Clone)]
    struct RestoreProbe(Rc<RefCell<Vec<&'static str>>>);

    impl Drop for RestoreProbe {
        fn drop(&mut self) {
            self.0.borrow_mut().push("restored");
        }
    }

    struct FakeTerminal {
        restore: Option<RestoreProbe>,
    }

    impl Drop for FakeTerminal {
        fn drop(&mut self) {
            self.restore.take();
        }
    }

    #[cfg(unix)]
    #[test]
    fn receiver_rejects_equal_length_in_place_config_edit_after_probe_validation() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let checkout = root.join("checkout");
        let config = checkout.join(".git/config");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "origin = old\n").unwrap();
        let config_path = temp.path().join("inventory.toml");
        std::fs::write(&config_path, "root = \"root\"\n").unwrap();
        let target = TargetSnapshot::capture_for_test(checkout.clone());
        assert!(target.contents_match_current_for_test());
        let before = std::fs::metadata(&config).unwrap();

        // This is the ordering under test: the background content check succeeded, then the
        // existing config inode is edited in place before the old event reaches the receiver.
        std::fs::write(&config, "origin = new\n").unwrap();
        let after = std::fs::metadata(&config).unwrap();
        assert_eq!(before.dev(), after.dev());
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.len(), after.len());

        let context = ScanContext {
            generation: 1,
            config_revision: LocalInventoryFilesystem.config_revision(&config_path),
            root: root.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(ScanEvent::Checkout {
                checkout: LocalCheckout {
                    path: checkout.clone(),
                    identity: None,
                    origin: OriginFact::Absent,
                    branch: ObservationField::Known("main".to_owned()),
                    changes: ObservationField::Clean,
                },
                context,
                target,
                content_current: true,
            })
            .unwrap();
        drop(sender);
        let mut session = InventorySession {
            config: Config {
                root: "root".to_owned(),
                providers: Default::default(),
                repositories: Vec::new(),
            },
            config_path,
            home: temp.path().to_path_buf(),
            root,
            scan: LocalScan::default(),
            worker: ScanHandle::for_test(receiver),
            generation: 1,
            complete: false,
            problems: Default::default(),
            marks: Default::default(),
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            message: None,
            cached_report: None,
            batch: None,
        };

        session.receive_events();

        assert!(session.scan.checkouts.is_empty());
        assert_eq!(session.scan.stale_paths, vec![checkout]);
    }

    #[test]
    fn inspection_anchor_does_not_follow_reconciled_table_selection() {
        let row = |key: &str| InventoryRow {
            key: RowKey::Pattern(key.to_owned()),
            repository: key.to_owned(),
            path: key.to_owned(),
            registration: RegistrationState::Pattern,
            checkout: CheckoutState::Pattern,
            origin: key.to_owned(),
            remote: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: Some(key.to_owned()),
            warnings: Vec::new(),
            markable: false,
        };
        let alpha = row("github.com/org/alpha/*");
        let beta = row("github.com/org/beta/*");
        let mut selection = Selection::default();
        selection.table.select(Some(0));
        selection.open_inspection(&[&alpha]);

        // Reconciliation legitimately advances the table to beta, but an open inspection cannot.
        selection.reconcile(&[&beta], true);

        assert_eq!(selection.selected_row(&[&beta]), Some(&beta));
        assert!(selection.inspection);
        assert_eq!(selection.inspected_row(&[&beta]), None);
    }

    #[test]
    fn provisional_selection_yields_to_scan_completion_and_explicit_navigation() {
        let row = |name: &str| InventoryRow {
            key: RowKey::Pattern(name.to_owned()),
            repository: name.to_owned(),
            path: name.to_owned(),
            registration: RegistrationState::Pattern,
            checkout: CheckoutState::Pattern,
            origin: name.to_owned(),
            remote: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: Some(name.to_owned()),
            warnings: Vec::new(),
            markable: false,
        };
        let alpha = row("alpha");
        let beta = row("beta");
        let mut selection = Selection::default();
        selection.reconcile(&[&beta], false);
        selection.reconcile(&[&alpha], false);
        assert_eq!(selection.selected_row(&[&alpha]), Some(&alpha));
        assert_eq!(selection.key, Some(beta.key.clone()));
        selection.reconcile(&[&alpha, &beta], true);
        assert_eq!(selection.selected_row(&[&alpha, &beta]), Some(&beta));

        selection.reconcile(&[&alpha], true);
        selection.reconcile(&[&beta, &alpha], true);
        assert_eq!(selection.selected_row(&[&beta, &alpha]), Some(&alpha));

        selection.move_by(&[&beta, &alpha], -1);
        selection.reconcile(&[&alpha], false);
        selection.move_by(&[&alpha], 1);
        selection.reconcile(&[&beta, &alpha], true);
        assert_eq!(selection.selected_row(&[&beta, &alpha]), Some(&alpha));
    }

    fn display_row(key: RowKey, checkout: CheckoutState) -> InventoryRow {
        InventoryRow {
            key,
            repository: String::new(),
            path: String::new(),
            registration: RegistrationState::Explicit,
            checkout,
            origin: String::new(),
            remote: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: None,
            warnings: Vec::new(),
            markable: true,
        }
    }

    #[test]
    fn only_a_known_clean_observation_reads_clean() {
        assert_eq!(super::observation_text(&ObservationField::Clean), "clean");
        for field in [
            ObservationField::Pending,
            ObservationField::Error("status failed".to_owned()),
            ObservationField::Stale("main".to_owned()),
            super::stale_observation(ObservationField::Clean),
            super::stale_observation(ObservationField::Pending),
            ObservationField::Known("main".to_owned()),
            ObservationField::Dirty("1 modified".to_owned()),
            ObservationField::NotApplicable,
        ] {
            assert_ne!(super::observation_text(&field), "clean", "{field:?}");
        }
        assert_eq!(
            super::observation_text(&super::stale_observation(ObservationField::Clean)),
            "stale: known clean"
        );
        assert_eq!(super::checkout_text(&CheckoutState::Unknown), "unknown");
    }

    #[test]
    fn search_matches_the_labels_the_table_shows() {
        let mut pattern = display_row(
            RowKey::Pattern("github.com/org/*".to_owned()),
            CheckoutState::Pattern,
        );
        pattern.registration = RegistrationState::Pattern;
        assert!(super::row_matches_query(&pattern, "wildcard"));
        assert!(super::row_matches_query(&pattern, "pattern"));
        let mut clean = display_row(RowKey::Path("/repos/clean".into()), CheckoutState::Cloned);
        clean.changes = ObservationField::Clean;
        assert!(super::row_matches_query(&clean, "known clean"));
    }

    #[test]
    fn short_names_number_copies_and_qualify_shared_last_segments() {
        let checkout = |identity: &str, path: &str| {
            display_row(
                RowKey::Checkout {
                    identity: identity.to_owned(),
                    path: path.into(),
                },
                CheckoutState::Cloned,
            )
        };
        let declaration = |identity: &str, path: &str, state| {
            display_row(
                RowKey::Declaration {
                    identity: identity.to_owned(),
                    destination: path.into(),
                },
                state,
            )
        };
        let rows = vec![
            declaration(
                "github.com/org/alpha",
                "/r/org/alpha",
                CheckoutState::Cloned,
            ),
            checkout("github.com/org/alpha", "/r/a/alpha"),
            checkout("github.com/org/alpha", "/r/b/alpha"),
            declaration(
                "github.com/org/wrong",
                "/r/org/wrong",
                CheckoutState::Conflict,
            ),
            checkout("github.com/other/wrong", "/r/org/wrong"),
            declaration("github.com/org/lone", "/r/org/lone", CheckoutState::Missing),
            checkout("github.com/org/lone", "/r/elsewhere/lone"),
            display_row(RowKey::Path("/r/local/beta".into()), CheckoutState::Cloned),
            declaration("github.com/org/beta", "/r/org/beta", CheckoutState::Missing),
            checkout("github.com/org/x", "/r/hub/x"),
            checkout("gitlab.com/org/x", "/r/lab/x"),
            checkout("gitlab.com/org/x", "/r/lab-backup/x"),
            checkout("gitlab.com/a/sub/y", "/r/a/y"),
            checkout("gitlab.com/b/sub/y", "/r/b/y"),
            display_row(
                RowKey::Pattern("github.com/org/*".to_owned()),
                CheckoutState::Pattern,
            ),
            display_row(
                RowKey::Pattern("codeberg.org/org/*".to_owned()),
                CheckoutState::Pattern,
            ),
            display_row(
                RowKey::Path("/r/one/work/proj".into()),
                CheckoutState::Cloned,
            ),
            display_row(
                RowKey::Path("/r/two/work/proj".into()),
                CheckoutState::Cloned,
            ),
            display_row(
                RowKey::Path("/r/org/pending".into()),
                CheckoutState::Unknown,
            ),
            declaration(
                "github.com/org/pending",
                "/r/org/pending",
                CheckoutState::Missing,
            ),
        ];
        let names = super::short_names(&rows);
        let name = |row: &InventoryRow| names[&row.key].as_str();
        assert_eq!(
            rows.iter().map(name).collect::<Vec<_>>(),
            [
                "alpha",
                "alpha (copy)",
                "alpha (copy 2)",
                "org/wrong",
                "other/wrong",
                "lone",
                "lone",
                "local/beta",
                "org/beta",
                "github.com/org/x",
                "gitlab.com/org/x",
                "gitlab.com/org/x (copy)",
                "a/sub/y",
                "b/sub/y",
                "github.com/org/*",
                "codeberg.org/org/*",
                "one/work/proj",
                "two/work/proj",
                "org/pending",
                "org/pending",
            ]
        );
    }

    #[test]
    fn name_cells_stay_distinct_when_truncated() {
        let names = [
            "obsidian-theme",
            "obsidian-theme (copy)",
            "obsidian-theme (copy 2)",
            "black-atom-core-extras",
            "black-atom-core-extras (copy)",
            "black-atom-core-extras (copy 12)",
            "github.com/org/shared-tooling",
            "gitlab.com/org/shared-tooling",
            "gitlab.com/org/shared-tooling (copy)",
            "one/work/a-long-project-name",
            "two/work/a-long-project-name",
            "very-long-prefix-service-a",
            "very-long-prefix-service-b",
        ];
        for width in [6, 17, 19] {
            let cells = names
                .iter()
                .map(|name| super::abbreviate_name(name, width))
                .collect::<Vec<_>>();
            for (index, cell) in cells.iter().enumerate() {
                assert!(cell.chars().count() <= width, "{cell}");
                if width > 6 {
                    assert!(!cells[..index].contains(cell), "{width}: {cells:?}");
                    if let Some(at) = names[index].rfind(" (copy") {
                        assert!(cell.ends_with(&names[index][at..]), "{cell}");
                    }
                }
            }
        }
        assert_eq!(
            super::abbreviate_name("github.com/org/shared-tooling", 17),
            "github.c…-tooling"
        );
        assert_eq!(
            super::abbreviate_name("alpha (copy 2)", 17),
            "alpha (copy 2)"
        );
    }

    #[test]
    fn constructor_failure_restores_after_acquisition_without_starting_scan() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let scan_started = Rc::new(RefCell::new(false));
        let acquired_events = Rc::clone(&events);
        let restore_events = Rc::clone(&events);
        let scan_started_for_start = Rc::clone(&scan_started);

        let result = start_with_loading(
            move || {
                enter_after_restore(
                    move || {
                        acquired_events.borrow_mut().push("acquired");
                        Ok(RestoreProbe(restore_events))
                    },
                    || Err::<FakeTerminal, _>("constructor failed".to_owned()),
                )
                .map(|(restore, _terminal)| FakeTerminal {
                    restore: Some(restore),
                })
            },
            |_terminal| Ok(()),
            move || *scan_started_for_start.borrow_mut() = true,
        );

        assert!(matches!(result, Err(StartupError::Enter(error)) if error == "constructor failed"));
        assert_eq!(&*events.borrow(), &["acquired", "restored"]);
        assert!(!*scan_started.borrow());
    }

    #[test]
    fn partial_restore_flags_only_release_acquired_terminal_state() {
        let mut raw_only = TerminalRestore {
            raw: true,
            alternate: false,
        };
        raw_only.restore();
        assert!(!raw_only.raw);
        assert!(!raw_only.alternate);

        let mut alternate_only = TerminalRestore {
            raw: false,
            alternate: true,
        };
        alternate_only.restore();
        assert!(!alternate_only.raw);
        assert!(!alternate_only.alternate);
    }

    #[test]
    fn loading_draw_failure_restores_after_construction_without_starting_scan() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let scan_started = Rc::new(RefCell::new(false));
        let scan_started_for_start = Rc::clone(&scan_started);
        let restore = RestoreProbe(Rc::clone(&events));

        let result = start_with_loading(
            move || {
                Ok(FakeTerminal {
                    restore: Some(restore),
                })
            },
            |_terminal| Err("loading draw failed".to_owned()),
            move || *scan_started_for_start.borrow_mut() = true,
        );

        assert!(matches!(result, Err(StartupError::Draw(error)) if error == "loading draw failed"));
        assert_eq!(&*events.borrow(), &["restored"]);
        assert!(!*scan_started.borrow());
    }
}
