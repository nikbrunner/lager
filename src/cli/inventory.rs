use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap};

use crate::application::inventory::{
    self as model, InventoryFilesystem, InventoryReport, LocalScan,
};
use crate::infrastructure::config::load_inventory;
use crate::infrastructure::inventory::{
    LocalInventoryFilesystem, ScanContext, ScanEvent, ScanHandle, start_local_scan_with_context,
};
use crate::presentation::escape;

use super::args::InventoryArgs;

mod keys;
use keys::{Action, Bindings, Mode};

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
    bindings: Bindings,
    notice: Option<String>,
    cached_report: Option<InventoryReport>,
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
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            cached_report: None,
        }
    }

    /// Inventory rows are a snapshot of scan events, not a reason to probe the filesystem again
    /// for every terminal frame. Refreshing and receiving an event invalidate this cache.
    fn report(&mut self) -> Result<InventoryReport, String> {
        if self.cached_report.is_none() {
            let mut report = model::build_inventory(
                &LocalInventoryFilesystem,
                &self.config,
                &self.config_path,
                &self.home,
                self.scan.clone(),
            )?;
            report.observation_generation = self.generation;
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

    fn draw_loading(&mut self) -> Result<(), String> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Paragraph::new("Scanning repository roots…\nGit status appears as observations complete. Remote discovery is unavailable.")
                .block(Block::default().title("LAGER / inventory").borders(Borders::ALL)).wrap(Wrap { trim: false }), area);
        }).map(|_| ()).map_err(|error| error.to_string())
    }

    fn run(&mut self, session: &mut InventorySession) -> i32 {
        let mut selection = Selection::default();
        loop {
            session.receive_events();
            let report = match session.report() {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("lager: {}", escape(error));
                    return 1;
                }
            };
            let visible_rows = matching_rows(&report, &selection.query);
            selection.reconcile(&visible_rows);
            if let Err(error) = self.draw_report(&report, &visible_rows, session, &mut selection) {
                eprintln!("lager: terminal render failed: {}", escape(error));
                return 1;
            }
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        let mode = if selection.help || selection.inspection {
                            Mode::Inspection
                        } else if selection.menu {
                            Mode::Menu
                        } else if selection.search {
                            Mode::Search
                        } else {
                            Mode::Normal
                        };
                        match session.bindings.action(mode, key) {
                            Some(Action::Quit) => {
                                session.worker.cancel_and_join();
                                session.receive_events();
                                return i32::from(session.had_failure);
                            }
                            Some(Action::Refresh) => {
                                if let Err(error) = session.refresh() {
                                    session.had_failure = true;
                                    session.notice =
                                        Some(format!("refresh failed: {}", escape(error)));
                                }
                            }
                            Some(Action::Menu) if !selection.search => {
                                selection.menu = true;
                                selection.menu_index = 0;
                                selection.menu_scroll = 0;
                            }
                            Some(Action::Search) => {
                                selection.menu = false;
                                selection.search = true;
                            }
                            Some(Action::ClearSearch) => selection.query.clear(),
                            Some(Action::Help) => {
                                selection.help = !selection.help;
                                selection.help_scroll = 0;
                            }
                            Some(Action::Cancel | Action::Accept) if selection.help => {
                                selection.help = false;
                            }
                            Some(Action::Cancel | Action::Accept) if selection.inspection => {
                                selection.close_inspection();
                            }
                            Some(Action::Cancel) if selection.menu => {
                                selection.menu = false;
                                selection.menu_scroll = 0;
                            }
                            Some(Action::Accept) if selection.menu => {
                                let action = root_menu_actions()
                                    .get(selection.menu_index)
                                    .copied()
                                    .expect("menu selection is clamped");
                                selection.menu = false;
                                selection.menu_scroll = 0;
                                match action {
                                    Action::Inspect => selection.open_inspection(&visible_rows),
                                    Action::Search => selection.search = true,
                                    Action::Help => {
                                        selection.help = true;
                                        selection.help_scroll = 0;
                                    }
                                    Action::Quit => {
                                        session.worker.cancel_and_join();
                                        session.receive_events();
                                        return i32::from(session.had_failure);
                                    }
                                    _ => unreachable!("root menu contains only overview actions"),
                                }
                            }
                            Some(Action::Cancel | Action::Accept) if selection.search => {
                                selection.search = false;
                            }
                            Some(Action::Inspect) if !selection.search => {
                                selection.menu = false;
                                selection.menu_scroll = 0;
                                selection.open_inspection(&visible_rows);
                            }
                            Some(Action::Cancel | Action::Accept) => {}
                            Some(Action::Up) if selection.help => {
                                selection.help_scroll = selection.help_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if selection.help => {
                                selection.help_scroll = selection
                                    .help_scroll
                                    .saturating_add(1)
                                    .min(self.help_scroll_limit(session));
                            }
                            Some(Action::PageUp) if selection.help => {
                                selection.help_scroll = selection.help_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if selection.help => {
                                selection.help_scroll = selection
                                    .help_scroll
                                    .saturating_add(10)
                                    .min(self.help_scroll_limit(session));
                            }
                            Some(Action::Up) if selection.inspection => {
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if selection.inspection => {
                                let limit = selection
                                    .inspected_row(&visible_rows)
                                    .map(|row| self.inspection_scroll_limit(row))
                                    .unwrap_or(0);
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_add(1).min(limit);
                            }
                            Some(Action::PageUp) if selection.inspection => {
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if selection.inspection => {
                                let limit = selection
                                    .inspected_row(&visible_rows)
                                    .map(|row| self.inspection_scroll_limit(row))
                                    .unwrap_or(0);
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_add(10).min(limit);
                            }
                            Some(Action::Up) if selection.menu => selection.move_menu_by(-1),
                            Some(Action::Down) if selection.menu => selection.move_menu_by(1),
                            Some(Action::PageUp) if selection.menu => selection.move_menu_by(-10),
                            Some(Action::PageDown) if selection.menu => selection.move_menu_by(10),
                            Some(Action::Up) => selection.move_by(&visible_rows, -1),
                            Some(Action::Down) => selection.move_by(&visible_rows, 1),
                            None if selection.search => selection.edit_query(key),
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

    fn help_scroll_limit(&self, session: &InventorySession) -> u16 {
        let lines = session.bindings.help(Mode::Normal).len() as u16;
        let viewport = self
            .terminal
            .size()
            .map(|size| size.height.saturating_sub(5))
            .unwrap_or(1);
        lines.saturating_sub(viewport.max(1))
    }

    fn inspection_scroll_limit(&self, row: &model::InventoryRow) -> u16 {
        let area = self
            .terminal
            .size()
            .map(|size| Rect::new(0, 0, size.width, size.height))
            .unwrap_or_default()
            .inner(Margin {
                horizontal: 1,
                vertical: 1,
            });
        inspection_scroll_limit(row, inspection_popup_area(area))
    }

    fn menu_scroll_metrics(&self) -> (u16, u16) {
        let area = self
            .terminal
            .size()
            .map(|size| Rect::new(0, 0, size.width, size.height))
            .unwrap_or_default()
            .inner(Margin {
                horizontal: 1,
                vertical: 1,
            });
        menu_scroll_metrics(inspection_popup_area(area))
    }

    fn draw_report(
        &mut self,
        report: &InventoryReport,
        visible_rows: &[&model::InventoryRow],
        session: &InventorySession,
        selection: &mut Selection,
    ) -> Result<(), String> {
        let complete = session.complete;
        selection.help_scroll = selection.help_scroll.min(self.help_scroll_limit(session));
        if let Some(row) = selection.inspected_row(visible_rows) {
            selection.inspection_scroll = selection
                .inspection_scroll
                .min(self.inspection_scroll_limit(row));
        }
        let (menu_limit, menu_viewport) = self.menu_scroll_metrics();
        selection.clamp_menu_scroll(menu_limit, menu_viewport);
        let (mut header, mut summary) = report_lines(
            report,
            complete,
            visible_rows.len(),
            selection.search,
            &selection.query,
        );
        header.truncate(1);
        summary.pop();
        if let Some(notice) = &session.notice {
            summary.push(Line::raw(notice.clone()));
        }
        let mode = if selection.search {
            Mode::Search
        } else {
            Mode::Normal
        };
        summary.push(Line::raw("remote unavailable"));
        summary.push(Line::raw(session.bindings.help(mode).join(" · ")));
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
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(summary.len() as u16),
                    ])
                    .split(inner);
                frame.render_widget(
                    Paragraph::new(header).style(Style::default().fg(Color::Reset)),
                    chunks[0],
                );
                if visible_rows.is_empty() {
                    frame.render_widget(
                        Paragraph::new(if !report.rows.is_empty() && !selection.query.is_empty() {
                            "No matching repositories."
                        } else if !complete {
                            "Discovering local repositories…"
                        } else if report.incomplete {
                            "No repositories found so far; local scan is incomplete."
                        } else {
                            "No repositories found."
                        }),
                        chunks[1],
                    );
                } else {
                    frame.render_stateful_widget(
                        repository_table(visible_rows, chunks[1].width),
                        chunks[1],
                        &mut selection.table,
                    );
                }
                frame.render_widget(
                    Paragraph::new(summary).style(Style::default().fg(Color::Reset)),
                    chunks[2],
                );
                if selection.help {
                    let help = session.bindings.help(Mode::Normal).join("\n");
                    frame.render_widget(ratatui::widgets::Clear, inner);
                    let help_chunks = Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(2),
                    ])
                    .split(inner);
                    frame.render_widget(Paragraph::new("HELP / inventory"), help_chunks[0]);
                    frame.render_widget(
                        Paragraph::new(help).scroll((selection.help_scroll, 0)),
                        help_chunks[1],
                    );
                    frame.render_widget(
                        Paragraph::new(session.bindings.help(Mode::Inspection).join(" · "))
                            .wrap(Wrap { trim: false }),
                        help_chunks[2],
                    );
                } else if selection.inspection {
                    if let Some(row) = selection.inspected_row(visible_rows) {
                        draw_inspection(frame, inner, row, selection.inspection_scroll, session);
                    } else {
                        draw_unavailable_inspection(frame, inner, session);
                    }
                } else if selection.menu {
                    draw_root_menu(frame, inner, selection, session);
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn inspection_popup_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(1, 100);
    let height = area.height.saturating_sub(4).clamp(1, 20);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn inspection_lines(row: &model::InventoryRow) -> Vec<Line<'static>> {
    let mut lines = [
        ("Repository", escape(&row.repository)),
        ("Path", escape(&row.path)),
        ("Registration", escape(row.registration.label())),
        ("Checkout", escape(row.checkout.label())),
        ("Origin", escape(&row.origin)),
        ("Branch", escape(row.branch.label())),
        ("Changes", escape(row.changes.label())),
    ]
    .into_iter()
    .map(|(name, value)| Line::raw(format!("{name}: {value}")))
    .collect::<Vec<_>>();
    if let Some(declaration) = &row.declaration {
        lines.push(Line::raw(format!("Declaration: {}", escape(declaration))));
    }
    if let Some(path) = &row.configured_destination {
        lines.push(Line::raw(format!(
            "Configured path: {}",
            escape(path.to_string_lossy())
        )));
    }
    if let Some(path) = &row.observed_path {
        lines.push(Line::raw(format!(
            "Found path: {}",
            escape(path.to_string_lossy())
        )));
    }
    lines.push(Line::raw(if row.markable {
        "Mark: available".to_owned()
    } else {
        "Mark: unavailable (declaration patterns cannot be marked)".to_owned()
    }));
    if !row.warnings.is_empty() {
        lines.push(Line::raw("Warnings:"));
        lines.extend(
            row.warnings
                .iter()
                .map(|warning| Line::raw(format!("- {}", escape(warning)))),
        );
    }
    lines
}

fn inspection_scroll_limit(row: &model::InventoryRow, area: Rect) -> u16 {
    let content = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let required = Paragraph::new(inspection_lines(row))
        .wrap(Wrap { trim: false })
        .line_count(content.width);
    let viewport = usize::from(content.height.saturating_sub(1).max(1));
    required.saturating_sub(viewport).min(usize::from(u16::MAX)) as u16
}

fn root_menu_actions() -> &'static [Action] {
    &[Action::Inspect, Action::Search, Action::Help, Action::Quit]
}

fn menu_scroll_metrics(popup: Rect) -> (u16, u16) {
    let content = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let viewport = content.height.saturating_sub(1).max(1);
    (
        (root_menu_actions().len() as u16).saturating_sub(viewport),
        viewport,
    )
}

fn draw_root_menu(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    selection: &Selection,
    session: &InventorySession,
) {
    let popup = inspection_popup_area(area);
    let block = Block::default()
        .title("MENU / actions")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let content = block.inner(popup);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(content);
    let items = root_menu_actions()
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let style = if index == selection.menu_index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let hint = session.bindings.hint(Mode::Menu, *action);
            Line::from(Span::styled(
                if hint.is_empty() {
                    action.name().to_owned()
                } else {
                    format!("{hint} {}", action.name())
                },
                style,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(items).scroll((selection.menu_scroll, 0)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(session.bindings.help(Mode::Menu).join(" · "))
            .style(Style::default().fg(Color::Reset)),
        chunks[1],
    );
}

fn draw_unavailable_inspection(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    session: &InventorySession,
) {
    let popup = inspection_popup_area(area);
    let block = Block::default()
        .title("INSPECT / unavailable")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let content = block.inner(popup);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(content);
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(
            "Inspected target is unavailable or stale. Close this popup and inspect a current row.",
        ),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(session.bindings.help(Mode::Inspection).join(" · "))
            .style(Style::default().fg(Color::Reset)),
        chunks[1],
    );
}

fn draw_inspection(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    row: &model::InventoryRow,
    scroll: u16,
    session: &InventorySession,
) {
    let popup = inspection_popup_area(area);
    let block = Block::default()
        .title(format!("INSPECT / {}", escape(&row.repository)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let content = block.inner(popup);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(content);
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(inspection_lines(row))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(session.bindings.help(Mode::Inspection).join(" · "))
            .style(Style::default().fg(Color::Reset)),
        chunks[1],
    );
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
    visible_count: usize,
    search: bool,
    query: &str,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let header = vec![Line::from(vec![
        Span::styled(
            if search { "SEARCH" } else { "NORMAL" },
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " / {} · {} rows · generation {} · {}",
            escape(query),
            visible_count,
            report.observation_generation,
            escape(&report.config_revision)
        )),
    ])];
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
    (header, summary)
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
    menu: bool,
    menu_index: usize,
    menu_scroll: u16,
    help: bool,
    help_scroll: u16,
    inspection: bool,
    inspection_key: Option<model::RowKey>,
    inspection_scroll: u16,
}

impl Selection {
    fn reconcile(&mut self, rows: &[&model::InventoryRow]) {
        let index = self
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
            })
            .or_else(|| {
                (!rows.is_empty()).then_some(
                    self.table
                        .selected()
                        .unwrap_or(0)
                        .min(rows.len().saturating_sub(1)),
                )
            });
        self.table.select(index);
        // An empty filtered or in-flight refresh frame must not discard the target needed when
        // its checkout or declaration row returns.
        if let Some(index) = index {
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

    fn move_menu_by(&mut self, delta: isize) {
        let last = root_menu_actions().len().saturating_sub(1);
        self.menu_index = self.menu_index.saturating_add_signed(delta).min(last);
    }

    fn clamp_menu_scroll(&mut self, limit: u16, viewport: u16) {
        self.menu_scroll = self.menu_scroll.min(limit);
        let selected = self.menu_index.min(u16::MAX as usize) as u16;
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

    fn edit_query(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                self.query.pop();
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.query.push(character);
            }
            _ => {}
        }
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

fn repository_table(rows: &[&model::InventoryRow], width: u16) -> Table<'static> {
    let compact = width < 120;
    let headings = if compact {
        vec![
            "repository",
            "registration",
            "checkout",
            "branch",
            "changes",
        ]
    } else {
        vec![
            "repository",
            "path",
            "registration",
            "checkout",
            "origin",
            "branch",
            "changes",
        ]
    };
    let values = rows
        .iter()
        .map(|row| {
            let mut values = vec![escape(&row.repository)];
            if !compact {
                values.push(escape(&row.path));
            }
            values.extend([
                row.registration.label().to_owned(),
                row.checkout.label().to_owned(),
            ]);
            if !compact {
                values.push(escape(&row.origin));
            }
            values.extend([escape(row.branch.label()), escape(row.changes.label())]);
            values
        })
        .collect::<Vec<_>>();
    let mut widths = headings
        .iter()
        .enumerate()
        .map(|(column, heading)| {
            values
                .iter()
                .map(|row| Line::raw(row[column].clone()).width())
                .max()
                .unwrap_or(0)
                .max(heading.len()) as u16
        })
        .collect::<Vec<_>>();
    let available = width.saturating_sub(2 + 3 * (headings.len() as u16 - 1));
    while widths.iter().map(|width| u32::from(*width)).sum::<u32>() > u32::from(available) {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(index, value)| **value > headings[*index].len() as u16)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }
    let columns = |values: Vec<String>| {
        values
            .into_iter()
            .zip(&widths)
            .map(|(value, width)| {
                let padding = usize::from(*width).saturating_sub(Line::raw(value.clone()).width());
                format!("{value}{}", " ".repeat(padding))
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let rows = values
        .into_iter()
        .zip(rows.iter().copied())
        .map(|(values, row)| {
            let color = if std::env::var_os("NO_COLOR").is_some() {
                Color::Reset
            } else {
                match row.registration {
                    model::RegistrationState::Explicit => Color::Green,
                    model::RegistrationState::Wildcard { .. }
                    | model::RegistrationState::Pattern => Color::Blue,
                    model::RegistrationState::Excluded { .. } => Color::Yellow,
                    model::RegistrationState::Unregistered => Color::Reset,
                }
            };
            let values = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let tail = (!compact && index == 1)
                        || (index == 0
                            && row.observed_path.is_some()
                            && row.repository == row.path);
                    abbreviate(&value, usize::from(widths[index]), tail)
                })
                .collect();
            let mut lines = vec![Line::raw(columns(values))];
            if !compact {
                if let Some(destination) = &row.configured_destination
                    && row
                        .observed_path
                        .as_ref()
                        .is_some_and(|path| path != destination)
                {
                    lines.push(Line::raw(format!(
                        "  configured: {}",
                        escape(destination.to_string_lossy())
                    )));
                }
                lines.extend(
                    row.warnings
                        .iter()
                        .map(|warning| Line::raw(format!("  warning: {}", escape(warning)))),
                );
            }
            for (name, fact) in [("branch", &row.branch), ("changes", &row.changes)] {
                if matches!(
                    fact,
                    model::ObservationField::Error(_) | model::ObservationField::Stale(_)
                ) {
                    lines.push(Line::raw(format!("  {name}: {}", escape(fact.label()))));
                }
            }
            let height = lines.len() as u16;
            Row::new([ratatui::text::Text::from(lines)])
                .height(height)
                .style(Style::default().fg(color))
        })
        .collect::<Vec<_>>();
    Table::new(rows, [Constraint::Min(0)])
        .header(Row::new([columns(
            headings.into_iter().map(str::to_owned).collect(),
        )]))
        .column_spacing(0)
        .highlight_symbol("  ")
        .highlight_spacing(HighlightSpacing::Always)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
}

fn abbreviate(value: &str, width: usize, tail: bool) -> String {
    if Line::raw(value).width() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let characters = value.chars().collect::<Vec<_>>();
    let iter: Box<dyn Iterator<Item = &char>> = if tail {
        Box::new(characters.iter().rev())
    } else {
        Box::new(characters.iter())
    };
    let mut result = Vec::new();
    let mut used = 1;
    for character in iter {
        used += Span::raw(character.to_string()).width();
        if used > width {
            break;
        }
        result.push(*character);
    }
    if tail {
        result.reverse();
        result.insert(0, '…');
    } else {
        result.push('…');
    }
    result.into_iter().collect()
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
            had_failure: false,
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            cached_report: None,
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
        selection.reconcile(&[&beta]);

        assert_eq!(selection.selected_row(&[&beta]), Some(&beta));
        assert!(selection.inspection);
        assert_eq!(selection.inspected_row(&[&beta]), None);
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
