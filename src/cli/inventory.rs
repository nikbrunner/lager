use std::collections::{HashMap, HashSet};
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
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
};

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
        loop {
            if let Some(code) = interrupted() {
                session.worker.cancel_and_join();
                return code;
            }
            session.receive_events();
            let report = match session.report() {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("lager: {}", escape(error));
                    return 1;
                }
            };
            let visible_rows = matching_rows(&report, &selection.query);
            selection.reconcile(&visible_rows, session.complete);
            if let Err(error) = self.draw_report(&report, &visible_rows, session, &mut selection) {
                eprintln!("lager: terminal render failed: {}", escape(error));
                return 1;
            }
            let ready = event::poll(Duration::from_millis(50));
            if let Some(code) = interrupted() {
                session.worker.cancel_and_join();
                return code;
            }
            match ready {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL
                        {
                            session.worker.cancel_and_join();
                            return 130;
                        }
                        let mode = if selection.help || selection.inspection {
                            Mode::Inspection
                        } else if selection.menu {
                            Mode::Menu
                        } else if selection.search {
                            Mode::Search
                        } else {
                            Mode::Normal
                        };
                        let action = session.bindings.action(mode, key);
                        let action = if mode == Mode::Menu && action == Some(Action::Accept) {
                            root_menu_actions().get(selection.menu_index).copied()
                        } else {
                            action
                        };
                        match action {
                            Some(Action::Quit) => {
                                session.worker.cancel_and_join();
                                session.receive_events();
                                return i32::from(session.had_failure);
                            }
                            Some(Action::Refresh) => {
                                selection.menu = false;
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
                            Some(Action::ClearSearch) => {
                                selection.menu = false;
                                selection.query.clear();
                            }
                            Some(Action::Help) => {
                                if !selection.help {
                                    selection.help_mode = mode;
                                }
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
                                    .min(self.help_scroll_limit(session, selection.help_mode));
                            }
                            Some(Action::PageUp) if selection.help => {
                                selection.help_scroll = selection.help_scroll.saturating_sub(10);
                            }
                            Some(Action::PageDown) if selection.help => {
                                selection.help_scroll = selection
                                    .help_scroll
                                    .saturating_add(10)
                                    .min(self.help_scroll_limit(session, selection.help_mode));
                            }
                            Some(Action::Up) if selection.inspection => {
                                selection.inspection_scroll =
                                    selection.inspection_scroll.saturating_sub(1);
                            }
                            Some(Action::Down) if selection.inspection => {
                                let limit = selection
                                    .inspected_row(&visible_rows)
                                    .map(|row| {
                                        self.inspection_scroll_limit(
                                            row,
                                            &report.rows,
                                            &session.home,
                                        )
                                    })
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
                                    .map(|row| {
                                        self.inspection_scroll_limit(
                                            row,
                                            &report.rows,
                                            &session.home,
                                        )
                                    })
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
                popup_body_width(area, WIDE_POPUP),
            ),
        )
    }

    fn menu_scroll_metrics(&self, session: &InventorySession, selection: &Selection) -> (u16, u16) {
        let [_, _, body, _] = popup_regions(self.area(), POPUP, &menu_widget(selection, session));
        let viewport = body.height.max(1);
        (
            (root_menu_actions().len() as u16).saturating_sub(viewport),
            viewport,
        )
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
        if let Some(row) = selection.inspected_row(visible_rows) {
            selection.inspection_scroll = selection
                .inspection_scroll
                .min(self.inspection_scroll_limit(row, &report.rows, &session.home));
        }
        let (menu_limit, menu_viewport) = self.menu_scroll_metrics(session, selection);
        selection.clamp_menu_scroll(menu_limit, menu_viewport);
        let bindings = &session.bindings;
        let view = if selection.help {
            View::Help
        } else if selection.inspection {
            View::Inspect
        } else if selection.menu {
            View::Menu
        } else if selection.search {
            View::Search
        } else {
            View::Normal
        };
        let compact = self.area().width < WIDE_TABLE;
        let status = status_lines(
            report,
            complete,
            session.notice.as_deref(),
            view,
            compact,
            bindings,
        );
        let home = session.home.as_path();
        let names = short_names(&report.rows);
        let shown = &*selection;
        let mut table_state = shown.table.clone();
        let result = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                let footer = footer_widget(bindings, view);
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
                            "{} · {} {}",
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
                    View::Inspect => {
                        let footer = format!(
                            "{} scroll · {}",
                            rows_hint(bindings, Mode::Inspection, true),
                            first_key_hint(bindings, Mode::Inspection, Action::Cancel, "close")
                        );
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
                    View::Menu => draw_popup(
                        frame,
                        POPUP,
                        "Menu / actions",
                        menu_widget(shown, session).scroll((shown.menu_scroll, 0)),
                        format!(
                            "{} select · {} · {}",
                            rows_hint(bindings, Mode::Menu, true),
                            first_key_hint(bindings, Mode::Menu, Action::Accept, "choose"),
                            first_key_hint(bindings, Mode::Menu, Action::Cancel, "back")
                        ),
                    ),
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
    Help,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Search => "SEARCH",
            Self::Inspect => "INSPECT",
            Self::Menu => "MENU",
            Self::Help => "HELP",
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

fn footer_widget(bindings: &Bindings, view: View) -> Paragraph<'static> {
    let first = |mode: Mode, action: Action| {
        bindings
            .keys(mode, action)
            .first()
            .map(|key| (*key).to_owned())
            .unwrap_or_default()
    };
    let items = match view {
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
        View::Help => vec![
            (rows_hint(bindings, Mode::Inspection, false), "scroll"),
            (first(Mode::Inspection, Action::Cancel), "close"),
        ],
        View::Normal | View::Inspect => {
            let mut items = vec![(rows_hint(bindings, Mode::Normal, false), "rows")];
            items.extend(
                [
                    (Action::Search, "search"),
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
    let mut spans = Vec::new();
    for (keys, label) in items.into_iter().filter(|(keys, _)| !keys.is_empty()) {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            keys,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {label}")));
    }
    Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true })
}

fn status_lines(
    report: &InventoryReport,
    complete: bool,
    notice: Option<&str>,
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
        View::Menu | View::Help => state.to_owned(),
        View::Normal | View::Inspect if compact && complete && !inspect.is_empty() => {
            format!("Path and origin: {} inspects the full value", inspect[0])
        }
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
        Mode::Inspection | Mode::Input | Mode::Confirmation => vec![
            (pair(Down, Up), "scroll"),
            (pair(PageUp, PageDown), "page up / down"),
            (single(Accept), "close"),
            (single(Cancel), "close"),
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

/// Lays out `name value` with the value in its own column; long values wrap inside it.
fn field_lines(name: &str, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width).saturating_sub(INSPECT_KEY_COLUMN).max(1);
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
    let reason = if row.markable {
        "unavailable · read-only inventory"
    } else {
        "unavailable · patterns are inspect-only"
    };
    for action in ["Register", "Clone", "Remove"] {
        lines.extend(field_lines(action, reason, dim(), width));
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

fn menu_widget(selection: &Selection, session: &InventorySession) -> Paragraph<'static> {
    Paragraph::new(
        root_menu_actions()
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
            .collect::<Vec<_>>(),
    )
}

fn root_menu_actions() -> &'static [Action] {
    &[
        Action::Inspect,
        Action::Search,
        Action::ClearSearch,
        Action::Refresh,
        Action::Help,
        Action::Quit,
    ]
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
    menu: bool,
    menu_index: usize,
    menu_scroll: u16,
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
                        Cell::from(Span::styled(
                            format!(" {}{text}", " ".repeat(slot(column))),
                            style,
                        ))
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
