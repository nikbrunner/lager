use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::application::inventory::{
    self as model, InventoryFilesystem, InventoryReport, LocalScan,
};
use crate::application::ports::ConfigStore;
use crate::infrastructure::config::CommandConfigStore;
use crate::infrastructure::inventory::{
    LocalInventoryFilesystem, ScanContext, ScanEvent, ScanHandle, start_local_scan_with_context,
};
use crate::presentation::escape;

use super::args::InventoryArgs;

pub fn run(config_path: &Path, args: InventoryArgs) -> i32 {
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
    let store = CommandConfigStore::default();
    let config = match store.load(config_path) {
        Ok(config) => config,
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
        || InventorySession::new(config, config_path.to_path_buf(), home, root),
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
    terminal.run(&mut session)
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
    had_failure: bool,
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
            had_failure: false,
        }
    }

    fn report(&self) -> Result<InventoryReport, String> {
        let mut report = model::build_inventory(
            &LocalInventoryFilesystem,
            &self.config,
            &self.config_path,
            &self.home,
            self.scan.clone(),
        )?;
        report.observation_generation = self.generation;
        Ok(report)
    }

    fn event_is_current(&self, context: &ScanContext) -> bool {
        context.generation == self.generation
            && context.root == self.root
            && context.config_revision
                == LocalInventoryFilesystem.config_revision(&self.config_path)
    }

    fn receive_events(&mut self) {
        while let Ok(event) = self.worker.receiver.try_recv() {
            if matches!(&event, ScanEvent::Failure)
                || matches!(&event, ScanEvent::Diagnostic { diagnostic, .. } if diagnostic.incomplete)
            {
                self.had_failure = true;
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
    }

    fn reject_changed_target(&mut self, path: PathBuf) {
        self.scan.pending_paths.retain(|pending| pending != &path);
        self.scan.checkouts.retain(|checkout| checkout.path != path);
        if !self.scan.stale_paths.contains(&path) {
            self.scan.stale_paths.push(path);
        }
    }

    fn refresh(&mut self) -> Result<(), String> {
        let store = CommandConfigStore::default();
        let config = store
            .load(&self.config_path)
            .map_err(|error| error.to_string())?;
        let root = config
            .resolve_root(&self.home)
            .map_err(|error| error.to_string())?;
        // Do not install a new config while the old worker can still emit against it.
        self.worker.cancel_and_join();
        self.receive_events();
        self.generation += 1;
        self.config = config;
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

    fn draw_loading(&mut self) -> Result<(), String> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Paragraph::new("Scanning repository roots…\nGit status appears as observations complete. Remote discovery is unavailable.\nq quit")
                .block(Block::default().title("LAGER / inventory").borders(Borders::ALL)).wrap(Wrap { trim: false }), area);
        }).map(|_| ()).map_err(|error| error.to_string())
    }

    fn run(&mut self, session: &mut InventorySession) -> i32 {
        loop {
            session.receive_events();
            let report = match session.report() {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("lager: {}", escape(error));
                    return 1;
                }
            };
            if let Err(error) = self.draw_report(&report, session.complete) {
                eprintln!("lager: terminal render failed: {}", escape(error));
                return 1;
            }
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key))
                        if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) =>
                    {
                        session.worker.cancel_and_join();
                        session.receive_events();
                        return i32::from(session.had_failure);
                    }
                    Ok(Event::Key(key))
                        if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('R') =>
                    {
                        if let Err(error) = session.refresh() {
                            eprintln!("lager: refresh failed: {}", escape(error));
                            return 1;
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

    fn draw_report(&mut self, report: &InventoryReport, complete: bool) -> Result<(), String> {
        let (header, rows, summary) = report_lines(report, complete);
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(
                    Block::default()
                        .title("LAGER / inventory")
                        .borders(Borders::ALL),
                    area,
                );
                let inner = area.inner(Margin {
                    horizontal: 1,
                    vertical: 1,
                });
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(1),
                        Constraint::Length(summary.len() as u16),
                    ])
                    .split(inner);
                frame.render_widget(
                    Paragraph::new(header).style(Style::default().fg(Color::Reset)),
                    chunks[0],
                );
                frame.render_widget(
                    Paragraph::new(rows)
                        .wrap(Wrap { trim: false })
                        .style(Style::default().fg(Color::Reset)),
                    chunks[1],
                );
                frame.render_widget(
                    Paragraph::new(summary).style(Style::default().fg(Color::Reset)),
                    chunks[2],
                );
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        self.restore.restore();
    }
}

fn report_lines(
    report: &InventoryReport,
    complete: bool,
) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<Line<'static>>) {
    let header = vec![
        Line::from(vec![
            Span::styled("NORMAL", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                " · {} rows · generation {} · {}",
                report.rows.len(),
                report.observation_generation,
                escape(&report.config_revision)
            )),
        ]),
        Line::raw("repository | path | registration | checkout | origin | branch | changes"),
    ];
    let mut rows = Vec::new();
    if report.rows.is_empty() {
        rows.push(Line::raw(if !complete {
            "Discovering local repositories…"
        } else if report.incomplete {
            "No repositories found so far; local scan is incomplete."
        } else {
            "No repositories found."
        }));
    }
    for row in &report.rows {
        rows.push(Line::raw(format!(
            "{} | {} | {} | {} | {} | {} | {}",
            escape(&row.repository),
            escape(&row.path),
            row.registration.label(),
            row.checkout.label(),
            escape(&row.origin),
            escape(row.branch.label()),
            escape(row.changes.label())
        )));
        if let Some(destination) = &row.configured_destination
            && row
                .observed_path
                .as_ref()
                .is_some_and(|observed| observed != destination)
        {
            rows.push(Line::raw(format!(
                "  configured: {}",
                escape(destination.to_string_lossy())
            )));
        }
        for warning in &row.warnings {
            rows.push(Line::raw(format!("  warning: {}", escape(warning))));
        }
    }
    let mut summary = Vec::new();
    if report.incomplete {
        summary.push(Line::raw(format!(
            "partial scan: {} diagnostics; one or more paths could not be read",
            report.diagnostics.len()
        )));
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
        summary.push(Line::raw(format!(
            "ignored repositories: {}",
            excluded.join(", ")
        )));
    } else if let Some(diagnostic) = report.diagnostics.first() {
        summary.push(Line::raw(format!(
            "diagnostic: {}: {}",
            escape(diagnostic.path.to_string_lossy()),
            escape(&diagnostic.message)
        )));
    }
    if !complete {
        summary.push(Line::raw("loading local discovery and observations…"));
    } else if !report.incomplete {
        summary.push(Line::raw("local scan complete"));
    }
    summary.push(Line::raw(
        "q quit · Esc quit · R refresh local/config · remote unavailable",
    ));
    (header, rows, summary)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use super::{
        InventorySession, StartupError, TerminalRestore, enter_after_restore, start_with_loading,
    };
    use crate::application::inventory::{
        InventoryFilesystem, LocalCheckout, LocalScan, ObservationField, OriginFact,
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
            had_failure: false,
        };

        session.receive_events();

        assert!(session.scan.checkouts.is_empty());
        assert_eq!(session.scan.stale_paths, vec![checkout]);
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
