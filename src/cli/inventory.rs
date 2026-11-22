use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
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
use crate::application::ports::{
    Desktop, EnsureEvent, EnsureReporter, HookRunner, HostError, ProviderCatalog,
};
use crate::application::remote::{
    self, ProviderPhase, ProviderState, RemoteBounds, RemoteDiscovery,
};
use crate::application::sessions::{self, Ensured, SessionError};
use crate::application::worktrees as worktree_service;
use crate::domain::repository::{RepositoryRef, validate_reference_safety};
use crate::infrastructure::config::{CommandConfigStore, load_inventory};
use crate::infrastructure::desktop::NativeDesktop;
use crate::infrastructure::filesystem::Filesystem;
use crate::infrastructure::git::NativeGit;
use crate::infrastructure::inventory::{
    CANCELLATION_BOUND, LocalInventoryFilesystem, ScanContext, ScanEvent, ScanHandle,
    start_local_scan_with_context,
};
use crate::infrastructure::providers::ConfiguredProviders;
use crate::infrastructure::rex::Rex;
use crate::infrastructure::session_record::SessionFile;
use crate::infrastructure::shell::{Shell, ShellError};
use crate::presentation::escape;

use super::args::InventoryArgs;

mod github;
mod keys;
pub(super) mod status;
mod worktrees;
use keys::{Action, Bindings, Mode};

pub fn run(config_path: &Path, args: InventoryArgs, interrupted: impl Fn() -> Option<i32>) -> i32 {
    if args.include_archived && !args.remote {
        eprintln!("lager: --include-archived requires --remote");
        return 2;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("lager: tui requires a TTY on stdin and stdout");
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
            session.remote.set_include_archived(args.include_archived);
            if args.remote {
                session.remote.enable();
            }
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
    let code = terminal.run(&mut session, &interrupted);
    // The terminal is restored before the session waits for background work to end.
    drop(terminal);
    session.stop_background();
    session.finish_open();
    if let Some(farewell) = &session.farewell {
        println!("{farewell}");
    }
    code.max(session.problems.exit_code())
}

struct InventorySession {
    config: crate::domain::config::Config,
    config_path: PathBuf,
    home: PathBuf,
    root: PathBuf,
    scan: LocalScan,
    worker: ScanHandle,
    generation: u64,
    /// The configuration revision the last refresh tried, loaded or not. The running scan
    /// started from it, and a changed revision reloads automatically.
    scan_revision: String,
    complete: bool,
    problems: SessionProblems,
    marks: Marks,
    bindings: Bindings,
    notice: Option<Notice>,
    /// The outcome of the last action; the next key press clears it.
    message: Option<String>,
    cached_report: Option<InventoryReport>,
    /// The running or last finished batch; a new batch replaces it.
    batch: Option<Batch>,
    remote: RemoteDiscovery<ConfiguredProviders>,
    /// The rows the provider catalogs added to the last report.
    remote_rows: Vec<model::RowKey>,
    /// How many rows the catalogs added when remote discovery was last on.
    remote_cached: usize,
    /// Per-repository lookups of declared GitHub rows the catalogs did not list, by identity;
    /// `None` when the lookup failed. A refresh clears them.
    lookups: HashMap<String, Option<model::RemoteStatus>>,
    lookup_results: (mpsc::Sender<Lookup>, mpsc::Receiver<Lookup>),
    github: github::Remote,
    /// The layout of `[session]`; `None` turns sessions off.
    session_layout: Option<crate::domain::session::Layout>,
    /// The host's live sessions by canonical checkout path with their labels, or why the host
    /// is unavailable; `None` until it first answers.
    hosted: Option<Result<BTreeMap<PathBuf, String>, String>>,
    /// The rows' checkout paths with a live session, as of the last report.
    hosted_rows: HashSet<PathBuf>,
    host_answers: (mpsc::Sender<HostAnswer>, mpsc::Receiver<HostAnswer>),
    /// The label of the session being opened.
    opening: Option<String>,
    /// Why sessions are unavailable, shown as the state until the next refresh.
    host_notice: Option<String>,
    /// The line printed once the terminal is restored.
    farewell: Option<String>,
    /// The number of the latest list request; answers to older ones are dropped.
    listing: u64,
}

/// What a session worker reports back.
enum HostAnswer {
    Listed(u64, Result<BTreeMap<PathBuf, String>, SessionError>),
    /// The line the farewell starts with, and the open's outcome.
    Opened(Option<String>, Result<Ensured, SessionError>),
    /// The row's checkout is no repository root Git can name.
    Refused(String),
}

/// Looks `reference` up on its own thread; no answer by `deadline` is an error naming `bound`.
fn bounded_lookup(
    providers: &std::sync::Arc<ConfiguredProviders>,
    reference: &RepositoryRef,
    deadline: Instant,
    bound: Duration,
) -> Result<Option<crate::application::ports::RemoteState>, String> {
    let late = || format!("no response within {} s", bound.as_secs());
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(late());
    }
    let (done, answer) = mpsc::channel();
    let (providers, asked) = (std::sync::Arc::clone(providers), reference.clone());
    std::thread::spawn(move || {
        let _ = done.send(providers.lookup(&asked));
    });
    // shortcut: a lookup past its bound stays unknown while its gh runs on; make it
    // cancellable like the catalog request if stalled lookups ever pile up.
    answer
        .recv_timeout(remaining)
        .unwrap_or_else(|_| Err(late()))
}

/// One answered lookup: the identity, the provider's answer, and whether remote discovery was
/// on when it started.
type Lookup = (
    String,
    Result<Option<crate::application::ports::RemoteState>, String>,
    bool,
);

/// The providers of a configuration that passed validation.
fn configured_providers(config: &crate::domain::config::Config) -> ConfiguredProviders {
    ConfiguredProviders::from_config(config).unwrap_or_default()
}

impl InventorySession {
    fn new(
        config: crate::domain::config::Config,
        config_path: PathBuf,
        home: PathBuf,
        root: PathBuf,
    ) -> Self {
        let scan_revision = LocalInventoryFilesystem.config_revision(&config_path);
        let worker = start_local_scan_with_context(ScanContext {
            generation: 1,
            config_revision: scan_revision.clone(),
            root: root.clone(),
        });
        let remote = RemoteDiscovery::new(
            configured_providers(&config),
            remote::wildcards(&config),
            RemoteBounds::default(),
        );
        let mut session = Self {
            remote,
            remote_rows: Vec::new(),
            remote_cached: 0,
            lookups: HashMap::new(),
            lookup_results: mpsc::channel(),
            github: github::Remote::default(),
            session_layout: super::session::layout(&config_path).ok(),
            hosted: None,
            hosted_rows: HashSet::new(),
            host_answers: mpsc::channel(),
            opening: None,
            host_notice: None,
            farewell: None,
            listing: 0,
            config,
            config_path,
            home,
            root,
            scan: LocalScan::default(),
            worker,
            generation: 1,
            scan_revision,
            complete: false,
            problems: SessionProblems::default(),
            marks: Marks::default(),
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            message: None,
            cached_report: None,
            batch: None,
        };
        session.list_sessions();
        session
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
            // The cached catalogs are merged into every rebuild, so a local refresh never
            // needs a provider.
            self.remote_rows = model::add_remote_catalog(
                &mut report,
                &LocalInventoryFilesystem,
                &self.config,
                &self.home,
                self.remote.members(),
                self.remote.include_archived(),
            )?;
            if self.remote.enabled() {
                self.remote_cached = self.remote_rows.len();
            }
            self.start_lookups(&report);
            model::apply_lookups(&mut report, &self.lookups, self.remote.enabled());
            // A wildcard is a scan pattern; the table lists its members only.
            report
                .rows
                .retain(|row| !matches!(row.key, model::RowKey::Pattern(_)));
            // Records name canonical paths; resolving them here keeps the filesystem out of
            // every frame.
            if let Some(Ok(hosted)) = &self.hosted {
                self.hosted_rows = report
                    .rows
                    .iter()
                    .filter_map(|row| row.observed_path.clone())
                    .filter(|path| hosted.contains_key(&super::session::key(path)))
                    .collect();
            }
            self.cached_report = Some(report);
        }
        Ok(self
            .cached_report
            .as_ref()
            .expect("report was cached")
            .clone())
    }

    /// Looks up, off the UI loop, the declared GitHub rows `model::unlisted` names. Each
    /// identity is looked up once until a refresh.
    fn start_lookups(&mut self, report: &InventoryReport) {
        let discovery = self.remote.enabled();
        let ready = |host: &str| {
            self.remote
                .providers()
                .get(host)
                .is_some_and(|provider| provider.phase == ProviderPhase::Ready)
        };
        let references = model::unlisted(report, &self.config, discovery, ready)
            .into_iter()
            .filter(|reference| !self.lookups.contains_key(&reference.identity()))
            .collect::<Vec<_>>();
        if references.is_empty() {
            return;
        }
        for reference in &references {
            self.lookups
                .insert(reference.identity(), Some(model::RemoteStatus::Pending));
        }
        let config = self.config.clone();
        let sender = self.lookup_results.0.clone();
        let bound = RemoteBounds::default().request;
        std::thread::spawn(move || {
            let providers = ConfiguredProviders::from_config(&config).map(std::sync::Arc::new);
            for reference in references {
                let state = match &providers {
                    Ok(providers) => {
                        bounded_lookup(providers, &reference, Instant::now() + bound, bound)
                    }
                    Err(error) => Err(error.clone()),
                };
                if sender
                    .send((reference.identity(), state, discovery))
                    .is_err()
                {
                    return;
                }
            }
        });
    }

    /// Asks the host for its live sessions off the UI loop; nothing without `[session]`.
    fn list_sessions(&mut self) {
        self.listing += 1;
        if self.session_layout.is_none() {
            return;
        }
        let (sender, request) = (self.host_answers.0.clone(), self.listing);
        std::thread::spawn(move || {
            let listed = sessions::list(&Rex, &SessionFile).map(|listed| {
                listed
                    .into_iter()
                    .map(|(path, recorded)| (path, recorded.label))
                    .collect()
            });
            let _ = sender.send(HostAnswer::Listed(request, listed));
        });
    }

    /// Opens the session of the primary checkout at `path` off the UI loop.
    fn open_session(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.open_checkout(super::session::label(&path, &path), None, move || {
            let repository =
                worktree_service::resolve_repository(&NativeGit, &Filesystem, &path, true)?;
            let label = super::session::label(&repository.primary, &repository.primary);
            Ok((repository.primary, label))
        });
    }

    /// Opens a session off the UI loop. `checkout` names the canonical checkout and its label,
    /// or why it has none; the farewell starts with `lead`.
    fn open_checkout(
        &mut self,
        label: String,
        lead: Option<String>,
        checkout: impl FnOnce() -> Result<(PathBuf, String), String> + Send + 'static,
    ) {
        let Some(layout) = self.session_layout.clone() else {
            return;
        };
        self.opening = Some(label);
        let sender = self.host_answers.0.clone();
        std::thread::spawn(move || {
            let answer = match checkout() {
                Ok((checkout, label)) => HostAnswer::Opened(
                    lead,
                    sessions::open(&Rex, &SessionFile, &checkout, &label, &layout),
                ),
                Err(reason) => HostAnswer::Refused(reason),
            };
            let _ = sender.send(answer);
        });
    }

    /// Waits for an open still in flight, so the session it creates is recorded. Each host
    /// call is bounded by the adapter.
    fn finish_open(&mut self) {
        let Some(label) = self.opening.clone().filter(|_| self.farewell.is_none()) else {
            return;
        };
        eprintln!("lager: waiting for session {} to open…", escape(label));
        // Only the workers hold senders now, so a worker that dies without answering ends it.
        self.host_answers.0 = mpsc::channel().0;
        while self.opening.is_some() && self.farewell.is_none() {
            match self.host_answers.1.recv() {
                Ok(HostAnswer::Listed(..)) => {}
                Ok(answer) => self.open_answered(answer),
                Err(_) => break,
            }
        }
        if let Some(message) = self.message.as_ref().filter(|_| self.farewell.is_none()) {
            eprintln!("lager: {message}");
        }
    }

    /// Takes the answer of an open: the farewell, or the reason it stays open.
    fn open_answered(&mut self, answer: HostAnswer) {
        self.opening = None;
        match answer {
            HostAnswer::Opened(lead, Ok(ensured)) => {
                let verb = match ensured {
                    Ensured::Created(_) => "Opened",
                    Ensured::Existing(_) => "Switched to",
                };
                let line = format!("{verb} session {}", escape(&ensured.recorded().label));
                self.farewell = Some(match lead {
                    Some(lead) => format!("{lead}\n{line}"),
                    None => line,
                });
            }
            HostAnswer::Opened(lead, Err(error)) => {
                let lead = lead.map(|lead| format!("{lead} · ")).unwrap_or_default();
                self.message = Some(match self.host_error(error) {
                    Ok(reason) => format!("{lead}Open session unavailable: {reason}"),
                    Err(reason) => {
                        self.problems.record("session", reason.clone());
                        format!("! {lead}Open session failed: {reason}")
                    }
                });
            }
            HostAnswer::Refused(reason) => {
                self.message = Some(format!("Open session unavailable: {}", escape(reason)));
            }
            HostAnswer::Listed(..) => {}
        }
    }

    /// The reason of a failed host call: `Ok` for a missing or stopped host, which is a notice
    /// given when the host turns unavailable, and `Err` for a call that failed on a running
    /// host, which the caller records as a problem.
    fn host_error(&mut self, error: SessionError) -> Result<String, String> {
        match error {
            SessionError::Host(HostError::Unavailable(reason)) => {
                let reason = escape(reason);
                if !matches!(self.hosted, Some(Err(_))) {
                    self.host_notice = Some(format!("Sessions unavailable: {reason}"));
                }
                self.hosted = Some(Err(reason.clone()));
                Ok(reason)
            }
            SessionError::Host(HostError::Failed(reason)) | SessionError::Records(reason) => {
                Err(escape(model::problem_reason(&reason)))
            }
        }
    }

    fn event_is_current(&self, context: &ScanContext) -> bool {
        context.generation == self.generation
            && context.root == self.root
            && context.config_revision
                == LocalInventoryFilesystem.config_revision(&self.config_path)
    }

    fn receive_events(&mut self) {
        let (mut received, failures) = self.remote.poll();
        while let Ok((identity, state, discovery)) = self.lookup_results.1.try_recv() {
            // A lookup started with remote discovery off fails quietly: the origin stays alone.
            if let Err(error) = &state
                && discovery
            {
                self.problems.record(
                    format!("lookup · {}", escape(&identity)),
                    escape(model::problem_reason(error)),
                );
            }
            self.lookups
                .insert(identity, model::RemoteStatus::looked_up(state));
            received = true;
        }
        while let Ok(answer) = self.host_answers.1.try_recv() {
            match answer {
                HostAnswer::Listed(request, _) if request != self.listing => {}
                HostAnswer::Listed(_, Ok(listed)) => {
                    self.hosted = Some(Ok(listed));
                    self.host_notice = None;
                }
                HostAnswer::Listed(_, Err(error)) => {
                    if let Err(reason) = self.host_error(error) {
                        self.problems.record("session", reason);
                    }
                }
                answer => {
                    let failed = matches!(answer, HostAnswer::Opened(_, Err(_)));
                    self.open_answered(answer);
                    if failed {
                        self.list_sessions();
                    }
                }
            }
            received = true;
        }
        for failure in failures {
            self.problems.record(
                format!("provider · {}", escape(&failure.provider)),
                escape(model::problem_reason(&failure.error)),
            );
        }
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

    /// A configuration changed on disk drops every later event of the running scan, so the scan
    /// is restarted with it, once per revision. It waits for the normal view, so bindings never
    /// change under an open dialog, and for a batch, which refreshes after its own writes. A
    /// revision that fails to load is a notice, not a session problem: an editor's half-written
    /// save is retried on its next save.
    fn reload_changed_config(&mut self, idle: bool) {
        if self.complete || self.running() || !idle {
            return;
        }
        let revision = LocalInventoryFilesystem.config_revision(&self.config_path);
        if revision == self.scan_revision {
            return;
        }
        self.notice = Some(match self.refresh() {
            Ok(()) => match self.notice.take() {
                Some(Notice::Warning(warning)) => {
                    Notice::Warning(format!("Config changed on disk · reloaded · {warning}"))
                }
                _ => Notice::Info("Config changed on disk · reloaded".to_owned()),
            },
            Err((error, _)) => Notice::Warning(format!(
                "R retries · config changed on disk but did not load: {} · previous configuration stays",
                reload_reason(&error, &self.config_path)
            )),
        });
    }

    /// Ends the local scan and the provider requests after the terminal is restored. Both count
    /// down one cancellation bound; what still runs after it is left behind.
    fn stop_background(&mut self) {
        github::finish(self);
        let deadline = Instant::now() + CANCELLATION_BOUND;
        self.worker.cancel();
        self.remote.disable();
        // Work that ends within moments of the cancel is waited for without a word.
        let quiet = Instant::now() + Duration::from_millis(100);
        while self.busy() && Instant::now() < quiet {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.busy() {
            eprintln!(
                "lager: waiting up to {} s for background work to stop…",
                CANCELLATION_BOUND.as_secs()
            );
        }
        self.worker.finish_by(deadline);
        self.remote.shutdown_by(deadline);
    }

    fn busy(&mut self) -> bool {
        !self.worker.finished() || self.remote.busy()
    }

    fn reject_changed_target(&mut self, path: PathBuf) {
        self.scan.pending_paths.retain(|pending| pending != &path);
        self.scan.checkouts.retain(|checkout| checkout.path != path);
        if !self.scan.stale_paths.contains(&path) {
            self.scan.stale_paths.push(path);
        }
    }

    /// Installs the configuration and bindings together, or neither. The error names what
    /// stays active. A scan in progress when a revision fails to load restarts with the
    /// previous configuration, since its events carry a revision that is no longer on disk.
    fn refresh(&mut self) -> Result<(), (String, &'static str)> {
        self.lookups.clear();
        let installed = self.install_config();
        if installed.is_err() && !self.complete {
            self.worker.cancel();
            self.receive_events();
            self.restart_scan();
        }
        installed
    }

    fn install_config(&mut self) -> Result<(), (String, &'static str)> {
        // A revision tried here, automatically or by `R`, is not reloaded automatically again.
        self.scan_revision = LocalInventoryFilesystem.config_revision(&self.config_path);
        let kept = |error: String| (error, "configuration stays");
        let (config, overrides, warnings) =
            load_inventory(&self.config_path).map_err(|error| kept(error.to_string()))?;
        let bindings = Bindings::load(overrides).map_err(|error| (error, "bindings stay"))?;
        let root = config
            .resolve_root(&self.home)
            .map_err(|error| kept(error.to_string()))?;
        // The old worker's events carry its generation and are dropped from here on; it is
        // not waited for, so a scan blocked in a filesystem call cannot hold the screen.
        self.worker.cancel();
        self.receive_events();
        self.remote
            .reconfigure(configured_providers(&config), remote::wildcards(&config));
        self.config = config;
        self.bindings = bindings;
        self.notice = warning_notice(warnings);
        self.host_notice = None;
        self.root = root;
        self.session_layout = super::session::layout(&self.config_path).ok();
        if self.session_layout.is_none() {
            self.hosted = None;
        }
        self.list_sessions();
        self.restart_scan();
        Ok(())
    }

    /// Starts a scan of the installed configuration under `scan_revision`.
    fn restart_scan(&mut self) {
        self.generation += 1;
        self.cached_report = None;
        self.scan = stale_scan(&self.scan);
        std::mem::replace(
            &mut self.worker,
            start_local_scan_with_context(ScanContext {
                generation: self.generation,
                config_revision: self.scan_revision.clone(),
                root: self.root.clone(),
            }),
        )
        .detach();
        self.complete = false;
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

    /// Why no batch can start now: one is running, or a GitHub action may move what it touches.
    fn batch_blocked(&self) -> Option<&'static str> {
        if self.running() {
            Some("A batch is already running")
        } else if self.github.running() {
            Some("A GitHub action is running")
        } else {
            None
        }
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

/// The first line of a configuration load error without the config path, short enough that the
/// reload notice keeps its tail at 132 columns.
fn reload_reason(error: &str, config_path: &Path) -> String {
    let error = error.lines().next().unwrap_or_default();
    let path = format!(" {}", config_path.display());
    let reason = error.replacen(&path, "", 1);
    let reason = reason
        .split_once("config: ")
        .map_or(reason.as_str(), |(_, rest)| rest);
    let reason = escape(model::problem_reason(reason));
    match reason.char_indices().nth(40) {
        Some((end, _)) => format!("{}…", &reason[..end]),
        None => reason,
    }
}

/// A line above the status that stays until the next refresh replaces it.
enum Notice {
    Info(String),
    Warning(String),
}

fn warning_notice(warnings: Vec<String>) -> Option<Notice> {
    (!warnings.is_empty()).then(|| Notice::Warning(format!("warning: {}", warnings.join(" · "))))
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
    /// Rows below Help in the last frame: the status lines and the footer.
    below_help: u16,
    /// The area of the last frame; a key is judged against what was on screen when it was typed.
    drawn: Rect,
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
        Ok(Self {
            terminal,
            restore,
            below_help: 2,
            drawn: Rect::default(),
        })
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
                let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                    .areas(frame.area());
                frame.render_widget(bar("LAGER / inventory", "", header.width), header);
                frame.render_widget(
                    Paragraph::new(
                        "Scanning repository roots…\nGit status appears as observations complete.",
                    )
                    .wrap(Wrap { trim: false }),
                    body.inner(Margin {
                        horizontal: 2,
                        vertical: 1,
                    }),
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
                session.worker.cancel();
                return code;
            }
            session.receive_events();
            if session.farewell.is_some() && !session.running() {
                return quit(session);
            }
            github::poll(session, &mut selection);
            session.reload_changed_config(selection.view() == View::Normal && !selection.quit);
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
                        session.worker.cancel();
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
                        session.worker.cancel();
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
            if matches!(
                selection.view(),
                View::Preview
                    | View::AddPreview
                    | View::WorktreePlan
                    | View::WorktreeConfirm
                    | View::RemoteDeleteConfirm
                    | View::RemoteVisibilityConfirm
            ) && std::mem::take(&mut selection.preview_fresh)
            {
                discard_input();
                held = None;
            }
            if let Err(error) = self.draw_report(&report, &visible_rows, session, &mut selection) {
                eprintln!("lager: terminal render failed: {}", escape(error));
                // A closed terminal fails the draw around the moment its SIGHUP lands.
                return interrupted().unwrap_or(1);
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
                session.worker.cancel();
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
                            session.worker.cancel();
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
                            action
                                if selection.worktrees.is_some()
                                    && view != View::Help
                                    && action != Some(Action::Help) =>
                            {
                                let area = above_status(self.drawn, self.below_help);
                                if let Some(native) =
                                    worktrees::key(session, &mut selection, action, key, area)
                                    && let Some(code) = worktrees::run(
                                        self,
                                        session,
                                        &mut selection,
                                        native,
                                        interrupted,
                                    )
                                {
                                    return code;
                                }
                            }
                            action
                                if selection.github.is_some()
                                    && view != View::Help
                                    && action != Some(Action::Help) =>
                            {
                                github::key(session, &mut selection, action, key);
                            }
                            Some(Action::Worktrees) if view == View::Normal => {
                                let row = selection.selected_row(&visible_rows);
                                let name = row
                                    .map(|row| short_names(&report.rows)[&row.key].clone())
                                    .unwrap_or_default();
                                worktrees::open(session, &mut selection, row, name);
                            }
                            Some(Action::Add) if view == View::Normal => {
                                open_add(session, &mut selection);
                            }
                            Some(Action::Session)
                                if view == View::Normal && session.session_layout.is_some() =>
                            {
                                if let Some(code) = run_direct(
                                    session,
                                    &mut selection,
                                    &report,
                                    &visible_rows,
                                    Action::Session,
                                ) {
                                    return code;
                                }
                            }
                            Some(action @ (Action::Remove | Action::RemoveUnregister))
                                if view == View::Normal =>
                            {
                                let row = selection.selected_row(&visible_rows);
                                confirm_removal(
                                    session,
                                    &mut selection,
                                    &report,
                                    &visible_rows,
                                    row,
                                    if action == Action::Remove {
                                        BatchAction::Remove
                                    } else {
                                        BatchAction::RemoveUnregister
                                    },
                                );
                            }
                            Some(
                                action @ (Action::Register
                                | Action::Unregister
                                | Action::Clone
                                | Action::CloneRegister
                                | Action::Copy
                                | Action::Open
                                | Action::Remote
                                | Action::Retry
                                | Action::Archives
                                | Action::Rename
                                | Action::Archive
                                | Action::Visibility
                                | Action::DeleteRemote
                                | Action::CreateRemote),
                            ) if view == View::Normal => {
                                if let Some(code) = run_direct(
                                    session,
                                    &mut selection,
                                    &report,
                                    &visible_rows,
                                    action,
                                ) {
                                    return code;
                                }
                            }
                            // Opening Add turned remote discovery on; browsing only closes
                            // the form.
                            Some(Action::Accept)
                                if view == View::AddInput
                                    && selection.add.as_ref().is_some_and(|form| form.browse) =>
                            {
                                selection.add = None;
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
                            // Tab is the form's own focus key unless the input map binds it.
                            None if view == View::AddInput
                                && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) =>
                            {
                                if let Some(form) = selection.add.as_mut() {
                                    form.browse = !form.browse;
                                }
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
                            Some(acting @ (Action::Acknowledge | Action::Accept))
                                if view == View::Preview
                                    && selection.preview.as_ref().is_some_and(|batch| {
                                        acknowledgment(batch).is_some()
                                            && (acting == Action::Acknowledge
                                                || selection.preview_focus == 0)
                                    }) =>
                            {
                                // Accepting a loss the user has not seen does nothing.
                                let limit = self.preview_scroll_limit(&selection, session);
                                if selection.preview_seen < limit {
                                    session.message = Some(format!(
                                        "Scroll through every loss before accepting · {}",
                                        session
                                            .bindings
                                            .keys(Mode::Confirmation, Action::PageDown)
                                            .join("/")
                                    ));
                                } else if let Some(batch) = selection.preview.as_mut() {
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
                                } else if let Some(reason) = session.batch_blocked() {
                                    session.message = Some(reason.to_owned());
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
                            // A page is the visible list, so paging shows every line.
                            Some(Action::PageUp) if view == View::Preview => {
                                let page = self.preview_page(&selection, session);
                                selection.preview_scroll =
                                    selection.preview_scroll.saturating_sub(page);
                            }
                            Some(Action::PageDown) if view == View::Preview => {
                                let page = self.preview_page(&selection, session);
                                selection.preview_scroll = selection
                                    .preview_scroll
                                    .saturating_add(page)
                                    .min(self.preview_scroll_limit(&selection, session));
                                selection.preview_seen =
                                    selection.preview_seen.max(selection.preview_scroll);
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
                                    session.marks.toggle(row);
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
            above_status(self.area(), self.below_help),
            WIDE_POPUP,
            &help_widget(&session.bindings, mode, session.session_layout.is_some()),
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

    fn problems_scroll_limit(&self, session: &InventorySession) -> u16 {
        popup_scroll_limit(self.area(), WIDE_POPUP, &problems_widget(session))
    }

    fn preview_scroll_limit(&self, selection: &Selection, session: &InventorySession) -> u16 {
        selection.preview.as_ref().map_or(0, |batch| {
            let width = popup_body_width(self.area(), WIDE_POPUP);
            let lines = preview_lines(batch, &session.home, width);
            let choices = preview_choices(batch, selection.preview_focus);
            let (_, list, _, _) = preview_regions(self.area(), &lines, choices.len() as u16);
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .line_count(list.width)
                .saturating_sub(usize::from(list.height.max(1)))
                .min(usize::from(u16::MAX)) as u16
        })
    }

    /// The rows of the preview list in view.
    fn preview_page(&self, selection: &Selection, session: &InventorySession) -> u16 {
        selection.preview.as_ref().map_or(1, |batch| {
            let width = popup_body_width(self.area(), WIDE_POPUP);
            let lines = preview_lines(batch, &session.home, width);
            let choices = preview_choices(batch, selection.preview_focus);
            preview_regions(self.area(), &lines, choices.len() as u16)
                .1
                .height
                .max(1)
        })
    }

    fn batch_scroll_limit(&self, session: &InventorySession) -> u16 {
        session.batch.as_ref().map_or(0, |batch| {
            let area = self.area();
            popup_scroll_limit(
                area,
                WIDE_POPUP,
                &batch_widget(batch, popup_body_width(area, WIDE_POPUP), false),
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
            selection.inspection_scroll = selection
                .inspection_scroll
                .min(self.inspection_scroll_limit(row, &report.rows, &session.home));
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
                        .filter(|_| view == View::Normal)
                        .map(|key| format!(" · {key} → cancel batch"))
                        .unwrap_or_default()
                )
            })
            .or_else(|| github::status(session))
            .or_else(|| {
                (session.opening.as_ref())
                    .map(|label| format!("Opening session {}…", escape(label)))
            });
        let remote_state = remote_state(&session.remote);
        let root_missing = report
            .diagnostics
            .iter()
            .any(model::ScanDiagnostic::is_root_missing);
        let status = status_lines(
            report,
            &session.home,
            complete,
            session.notice.as_ref(),
            session.host_notice.as_deref(),
            session.message.as_deref().or(progress.as_deref()),
            remote_state.as_ref(),
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
        // Marks and hints wait for the host's answer and stay away while it is unavailable.
        let hosted = matches!(session.hosted, Some(Ok(_))).then_some(&session.hosted_rows);
        let marked = session.marks.len();
        let counter = (marked > 0).then(|| {
            format!(
                "{marked} marked · {} hidden",
                session.marks.hidden(visible_rows)
            )
        });
        let mut below_help = self.below_help;
        let mut drawn = self.drawn;
        let result = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                drawn = area;
                let footer = footer_widget(
                    bindings,
                    view,
                    area.width.saturating_sub(4),
                    !report.rows.is_empty(),
                    hosted.is_some(),
                );
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
                below_help = area.bottom().saturating_sub(status_area.y);
                frame.render_widget(
                    bar(
                        "LAGER / inventory",
                        &format!(
                            "{} · {} {}{problems}",
                            view.label(),
                            visible_rows.len(),
                            if visible_rows.len() == 1 {
                                "row"
                            } else {
                                "rows"
                            }
                        ),
                        header.width,
                    ),
                    header,
                );
                frame.render_widget(
                    Paragraph::new(source_line(session, report)).style(dim()),
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
                        repository_table(
                            &[],
                            &names,
                            &session.marks,
                            hosted,
                            home,
                            table.width,
                            None,
                        ),
                        table,
                    );
                    let message = if !report.rows.is_empty() && !shown.query.is_empty() {
                        Text::raw("No matching repositories.")
                    } else if !complete {
                        Text::raw("Discovering local repositories…")
                    } else if report.incomplete {
                        Text::raw("No repositories found so far; local scan is incomplete.")
                    } else {
                        empty_inventory(session, root_missing)
                    };
                    let [_, body] =
                        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(table);
                    frame.render_widget(
                        Paragraph::new(message).wrap(Wrap { trim: false }),
                        padded(body),
                    );
                } else {
                    // ratatui scrolls only far enough to show the selection and never pulls the
                    // window back, so a shorter list would keep rows above the viewport.
                    let page = usize::from(table.height.saturating_sub(1));
                    *table_state.offset_mut() = table_state
                        .offset()
                        .min(visible_rows.len().saturating_sub(page));
                    frame.render_stateful_widget(
                        repository_table(
                            visible_rows,
                            &names,
                            &session.marks,
                            hosted,
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
                    View::Help => draw_popup_in(
                        frame,
                        above_status(frame.area(), below_help),
                        WIDE_POPUP,
                        "Keyboard / effective bindings",
                        help_widget(bindings, shown.help_mode, session.session_layout.is_some())
                            .scroll((shown.help_scroll, 0)),
                        format!("{} scroll · {close}", rows_hint(bindings, Mode::Inspection)),
                    ),
                    View::Problems => draw_popup(
                        frame,
                        WIDE_POPUP,
                        &format!("Problems / {}", session.problems.entries().len()),
                        problems_widget(session).scroll((shown.problems_scroll, 0)),
                        format!("{} scroll · {close}", rows_hint(bindings, Mode::Inspection)),
                    ),
                    View::Inspect => {
                        let footer = [
                            format!("{} scroll", rows_hint(bindings, Mode::Inspection)),
                            first_key_hint(bindings, Mode::Inspection, Action::Copy, "copy"),
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
                                    popup_body_width(area, WIDE_POPUP),
                                )
                                .scroll((shown.inspection_scroll, 0)),
                                footer,
                            ),
                            None => draw_popup(
                                frame,
                                WIDE_POPUP,
                                "Inspect / unavailable",
                                Paragraph::new("Inspected target is unavailable or stale.")
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
                                rows_hint(bindings, Mode::Menu),
                                first_key_hint(bindings, Mode::Menu, Action::Accept, "choose"),
                                first_key_hint(bindings, Mode::Menu, Action::Cancel, "back")
                            ),
                        )
                    }
                    View::Preview => {
                        if let Some(batch) = &shown.preview {
                            let lines =
                                preview_lines(batch, home, popup_body_width(area, WIDE_POPUP));
                            let choice_lines = preview_choices(batch, shown.preview_focus);
                            let ([popup, title, _, footer], list, more, choices) =
                                preview_regions(area, &lines, choice_lines.len() as u16);
                            let below = Paragraph::new(lines.clone())
                                .wrap(Wrap { trim: false })
                                .line_count(list.width)
                                .saturating_sub(
                                    usize::from(shown.preview_scroll) + usize::from(list.height),
                                );
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
                            if let Some(more) = more.filter(|_| below > 0) {
                                let page_down = bindings
                                    .keys(Mode::Confirmation, Action::PageDown)
                                    .join("/");
                                frame.render_widget(
                                    Paragraph::new(format!(
                                        "… {below} more line{} · {page_down}{}",
                                        if below == 1 { "" } else { "s" },
                                        if acknowledges {
                                            " · Space works once every loss is shown"
                                        } else {
                                            ""
                                        }
                                    ))
                                    .style(warn()),
                                    more,
                                );
                            }
                            frame.render_widget(Paragraph::new(choice_lines), choices);
                            let hint = |action, label| {
                                first_key_hint(bindings, Mode::Confirmation, action, label)
                            };
                            let focus_hint =
                                format!("{} focus", rows_hint(bindings, Mode::Confirmation));
                            let scroll = format!("{pages} scroll");
                            let (choose, cancel) = (
                                hint(Action::Accept, "choose"),
                                hint(Action::Cancel, "cancel"),
                            );
                            // A narrow popup cuts the end of the line, so in a removal
                            // confirmation the keys that act come before scrolling; the row
                            // above the choices names the paging key while lines are left.
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
                                    popup_body_width(area, WIDE_POPUP),
                                    shown.batch_cancel,
                                )
                                .scroll((shown.batch_scroll, 0)),
                                if batch.finished() {
                                    format!(
                                        "{} scroll · {}",
                                        rows_hint(bindings, Mode::Inspection),
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
                                        .find(|key| back.first() != Some(key) && back.contains(key))
                                        .map(|key| format!(" · {key} quit"))
                                        .unwrap_or_default();
                                    format!(
                                        "{} focus · {} scroll · {} · {}{quit}",
                                        rows_hint(bindings, Mode::Inspection),
                                        paired_keys(
                                            &bindings.keys(Mode::Inspection, Action::PageUp),
                                            &bindings.keys(Mode::Inspection, Action::PageDown),
                                        ),
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
                                    "{} focus · {} · {}",
                                    rows_hint(bindings, Mode::Confirmation),
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
                                add_input_widget(form, popup_body_width(area, POPUP)),
                                format!(
                                    "{} · Tab focus · {}",
                                    first_key_hint(
                                        bindings,
                                        Mode::Input,
                                        Action::Accept,
                                        if form.browse { "browse" } else { "preview" }
                                    ),
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
                                home,
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
                            // The fields give way before the choices do.
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
                                    rows_hint(bindings, Mode::Confirmation),
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
                    View::Worktrees
                    | View::WorktreeInput
                    | View::WorktreePlan
                    | View::WorktreeBlocked
                    | View::WorktreeConfirm => {
                        if let Some(popup) = &shown.worktrees {
                            worktrees::draw(
                                frame,
                                popup,
                                bindings,
                                session
                                    .hosted
                                    .as_ref()
                                    .and_then(|hosted| hosted.as_ref().ok()),
                                above_status(frame.area(), below_help),
                            );
                        }
                    }
                    View::RemoteCreateInput
                    | View::RemoteRenameInput
                    | View::RemoteDeleteConfirm
                    | View::RemoteVisibilityConfirm => {
                        if let Some(dialog) = &shown.github {
                            github::draw(
                                frame,
                                dialog,
                                session,
                                above_status(frame.area(), below_help),
                            );
                        }
                    }
                    View::Normal | View::Search => {}
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string());
        self.below_help = below_help;
        self.drawn = drawn;
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
    Worktrees,
    WorktreeInput,
    WorktreePlan,
    WorktreeBlocked,
    WorktreeConfirm,
    RemoteCreateInput,
    RemoteRenameInput,
    RemoteDeleteConfirm,
    RemoteVisibilityConfirm,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Search => "SEARCH",
            Self::Inspect => "INSPECT",
            Self::Menu => "MENU",
            Self::Problems => "PROBLEMS",
            Self::Preview
            | Self::QuitBatch
            | Self::WorktreePlan
            | Self::WorktreeConfirm
            | Self::RemoteDeleteConfirm
            | Self::RemoteVisibilityConfirm => "CONFIRM",
            Self::Worktrees | Self::WorktreeBlocked => "WORKTREES",
            Self::Batch => "RESULTS",
            Self::Help => "HELP",
            Self::AddInput
            | Self::AddPreview
            | Self::WorktreeInput
            | Self::RemoteCreateInput
            | Self::RemoteRenameInput => "INPUT",
        }
    }

    fn mode(self) -> Mode {
        match self {
            Self::Normal => Mode::Normal,
            Self::Search => Mode::Search,
            Self::Menu => Mode::Menu,
            Self::Inspect | Self::Problems | Self::Batch | Self::Help | Self::WorktreeBlocked => {
                Mode::Inspection
            }
            Self::Preview
            | Self::AddPreview
            | Self::QuitBatch
            | Self::WorktreePlan
            | Self::WorktreeConfirm
            | Self::RemoteVisibilityConfirm => Mode::Confirmation,
            // The delete confirmation takes a typed name, so letters must stay text.
            Self::AddInput
            | Self::WorktreeInput
            | Self::RemoteCreateInput
            | Self::RemoteRenameInput
            | Self::RemoteDeleteConfirm => Mode::Input,
            Self::Worktrees => Mode::Worktrees,
        }
    }
}

const POPUP: u16 = 66;
const WIDE_POPUP: u16 = 100;
/// Terminal width at which the table shows every column of the wide grid.
const WIDE_TABLE: u16 = 132;
/// Reserved for the mark column; the repository text starts after it on every row.
const MARK_SLOT: usize = 4;
/// Reserved after the mark column for the open-session dot while sessions are shown.
const SESSION_SLOT: usize = 2;
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

/// The body of a finished, complete scan that found nothing and has nothing declared.
fn empty_inventory(session: &InventorySession, root_missing: bool) -> Text<'static> {
    let root = home_relative(&session.root, &session.home);
    let mut lines = vec![
        Line::raw("No repositories yet."),
        Line::raw(""),
        Line::styled(
            if root_missing {
                format!("{root} does not exist yet and the configuration declares none.")
            } else {
                format!("{root} holds no Git repositories and the configuration declares none.")
            },
            dim(),
        ),
    ];
    let mut next = Vec::new();
    if let Some(key) = session.bindings.keys(Mode::Normal, Action::Add).first() {
        next.push(Span::styled((*key).to_owned(), focus()));
        next.push(Span::styled(
            " adds one by reference or from your providers.",
            dim(),
        ));
    }
    if !session.remote.enabled() && session.remote_cached == 0 {
        next.push(Span::styled(
            if next.is_empty() {
                "Nothing has been contacted."
            } else {
                " Nothing has been contacted."
            },
            dim(),
        ));
    }
    if !next.is_empty() {
        lines.push(Line::from(next));
    }
    Text::from(lines)
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

fn rows_hint(bindings: &Bindings, mode: Mode) -> String {
    [
        bindings.keys(mode, Action::Down).first(),
        bindings.keys(mode, Action::Up).first(),
    ]
    .into_iter()
    .flatten()
    .copied()
    .collect::<Vec<_>>()
    .join("/")
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
    has_rows: bool,
    sessions: bool,
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
        // With no rows only the actions that need none are offered.
        View::Normal if !has_rows => [
            (Action::Add, "add"),
            (Action::Refresh, "refresh"),
            (Action::Menu, "menu"),
            (Action::Help, "help"),
            (Action::Quit, "quit"),
        ]
        .map(|(action, label)| (first(Mode::Normal, action), label))
        .to_vec(),
        View::Normal => {
            let mut items = vec![(rows_hint(bindings, Mode::Normal), "rows")];
            items.extend(
                [
                    (Action::Search, "search"),
                    (Action::Mark, "mark"),
                    (Action::Add, "add"),
                    (Action::Remove, "remove"),
                    (Action::Worktrees, "worktrees"),
                    (Action::Session, "session"),
                    (Action::Inspect, "inspect"),
                    (Action::Refresh, "refresh"),
                    (Action::Menu, "menu"),
                    (Action::Help, "help"),
                    (Action::Quit, "quit"),
                ]
                .into_iter()
                .filter(|(action, _)| sessions || *action != Action::Session)
                .map(|(action, label)| (first(Mode::Normal, action), label)),
            );
            items
        }
        // A popup carries its own hint row.
        _ => Vec::new(),
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
        "session",
        "worktrees",
        "mark",
        "remove",
        "add",
        "rows",
        "refresh",
        "search",
        "inspect",
        "delete",
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

/// What remote discovery is doing, for the status line.
enum RemoteState {
    Loading,
    /// Every failed provider with its error, already escaped.
    Failed(String),
}

fn remote_state(remote: &RemoteDiscovery<ConfiguredProviders>) -> Option<RemoteState> {
    if !remote.enabled() {
        return None;
    }
    if remote.loading() {
        return Some(RemoteState::Loading);
    }
    let failures = remote
        .providers()
        .iter()
        .filter_map(|(host, provider)| match &provider.phase {
            ProviderPhase::Failed(error) => Some(format!(
                "{} failed: {}",
                escape(host),
                escape(model::problem_reason(error))
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    (!failures.is_empty()).then(|| RemoteState::Failed(failures.join(" · ")))
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// How many of the rows the catalogs added come from this provider.
fn candidates(session: &InventorySession, host: &str) -> usize {
    session
        .remote_rows
        .iter()
        .filter(|key| {
            matches!(key, model::RowKey::Declaration { identity, .. }
                if identity.split('/').next().is_some_and(|owner| owner.eq_ignore_ascii_case(host)))
        })
        .count()
}

/// A provider's state as the source line and the Remote discovery menu word it.
fn provider_text(
    provider: &ProviderState,
    enabled: bool,
    candidates: usize,
    separator: &str,
    detailed: bool,
) -> String {
    let fetched = provider
        .cached
        .as_ref()
        .map(|cache| format!("fetched {}", clock(cache.fetched)));
    let stale = if provider.stale() {
        format!("{separator}{candidates} cached (stale)")
    } else {
        String::new()
    };
    match (&provider.phase, fetched) {
        (_, Some(fetched)) if !enabled => format!("off{separator}{fetched}"),
        (_, None) if !enabled => "not fetched".to_owned(),
        (_, Some(_)) if provider.config_changed => {
            format!("{candidates} cached (stale){separator}config changed")
        }
        (_, None) if provider.config_changed => format!("config changed{separator}not fetched"),
        (ProviderPhase::Idle, _) => format!("not fetched{stale}"),
        (ProviderPhase::Loading { .. }, Some(_)) => format!("refreshing{stale}"),
        (ProviderPhase::Loading { since }, None) => {
            format!("loading… {} s", since.elapsed().as_secs())
        }
        (ProviderPhase::Ready, _) => {
            let ready = format!(
                "ready{separator}{}",
                counted(candidates, "candidate", "candidates")
            );
            match provider.cached.as_ref().filter(|_| detailed) {
                Some(cache) => format!("{ready}{separator}{}", clock(cache.fetched)),
                None => ready,
            }
        }
        (ProviderPhase::Failed(error), _) if detailed => {
            format!("failed{separator}{}", escape(model::problem_reason(error)))
        }
        (ProviderPhase::Failed(_), _) => format!("failed{stale}"),
    }
}

fn source_line(session: &InventorySession, report: &InventoryReport) -> String {
    let remote = &session.remote;
    let pending = report
        .rows
        .iter()
        .filter(|row| {
            row.branch == model::ObservationField::Pending
                || row.changes == model::ObservationField::Pending
        })
        .count();
    let local = match (session.complete, report.incomplete) {
        (false, false) => "Local: scanning",
        (false, true) => "Local: incomplete, still scanning",
        (true, true) => "Local: incomplete",
        (true, false) if pending > 0 || remote.enabled() => "Local: ready",
        (true, false) => "Local + declarations",
    };
    let mut parts = vec![local.to_owned()];
    if pending > 0 {
        parts.push(format!("status: {pending} pending"));
    }
    if !remote.enabled() {
        parts.push("remote off".to_owned());
        if session.remote_cached > 0 {
            parts.push(format!(
                "{} cached for this run",
                counted(session.remote_cached, "candidate", "candidates")
            ));
        }
        return parts.join(" · ");
    }
    if remote.providers().is_empty() {
        parts.push("no providers configured".to_owned());
    }
    for (host, provider) in remote.providers() {
        parts.push(format!(
            "{}: {}",
            escape(host),
            provider_text(provider, true, candidates(session, host), ", ", false)
        ));
    }
    parts.join(" · ")
}

#[allow(clippy::too_many_arguments)]
fn status_lines(
    report: &InventoryReport,
    home: &Path,
    complete: bool,
    notice: Option<&Notice>,
    host_notice: Option<&str>,
    message: Option<&str>,
    remote: Option<&RemoteState>,
    view: View,
    compact: bool,
    bindings: &Bindings,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match notice {
        Some(Notice::Info(text)) => lines.push(Line::raw(text.clone())),
        Some(Notice::Warning(text)) => lines.push(Line::styled(format!("! {text}"), warn())),
        None => {}
    }
    let (notes, diagnostics): (Vec<_>, Vec<_>) = report
        .diagnostics
        .iter()
        .partition(|diagnostic| diagnostic.is_root_missing());
    // With no rows the empty table says that the root is missing.
    if !report.rows.is_empty() {
        for note in notes {
            lines.push(Line::styled(
                format!("{} does not exist yet", home_relative(&note.path, home)),
                dim(),
            ));
        }
    }
    if report.incomplete {
        let reasons = diagnostics
            .iter()
            .map(|diagnostic| {
                let path = home_relative(&diagnostic.path, home);
                match diagnostic.message.strip_suffix(" ignored") {
                    Some(kind) if !diagnostic.incomplete => {
                        format!("{path} ignored: {}", escape(kind))
                    }
                    _ => format!(
                        "{path} {}",
                        escape(model::problem_reason(&diagnostic.message))
                    ),
                }
            })
            .collect::<Vec<_>>();
        lines.push(Line::styled(
            format!("! Scan incomplete: {}", reasons.join(" · ")),
            warn(),
        ));
    }
    let excluded = diagnostics
        .iter()
        .filter(|diagnostic| !diagnostic.incomplete && !report.incomplete)
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
    }
    let state = match remote {
        _ if !complete => "Scanning local repositories…",
        None if report.rows.is_empty() && !report.incomplete => {
            "Scan complete · 0 repositories · 0 declarations"
        }
        Some(RemoteState::Loading) => "Remote on · local rows stay usable while providers load",
        _ => "No active operation",
    };
    // A failed provider is the current state once nothing else is going on.
    if let Some(RemoteState::Failed(failures)) = remote
        && complete
        && message.is_none()
        && view != View::Search
    {
        lines.push(Line::styled(format!("! {failures}"), warn()));
        return lines;
    }
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
        View::Normal if complete && host_notice.is_some() => {
            host_notice.unwrap_or_default().to_owned()
        }
        View::Normal
            if complete && remote.is_none() && report.rows.is_empty() && !report.incomplete =>
        {
            state.to_owned()
        }
        View::Normal if compact && complete && remote.is_none() && !inspect.is_empty() => {
            format!(
                "Path and origin: {} inspects, menu copies the full value",
                inspect[0]
            )
        }
        View::Normal if refresh.is_empty() => state.to_owned(),
        View::Normal => format!("{state} · {refresh}"),
        _ => state.to_owned(),
    };
    let warned = operation.starts_with("! ") || host_notice == Some(operation.as_str());
    lines.push(Line::styled(operation, if warned { warn() } else { dim() }));
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
    draw_popup_in(frame, frame.area(), cap, title, body, footer);
}

/// Help leaves the status and footer rows visible, so a configuration failure stays readable.
fn above_status(area: Rect, below: u16) -> Rect {
    Rect {
        height: area.height.saturating_sub(below),
        ..area
    }
}

fn draw_popup_in(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    cap: u16,
    title: &str,
    body: Paragraph<'static>,
    footer: String,
) {
    let [popup, title_area, content, footer_area] = popup_regions(area, cap, &body);
    frame.render_widget(Clear, popup);
    frame.render_widget(Block::bordered().border_style(focus()), popup);
    frame.render_widget(bar(title, "lager", title_area.width), title_area);
    frame.render_widget(body, content);
    frame.render_widget(Paragraph::new(footer).style(dim()), footer_area);
}

fn help_widget(bindings: &Bindings, mode: Mode, sessions: bool) -> Paragraph<'static> {
    use Action::*;
    let keys = |action| bindings.keys(mode, action);
    // Default lists pair up by position (`j/k ↑/↓`); configured lists show their direction.
    let pair = |first, second| {
        if bindings.configured(mode, first).is_none() && bindings.configured(mode, second).is_none()
        {
            return paired_keys(&keys(first), &keys(second));
        }
        [first, second]
            .into_iter()
            .filter(|action| !keys(*action).is_empty())
            .map(|action| {
                let arrow = if matches!(action, Up | PageUp) {
                    "↑"
                } else {
                    "↓"
                };
                format!("{} {arrow}", keys(action).join("/"))
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let single = |action| keys(action).join("/");
    let help_quit = || {
        [single(Help), single(Quit)]
            .into_iter()
            .filter(|keys| !keys.is_empty())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let entry = |keys: String, description: &str, actions: &[Action]| {
        let notes = actions
            .iter()
            .filter_map(|action| {
                bindings.configured(mode, *action).map(|keys| {
                    format!(
                        "{} = [{}]",
                        action.name(),
                        keys.iter()
                            .map(|key| format!("{key:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
            })
            .collect::<Vec<_>>();
        let description = if notes.is_empty() {
            description.to_owned()
        } else {
            format!("{description} (config: {})", notes.join(", "))
        };
        (keys, description)
    };
    let entries: Vec<(String, String)> = match mode {
        Mode::Normal => {
            let mut entries = vec![
                entry(pair(Down, Up), "previous / next row", &[Down, Up]),
                entry(single(Search), "live fuzzy search", &[Search]),
                entry(single(ClearSearch), "clear filter", &[ClearSearch]),
                entry(single(Mark), "toggle current mark", &[Mark]),
                entry(single(Add), "Add repository", &[Add]),
                entry(
                    single(Remove),
                    "Remove checkout, keep registration",
                    &[Remove],
                ),
                entry(
                    single(Worktrees),
                    "Worktrees of the current repository",
                    &[Worktrees],
                ),
            ];
            if sessions {
                entries.push(entry(
                    single(Session),
                    "Session of the current repository",
                    &[Session],
                ));
            }
            entries.extend(
                DIRECT_ACTIONS.map(|action| entry(single(action), direct_label(action), &[action])),
            );
            entries.extend([
                entry(single(Inspect), "Inspect repository", &[Inspect]),
                entry(single(Refresh), "refresh local/config state", &[Refresh]),
                entry(single(Menu), "all actions", &[Menu]),
                entry(help_quit(), "help / quit", &[Help, Quit]),
            ]);
            entries
        }
        Mode::Menu => vec![
            entry(pair(Down, Up), "previous / next entry", &[Down, Up]),
            entry(
                pair(PageUp, PageDown),
                "page up / down",
                &[PageUp, PageDown],
            ),
            entry(single(Accept), "choose", &[Accept]),
            entry(single(Cancel), "back", &[Cancel]),
            entry(single(Search), "live fuzzy search", &[Search]),
            entry(single(ClearSearch), "clear filter", &[ClearSearch]),
            entry(single(Inspect), "Inspect repository", &[Inspect]),
            entry(single(Refresh), "refresh local/config state", &[Refresh]),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ],
        Mode::Search => vec![
            entry(
                [keys(Accept), keys(Cancel)].concat().join(" / "),
                "keep the filter",
                &[Accept, Cancel],
            ),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ],
        Mode::Inspection => vec![
            entry(pair(Down, Up), "scroll", &[Down, Up]),
            entry(
                pair(PageUp, PageDown),
                "page up / down",
                &[PageUp, PageDown],
            ),
            entry(single(Copy), "copy a value…", &[Copy]),
            entry(single(Accept), "close", &[Accept]),
            entry(single(Cancel), "close", &[Cancel]),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ],
        Mode::Confirmation => vec![
            entry(pair(Down, Up), "move focus", &[Down, Up]),
            entry(
                pair(PageUp, PageDown),
                "page up / down",
                &[PageUp, PageDown],
            ),
            entry(
                single(Acknowledge),
                "acknowledge the listed losses",
                &[Acknowledge],
            ),
        ]
        .into_iter()
        .chain(sessions.then(|| entry(single(Session), SESSION_TICK, &[Session])))
        .chain([
            entry(single(Accept), "choose", &[Accept]),
            entry(single(Cancel), "cancel", &[Cancel]),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ])
        .collect(),
        Mode::Input => vec![
            entry(single(Accept), "preview the reference", &[Accept]),
            entry(single(Cancel), "back", &[Cancel]),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ],
        Mode::Worktrees => vec![
            entry(pair(Down, Up), "previous / next worktree", &[Down, Up]),
            entry(
                pair(PageUp, PageDown),
                "page up / down",
                &[PageUp, PageDown],
            ),
            entry(single(Add), "add a worktree…", &[Add]),
            entry(single(Remove), "remove the selected worktree", &[Remove]),
            entry(single(Copy), "copy path", &[Copy]),
        ]
        .into_iter()
        .chain(sessions.then(|| {
            entry(
                single(Session),
                "open the session of the selected worktree",
                &[Session],
            )
        }))
        .chain([
            entry(single(Accept), "close", &[Accept]),
            entry(single(Cancel), "close", &[Cancel]),
            entry(help_quit(), "help / quit", &[Help, Quit]),
        ])
        .collect(),
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
        Mode::Worktrees => "Worktrees mode",
    };
    let line = |keys: String, description: &str| {
        let padding = column
            .saturating_sub(Line::raw(keys.clone()).width())
            .max(1);
        Line::from(vec![
            Span::styled(keys, focus()),
            Span::raw(format!("{}{description}", " ".repeat(padding))),
        ])
    };
    let normal = bindings
        .unbound(Mode::Normal)
        .into_iter()
        .filter(|action| {
            DIRECT_ACTIONS.contains(action) && bindings.configured(Mode::Normal, *action).is_none()
        })
        .map(direct_label)
        .collect::<Vec<_>>();
    // The normal actions without a key follow the normal keys, in either section.
    let unbound = (!normal.is_empty()).then(|| {
        Line::from(vec![
            Span::styled(format!("{:<column$}", "unbound"), dim()),
            Span::raw(normal.join(", ")),
        ])
    });
    let mut lines = vec![Line::styled(title, dim())];
    lines.extend(
        entries
            .into_iter()
            .map(|(keys, description)| line(keys, &description)),
    );
    if mode == Mode::Normal {
        lines.extend(unbound.clone());
    }
    if sessions && mode == Mode::Normal {
        lines.extend([
            Line::raw(""),
            Line::styled("Sessions", dim()),
            line("●".to_owned(), "open session"),
        ]);
        if let Some(key) = keys(Session).first() {
            lines.push(line(
                (*key).to_owned(),
                "opens the session or shows the open one, then the inventory closes",
            ));
        }
        if let Some(key) = bindings.keys(Mode::Confirmation, Session).first() {
            lines.push(line((*key).to_owned(), SESSION_TICK));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Other modes", dim()));
    for other in [
        Mode::Normal,
        Mode::Search,
        Mode::Menu,
        Mode::Input,
        Mode::Confirmation,
        Mode::Inspection,
        Mode::Worktrees,
    ] {
        if other == mode {
            continue;
        }
        let first = |action| first_key_hint(bindings, other, action, "");
        let item = |keys: String, label: &str| {
            (!keys.trim().is_empty()).then(|| format!("{} {label}", keys.trim_end()))
        };
        let rows = rows_hint(bindings, other);
        let (name, items) = match other {
            Mode::Normal => (
                "normal",
                vec![
                    item(rows, "rows"),
                    item(first(Menu), "all actions"),
                    item(first(Quit), "quit"),
                ],
            ),
            Mode::Search => (
                "search",
                vec![item(
                    [first(Accept), first(Cancel)]
                        .map(|key| key.trim().to_owned())
                        .into_iter()
                        .filter(|key| !key.is_empty())
                        .collect::<Vec<_>>()
                        .join(" / "),
                    "keep the filter · letters are text",
                )],
            ),
            Mode::Menu => (
                "menu",
                vec![
                    item(rows, "select"),
                    item(first(Accept), "choose"),
                    item(first(Cancel), "back one level"),
                ],
            ),
            Mode::Input => (
                "input",
                vec![item(first(Accept), "accept"), item(first(Cancel), "cancel")],
            ),
            Mode::Confirmation => (
                "confirm",
                vec![
                    item(rows, "focus"),
                    item(first(Acknowledge), "acknowledge"),
                    sessions.then(|| item(first(Session), "session")).flatten(),
                    item(first(Accept), "choose"),
                    item(first(Cancel), "cancel"),
                ],
            ),
            Mode::Inspection => (
                "inspect",
                vec![
                    item(
                        format!(
                            "{rows} {}",
                            paired_keys(
                                &bindings.keys(other, PageUp),
                                &bindings.keys(other, PageDown)
                            )
                        ),
                        "scroll",
                    ),
                    item(first(Copy), "copy a value"),
                    item(first(Cancel), "close"),
                ],
            ),
            Mode::Worktrees => (
                "worktrees",
                vec![
                    item(rows, "select"),
                    sessions.then(|| item(first(Session), "session")).flatten(),
                    item(first(Add), "add"),
                    item(first(Remove), "remove"),
                    item(first(Copy), "copy path"),
                    item(first(Cancel), "close"),
                ],
            ),
        };
        let items = items.into_iter().flatten().collect::<Vec<_>>();
        lines.push(line(name.to_owned(), &items.join(" · ")));
        if other == Mode::Normal {
            lines.extend(unbound.clone());
        }
    }
    for (other, name) in [
        (Mode::Normal, "normal"),
        (Mode::Menu, "menu"),
        (Mode::Search, "search"),
        (Mode::Input, "input"),
        (Mode::Confirmation, "confirmation"),
        (Mode::Inspection, "inspection"),
        (Mode::Worktrees, "worktrees"),
    ] {
        // The menu borrows these normal keys only where its own keys leave them free.
        let missing = bindings
            .unbound(other)
            .into_iter()
            .filter(|action| sessions || *action != Session)
            .filter(|action| match other {
                Mode::Normal => {
                    !DIRECT_ACTIONS.contains(action)
                        || bindings.configured(other, *action).is_some()
                }
                Mode::Menu => !matches!(action, Search | Inspect | Refresh | ClearSearch),
                _ => true,
            })
            .map(Action::name)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            lines.push(Line::raw(format!(
                "No key in {name} mode: {}.",
                missing.join(", ")
            )));
        }
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// What the session key does in a worktree's plan or removal summary.
const SESSION_TICK: &str = "plan: open the session after · removal: close it after";

/// Normal-mode actions that run a menu entry; none has a default key.
const DIRECT_ACTIONS: [Action; 15] = [
    Action::Register,
    Action::Unregister,
    Action::Clone,
    Action::CloneRegister,
    Action::RemoveUnregister,
    Action::Copy,
    Action::Open,
    Action::Remote,
    Action::Retry,
    Action::Archives,
    Action::Rename,
    Action::Archive,
    Action::Visibility,
    Action::DeleteRemote,
    Action::CreateRemote,
];

fn direct_label(action: Action) -> &'static str {
    match action {
        Action::Register => "Register",
        Action::Unregister => "Unregister",
        Action::Clone => "Clone only",
        Action::CloneRegister => "Clone + register",
        Action::RemoveUnregister => "Remove + unregister",
        Action::Copy => "Copy",
        Action::Open => "Open origin",
        Action::Remote => "Remote",
        Action::Retry => "Retry remote",
        Action::Archives => "Remote archives",
        Action::Rename => "Rename on GitHub",
        Action::Archive => "Archive on GitHub",
        Action::Visibility => "Change visibility on GitHub",
        Action::DeleteRemote => "Delete everywhere",
        Action::CreateRemote => "New repository on GitHub",
        other => other.name(),
    }
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
/// shares that last segment, then one leading segment at a time while another repository
/// still shares the name. Patterns compare only with patterns, and a path row at a declared
/// destination counts as that declaration. A checkout reads
/// `name (copy)`, `name (copy 2)`, … when its identity already has a cloned declaration or a
/// checkout at an earlier path.
fn short_names(rows: &[model::InventoryRow]) -> HashMap<model::RowKey, String> {
    let declared: HashMap<&Path, &str> = rows
        .iter()
        .filter_map(|row| match &row.key {
            model::RowKey::Declaration {
                identity,
                destination,
            } => Some((destination.as_path(), identity.as_str())),
            _ => None,
        })
        .collect();
    // The last field is the group a row counts as when names are compared past depth 1: a path
    // row at a declared destination counts as that declaration.
    let groups = rows
        .iter()
        .map(|row| match &row.key {
            model::RowKey::Checkout { identity, .. }
            | model::RowKey::Declaration { identity, .. } => {
                (0, identity.clone(), '/', identity.clone())
            }
            model::RowKey::Pattern(pattern) => (1, pattern.clone(), '/', pattern.clone()),
            model::RowKey::Path(path) => {
                let text = path.to_string_lossy().into_owned();
                let counted = declared
                    .get(path.as_path())
                    .map_or_else(|| text.clone(), |identity| (*identity).to_owned());
                (2, text, std::path::MAIN_SEPARATOR, counted)
            }
        })
        .collect::<Vec<_>>();
    let mut depths = groups
        .iter()
        .map(|(kind, ..)| if *kind == 1 { 2 } else { 1 })
        .collect::<Vec<usize>>();
    loop {
        let name = |index: usize| {
            let (kind, group, separator, _) = &groups[index];
            let depth = depths[index];
            (
                (depth > 1).then_some(*kind == 1),
                trailing(group, *separator, depth),
            )
        };
        let mut sharing: HashMap<_, HashSet<&str>> = HashMap::new();
        for (index, (_, group, _, counted)) in groups.iter().enumerate() {
            let value = if depths[index] > 1 { counted } else { group };
            sharing.entry(name(index)).or_default().insert(value);
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
        .map(|(row, ((_, group, separator, _), depth))| {
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

/// Splits `value` into pieces at most `room` cells wide, so a column can wrap in place. A piece
/// breaks at the last space that fits and drops it; a word wider than `room` is cut.
fn column_chunks(value: &str, room: usize) -> Vec<String> {
    let room = room.max(1);
    let width = |text: &str| Span::raw(text.to_owned()).width();
    let mut chunks = vec![String::new()];
    let mut used = 0;
    for character in value.chars() {
        let size = Span::raw(character.to_string()).width();
        if used + size > room && used > 0 {
            let current = chunks.last_mut().expect("one chunk");
            let carried = match current.rfind(' ').filter(|space| *space > 0) {
                Some(space) if character != ' ' => {
                    let tail = current.split_off(space + 1);
                    current.pop();
                    tail
                }
                _ => String::new(),
            };
            used = width(&carried);
            chunks.push(carried);
        }
        if character == ' ' && used == 0 && chunks.len() > 1 {
            continue;
        }
        chunks.last_mut().expect("one chunk").push(character);
        used += size;
    }
    chunks
}

/// Splits `value` into pieces at most `room` cells wide, cutting by cell, so an exact value keeps
/// every character, spaces included.
fn cell_chunks(value: &str, room: usize) -> Vec<String> {
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

/// Lays out `name value` with the value in its own column; long prose wraps at spaces inside it.
fn field_lines(name: &str, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    labelled_lines(
        name,
        column_chunks(value, usize::from(width).saturating_sub(INSPECT_KEY_COLUMN)),
        style,
    )
}

/// Like [`field_lines`] for a path, identity, URL or branch: it wraps by cell.
fn exact_field_lines(name: &str, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    labelled_lines(
        name,
        cell_chunks(value, usize::from(width).saturating_sub(INSPECT_KEY_COLUMN)),
        style,
    )
}

fn labelled_lines(name: &str, chunks: Vec<String>, style: Style) -> Vec<Line<'static>> {
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
    let mut lines = exact_field_lines("Repository", &name, Style::default(), width);
    if let Some(identity) = identity(row) {
        lines.extend(exact_field_lines(
            "Identity",
            &escape(identity),
            Style::default(),
            width,
        ));
    }
    let nonstandard = row.observed_path.is_some()
        && row.configured_destination.is_some()
        && row.observed_path != row.configured_destination;
    if let Some(path) = &row.observed_path {
        lines.extend(exact_field_lines(
            "Found path",
            &home_relative(path, home),
            Style::default(),
            width,
        ));
        if nonstandard && let Some(found) = lines.last_mut() {
            found.spans.push(Span::styled(" · nonstandard", warn()));
        }
    }
    // Paths, identities, origins and branches wrap by cell; prose wraps at spaces.
    let mut field = |name: &str, value: String, style: Style, exact: bool| {
        lines.extend(if exact {
            exact_field_lines(name, &value, style, width)
        } else {
            field_lines(name, &value, style, width)
        });
    };
    if let Some(path) = &row.configured_destination {
        field(
            "Configured path",
            home_relative(path, home),
            Style::default(),
            true,
        );
    }
    if row.observed_path.is_none() && row.configured_destination.is_none() {
        field(
            "Path",
            home_relative(Path::new(&row.path), home),
            Style::default(),
            true,
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
        model::RegistrationState::Unregistered => "unregistered".to_owned(),
        model::RegistrationState::Pattern => "wildcard · declaration pattern".to_owned(),
    };
    field(
        "Registration",
        registration,
        registration_style(&row.registration),
        false,
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
                    " · satisfied by {}",
                    home_relative(Path::new(&declared.path), home)
                ),
                ref state => format!(" · configured path {}", state.label()),
            })
            .unwrap_or_default();
        field(
            "Declaration",
            format!("{kind}{target}"),
            Style::default(),
            false,
        );
    }
    field(
        "Checkout",
        checkout_text(&row.checkout).to_owned(),
        checkout_style(&row.checkout),
        false,
    );
    field("Origin", escape(&row.origin), Style::default(), true);
    if let Some(status) = row.remote_status {
        field("Remote", status.label().to_owned(), Style::default(), false);
    }
    for (name, observation) in [("Branch", &row.branch), ("Changes", &row.changes)] {
        field(
            name,
            observation_text(observation),
            observation_style(observation),
            name == "Branch",
        );
    }
    // Found path carries the misplacement against the shown configured path as `nonstandard`;
    // a misplacement against another declaration of the identity stays a warning.
    let marked = row
        .configured_destination
        .as_ref()
        .map(|path| format!("; configured: {}", path.to_string_lossy()));
    let warnings = row
        .warnings
        .iter()
        .filter(|warning| {
            !(nonstandard
                && warning.starts_with("misplaced checkout;")
                && marked.as_ref().is_some_and(|tail| warning.ends_with(tail)))
        })
        .collect::<Vec<_>>();
    if !warnings.is_empty() {
        lines.push(Line::default());
        lines.extend(
            warnings
                .into_iter()
                .map(|warning| Line::styled(escape(warning), warn())),
        );
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Root,
    Repository,
    Copy,
    Selection,
    Remote,
    Visibility,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Open(Menu),
    Inspect,
    OpenOrigin,
    Refresh,
    Problems,
    Add,
    Worktrees,
    OpenSession,
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
    RemoteEnable,
    RemoteDisable,
    RemoteRetry,
    RemoteRefresh,
    RemoteArchives,
    Github(github::Act),
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
        Menu::Remote => "Remote discovery".to_owned(),
        Menu::Visibility => format!("Change visibility / {name}"),
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
            vec![
                heading(format!(
                    "This row · {}",
                    if row.is_some() { name } else { "—" }
                )),
                on_row(
                    Entry::Open(Menu::Repository),
                    "Repository actions…",
                    String::new(),
                ),
                on_row(Entry::Inspect, "Inspect repository", key(Action::Inspect)),
                on_row(Entry::Open(Menu::Copy), "Copy value…", String::new()),
                on_row(Entry::OpenOrigin, "Open origin in browser", String::new()),
                match row.map(model::worktree_checkout) {
                    Some(Err(reason)) => {
                        item(Entry::Worktrees, "Worktrees…", reason.to_owned(), false)
                    }
                    _ => on_row(Entry::Worktrees, "Worktrees…", key(Action::Worktrees)),
                },
            ]
            .into_iter()
            .chain(session.session_layout.as_ref().map(|_| {
                let refused = match (row.map(model::worktree_checkout), &session.hosted) {
                    (None, _) => Some("no row".to_owned()),
                    (Some(Err(reason)), _) => Some(reason.to_owned()),
                    (_, Some(Err(reason))) => Some(reason.clone()),
                    _ if session.opening.is_some() => Some("a session is opening".to_owned()),
                    _ if session.running() => Some("a batch is running".to_owned()),
                    _ if session.github.running() => Some("a GitHub action is running".to_owned()),
                    _ => None,
                };
                match refused {
                    Some(reason) => item(Entry::OpenSession, "Open session", reason, false),
                    None => item(
                        Entry::OpenSession,
                        "Open session",
                        key(Action::Session),
                        true,
                    ),
                }
            }))
            .chain([
                heading(String::new()),
                heading(format!("Selection · {} marked", session.marks.len())),
                item(
                    Entry::Open(Menu::Selection),
                    "Selection…",
                    String::new(),
                    true,
                ),
                heading(String::new()),
                heading("Inventory".to_owned()),
                item(Entry::Add, "Add repository", key(Action::Add), true),
            ])
            .chain(github::create_line(session))
            .chain([
                item(
                    Entry::Open(Menu::Remote),
                    "Remote discovery…",
                    match remote_state(&session.remote) {
                        _ if !session.remote.enabled() => "off".to_owned(),
                        Some(RemoteState::Loading) => "on · loading".to_owned(),
                        Some(RemoteState::Failed(_)) => {
                            format!("on · {} failed", session.remote.failed().len())
                        }
                        None => "on".to_owned(),
                    },
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
                    "Problems…",
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
            ])
            .collect()
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
            let mut absent = None;
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
                        absent = preview.absent();
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
            if let Some(reason) = absent {
                return item(entry, label, reason, false);
            }
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
        .chain(github::group(session, row))
        .collect(),
        Menu::Visibility => github::visibility_lines(session, row),
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
        Menu::Remote => {
            let remote = &session.remote;
            let enabled = remote.enabled();
            let providers = remote.providers().len();
            let mut lines = remote
                .providers()
                .iter()
                .map(|(host, provider)| MenuLine {
                    entry: None,
                    label: escape(host),
                    hint: provider_text(provider, enabled, candidates(session, host), " · ", true),
                    enabled: false,
                })
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                lines.push(heading(String::new()));
            }
            let failed = remote.failed();
            let off = |entry, label: &str| item(entry, label, "remote is off".to_owned(), false);
            let retry = match (enabled, failed.is_empty()) {
                (false, _) => off(Entry::RemoteRetry, "Retry failed providers"),
                (true, true) => item(
                    Entry::RemoteRetry,
                    "Retry failed providers",
                    "none failed".to_owned(),
                    false,
                ),
                (true, false) => item(
                    Entry::RemoteRetry,
                    "Retry failed providers",
                    escape(failed.join(", ")),
                    true,
                ),
            };
            let refresh = if enabled {
                item(
                    Entry::RemoteRefresh,
                    "Refresh all enabled providers",
                    String::new(),
                    providers > 0,
                )
            } else {
                off(Entry::RemoteRefresh, "Refresh all enabled providers")
            };
            let archives = item(
                Entry::RemoteArchives,
                if remote.include_archived() {
                    "[x] Include archived"
                } else {
                    "[ ] Include archived"
                },
                String::new(),
                true,
            );
            if enabled {
                let marks = session
                    .remote_rows
                    .iter()
                    .filter(|key| session.marks.contains(key))
                    .count();
                lines.extend([
                    retry,
                    refresh,
                    archives,
                    item(
                        Entry::RemoteDisable,
                        "Disable remote",
                        format!(
                            "hides {}, removes {}",
                            counted(session.remote_rows.len(), "row", "rows"),
                            counted(marks, "mark", "marks")
                        ),
                        true,
                    ),
                ]);
            } else {
                lines.extend([
                    if providers == 0 {
                        item(
                            Entry::RemoteEnable,
                            "Enable remote",
                            "no providers configured".to_owned(),
                            false,
                        )
                    } else {
                        item(
                            Entry::RemoteEnable,
                            "Enable remote",
                            format!("fetches {}", counted(providers, "provider", "providers")),
                            true,
                        )
                    },
                    archives,
                    retry,
                    refresh,
                ]);
            }
            lines
        }
        Menu::Selection => {
            let marked = session.marks.len();
            let has_row = row.is_some();
            vec![
                item(
                    Entry::ToggleMark,
                    if row.is_some_and(|row| session.marks.contains(&row.key)) {
                        "Unmark this row"
                    } else {
                        "Mark this row"
                    },
                    match row {
                        Some(_) => key(Action::Mark),
                        None => "no row".to_owned(),
                    },
                    has_row,
                ),
                item(
                    Entry::MarkVisible,
                    "Mark visible concrete rows",
                    format!("{} visible", visible.len()),
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
                if line.entry.is_none() && line.hint.is_empty() {
                    return Line::styled(line.label.clone(), dim());
                }
                if line.entry.is_none() {
                    // A fact with its state, as the providers of the Remote discovery menu.
                    let mut fact = choice_line(&line.label, &line.hint, true, false, width);
                    fact.spans[1].style = Style::default();
                    return fact;
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
            open_add(session, selection);
            return None;
        }
        (Entry::Worktrees, row) => {
            worktrees::open(session, selection, row, name);
            return None;
        }
        (Entry::OpenSession, Some(row)) => {
            if let Ok(path) = model::worktree_checkout(row) {
                session.open_session(path);
            }
            None
        }
        (Entry::Github(act), row) => {
            github::activate(session, selection, act, row);
            return None;
        }
        (Entry::Problems, _) => {
            selection.problems = true;
            selection.problems_scroll = 0;
            None
        }
        (Entry::Unavailable(message), _) => Some(message.to_owned()),
        (Entry::RemoteEnable, _) => {
            enable_remote(session);
            None
        }
        (Entry::RemoteDisable, _) => Some(disable_remote(session, report)),
        (Entry::RemoteRetry, _) => {
            let failed = escape(session.remote.failed().join(", "));
            session.remote.retry();
            session.cached_report = None;
            Some(format!("Retrying {failed}"))
        }
        (Entry::RemoteRefresh, _) => {
            session.remote.refresh();
            session.cached_report = None;
            Some(format!(
                "Refreshing {} · cached rows stay, shown stale",
                counted(session.remote.providers().len(), "provider", "providers")
            ))
        }
        (Entry::RemoteArchives, _) => {
            let include = !session.remote.include_archived();
            session.remote.set_include_archived(include);
            session.cached_report = None;
            Some(format!(
                "Archived repositories {} · {}",
                if include { "included" } else { "hidden" },
                if session.remote.enabled() {
                    "refetching"
                } else {
                    "applies when remote is on"
                }
            ))
        }
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
        (Entry::Run(_), _) if session.batch_blocked().is_some() => {
            session.batch_blocked().map(str::to_owned)
        }
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
                selection.preview_seen = 0;
            }
            None
        }
        _ => None,
    };
    None
}

/// Runs the menu entry of a direct key on the current row or the marks, with the entry's
/// eligibility: a disabled entry reports its reason instead.
fn run_direct(
    session: &mut InventorySession,
    selection: &mut Selection,
    report: &InventoryReport,
    visible_rows: &[&model::InventoryRow],
    action: Action,
) -> Option<i32> {
    let (menu, entry) = match action {
        Action::Register => (Menu::Repository, Entry::Run(BatchAction::Register)),
        Action::Unregister => (Menu::Repository, Entry::Run(BatchAction::Unregister)),
        Action::Clone => (Menu::Repository, Entry::Run(BatchAction::Clone)),
        Action::CloneRegister => (Menu::Repository, Entry::Run(BatchAction::CloneRegister)),
        Action::Copy => (Menu::Root, Entry::Open(Menu::Copy)),
        Action::Open => (Menu::Root, Entry::OpenOrigin),
        Action::Remote => (Menu::Root, Entry::Open(Menu::Remote)),
        Action::Retry => (Menu::Remote, Entry::RemoteRetry),
        Action::Archives => (Menu::Remote, Entry::RemoteArchives),
        Action::Rename => (Menu::Repository, Entry::Github(github::Act::Rename)),
        Action::Archive => (Menu::Repository, Entry::Github(github::Act::Archive(true))),
        Action::Visibility => (Menu::Repository, Entry::Open(Menu::Visibility)),
        Action::DeleteRemote => (Menu::Repository, Entry::Github(github::Act::Delete)),
        Action::Session => (Menu::Root, Entry::OpenSession),
        _ => (Menu::Root, Entry::Github(github::Act::Create)),
    };
    // Archive and visibility entries name the state they lead to.
    let kind = |entry: Entry| match entry {
        Entry::Github(github::Act::Archive(_)) => Entry::Github(github::Act::Archive(true)),
        Entry::Github(github::Act::Visibility(_)) => Entry::Open(Menu::Visibility),
        entry => entry,
    };
    selection.open_menu(menu, visible_rows);
    let row = selection.menu_target_row(&report.rows);
    let name = row
        .map(|row| short_names(&report.rows)[&row.key].clone())
        .unwrap_or_default();
    let lines = menu_lines(menu, row, &name, session, visible_rows, &report.rows)
        .into_iter()
        .filter(|line| line.entry.is_some())
        .collect::<Vec<_>>();
    let Some(index) = lines
        .iter()
        .position(|line| line.entry.map(kind) == Some(entry))
    else {
        selection.menu.clear();
        session.message = Some(format!(
            "{} unavailable: {}",
            direct_label(action),
            if action == Action::CreateRemote {
                "no GitHub provider has create in remote_actions"
            } else {
                "no GitHub provider for this row"
            }
        ));
        return None;
    };
    let line = &lines[index];
    if !line.enabled {
        selection.menu.clear();
        session.message = Some(format!(
            "{} unavailable: {}",
            line.label.trim_end_matches('…'),
            line.hint
        ));
        return None;
    }
    if let Some(Entry::Open(submenu)) = line.entry {
        selection.menu = vec![(submenu, 0)];
        return None;
    }
    selection.menu = vec![(menu, index)];
    activate_menu_entry(session, selection, report, visible_rows)
}

/// Opens the Add form. Opening it turns remote discovery on when a provider is configured, and
/// it stays on when the form closes.
fn open_add(session: &mut InventorySession, selection: &mut Selection) {
    selection.add = Some(AddForm::default());
    // Without a configured provider there is nothing to turn on.
    if session.remote.enabled() || session.remote.providers().is_empty() {
        return;
    }
    enable_remote(session);
    session.message = Some("Remote enabled by Add · providers loading".to_owned());
}

fn enable_remote(session: &mut InventorySession) {
    session.remote.enable();
    session.lookups.retain(|_, status| status.is_some());
    session.cached_report = None;
}

/// Turns remote discovery off and drops the marks of the rows that disappear with it. Returns
/// what was hidden and removed, for the status line.
fn disable_remote(session: &mut InventorySession, report: &InventoryReport) -> String {
    let names = short_names(&report.rows);
    let hidden = session.remote_rows.len();
    let mut removed = Vec::new();
    for row in &report.rows {
        if session.remote_rows.contains(&row.key) && session.marks.contains(&row.key) {
            session.marks.toggle(row);
            removed.push(names[&row.key].clone());
        }
    }
    session.remote.disable();
    session.cached_report = None;
    let mut message = format!(
        "Remote off · {} hidden",
        counted(hidden, "remote-only row", "remote-only rows")
    );
    if !removed.is_empty() {
        message.push_str(&format!(
            " · {} removed: {}",
            counted(removed.len(), "mark", "marks"),
            removed.join(", ")
        ));
    }
    message
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
    if let Some(reason) = session.batch_blocked() {
        session.message = Some(reason.to_owned());
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
    selection.preview_seen = 0;
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
    /// The reference form focuses "Browse remote candidates" instead of the preview.
    browse: bool,
}

/// A spacer and the four preview choices, kept in view before the fields.
const ADD_CHOICES_HEIGHT: u16 = 5;

/// The preview's choices in board order; the cursor index past them is "Back to reference".
const ADD_CHOICES: [AddChoice; 3] = [
    AddChoice::Register,
    AddChoice::Clone,
    AddChoice::CloneRegister,
];

fn add_input_widget(form: &AddForm, width: u16) -> Paragraph<'static> {
    let width = usize::from(width);
    let mut lines = vec![
        Line::styled("reference", dim()),
        Line::styled(format!("{}█", escape(&form.text)), focus()),
    ];
    if let Some(reason) = &form.rejected {
        lines.push(Line::styled(
            format!("Rejected: {}", escape(reason)),
            tint(Color::Red),
        ));
    }
    lines.extend([
        Line::default(),
        choice_line("Preview destination", "", true, !form.browse, width),
        choice_line("Browse remote candidates", "", true, form.browse, width),
    ]);
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// The preview's fields, and its choices; the choices stay in view when long values leave too
/// little room.
fn add_preview_lines(
    preview: &AddPreview,
    choice: usize,
    home: &Path,
    width: u16,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let checkout = match preview.checkout {
        model::CheckoutState::Missing => "missing",
        model::CheckoutState::Cloned => "cloned",
        model::CheckoutState::Conflict => "conflict",
        model::CheckoutState::Unreadable => "unreadable",
        _ => "unknown",
    };
    let checkout_style = match preview.checkout {
        model::CheckoutState::Cloned => Style::default(),
        model::CheckoutState::Missing => warn(),
        _ => tint(Color::Red),
    };
    let fields = [
        ("identity", escape(&preview.identity), Style::default()),
        (
            "destination",
            home_relative(&preview.destination, home),
            Style::default(),
        ),
        ("checkout", checkout.to_owned(), checkout_style),
        (
            "registration",
            preview.registration.label().to_owned(),
            dim(),
        ),
        (
            "hook",
            preview
                .hook
                .as_deref()
                .map_or_else(|| "none".to_owned(), escape),
            dim(),
        ),
    ]
    .into_iter()
    .flat_map(|(name, value, style)| {
        if matches!(name, "checkout" | "registration") {
            field_lines(name, &value, style, width)
        } else {
            exact_field_lines(name, &value, style, width)
        }
    })
    .collect::<Vec<_>>();
    let mut lines = vec![Line::default()];
    let width = usize::from(width);
    for (index, action) in ADD_CHOICES.into_iter().enumerate() {
        let (hint, enabled) = match preview.eligibility(action) {
            Eligibility::Available => (String::new(), true),
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
    lines.push(choice_line(
        "Back to reference",
        "",
        true,
        choice == ADD_CHOICES.len(),
        width,
    ));
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
    let Some(choice) = ADD_CHOICES.get(form.choice).copied() else {
        form.preview = None;
        return;
    };
    let label = choice.label();
    session.message = Some(match preview.eligibility(choice) {
        Eligibility::Unavailable(reason) => format!("{label} unavailable: {reason}"),
        _ if session.batch_blocked().is_some() => {
            session.batch_blocked().unwrap_or_default().to_owned()
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

fn start_batch(session: &mut InventorySession, selection: &mut Selection, batch: Batch) {
    session.batch = Some(batch);
    selection.batch = true;
    selection.batch_cancel = false;
    selection.batch_scroll = 0;
}

fn refresh(session: &mut InventorySession) {
    if let Err((error, kept)) = session.refresh() {
        session.problems.record(
            "refresh",
            format!(
                "{} · previous configuration kept",
                escape(model::problem_reason(&error))
            ),
        );
        session.notice = Some(Notice::Warning(format!(
            "Config refresh failed: {} · previous {kept} active",
            escape(error)
        )));
    }
}

fn quit(session: &mut InventorySession) -> i32 {
    session.worker.cancel();
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
fn batch_widget(batch: &Batch, width: u16, cancel: bool) -> Paragraph<'static> {
    let active = batch.active();
    let names = batch
        .targets
        .iter()
        .map(|target| {
            if target.hidden {
                format!("{} (hidden by filter)", target.name)
            } else {
                target.name.clone()
            }
        })
        .collect::<Vec<_>>();
    let name_width = names
        .iter()
        .map(|name| Line::raw(name.clone()).width())
        .max()
        .unwrap_or(0)
        .max(11);
    let cells = |name: &str, disk: &str, config: &str| {
        let padding = name_width.saturating_sub(Line::raw(name.to_owned()).width());
        format!("{name}{} {disk:<9} {config:<13} ", " ".repeat(padding))
    };
    let mut lines = vec![Line::styled(
        format!(
            "{:<12} {}reason",
            "outcome",
            cells("repository", "disk", "config")
        ),
        dim(),
    )];
    let reason_column = 13 + Line::raw(cells("", "", "")).width();
    for (index, result) in batch.results.iter().enumerate() {
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
            Span::raw(cells(&names[index], &result.disk, &result.config)),
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
    if !batch.finished() {
        let width = usize::from(width);
        lines.extend([
            Line::default(),
            choice_line("Return to browsing", "", true, !cancel, width),
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
    let mut lines = field_lines(
        "processed",
        &format!("{} of {}", batch.processed(), batch.targets.len()),
        Style::default(),
        width,
    );
    if !left.is_empty() {
        lines.extend(field_lines(
            "unattempted",
            &left.join(", "),
            Style::default(),
            width,
        ));
    }
    let width = usize::from(width);
    lines.extend([
        Line::default(),
        choice_line("Return to inventory", "", true, !cancel, width),
        choice_line(
            "Cancel remaining work and quit",
            "completed changes remain",
            true,
            cancel,
            width,
        ),
    ]);
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// Joins `detail` with ` · `. The path in front wraps by cell, so the paragraph's word wrap only
/// breaks the prose after it.
fn path_first_lines(detail: &[String], width: u16) -> Vec<Line<'static>> {
    let mut chunks = cell_chunks(&detail[0], usize::from(width));
    let last = chunks.pop().unwrap_or_default();
    chunks
        .into_iter()
        .chain([std::iter::once(last)
            .chain(detail[1..].iter().cloned())
            .collect::<Vec<_>>()
            .join(" · ")])
        .map(|line| Line::styled(line, dim()))
        .collect()
}

/// Every frozen target before the batch runs: its exact path and identity, and whether it is
/// eligible, a no-op or blocked, with the reason.
fn preview_lines(batch: &Batch, home: &Path, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
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
        let lossy = batch.action.removes()
            && target.blocked.is_none()
            && target.found
            && !target.removal.losses.is_empty();
        if batch.action.removes() && !lossy && target.blocked.is_none() {
            detail.push(
                if target.found {
                    "clean"
                } else {
                    "known missing"
                }
                .to_owned(),
            );
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(heading));
        if lossy {
            // The losses to acknowledge come first, one per line; the identity follows.
            let identity = detail.remove(1);
            lines.extend(path_first_lines(&detail, width));
            // A loss that wraps continues under its text, so each bullet reads as one block.
            for loss in &target.removal.losses {
                let chunks = column_chunks(&escape(loss), usize::from(width).saturating_sub(4));
                lines.extend(chunks.into_iter().enumerate().map(|(index, chunk)| {
                    let bullet = if index == 0 { "  - " } else { "    " };
                    Line::styled(format!("{bullet}{chunk}"), warn())
                }));
            }
            lines.push(Line::styled(identity, dim()));
        } else {
            lines.extend(path_first_lines(&detail, width));
        }
        lines.extend(target.removal.occupies.iter().map(|identity| {
            Line::styled(
                format!(
                    "sits at the configured destination of {} · that declaration stays",
                    escape(identity)
                ),
                warn(),
            )
        }));
        lines.extend(target.removal.shared.iter().flat_map(|path| {
            path_first_lines(
                &[
                    home_relative(path, home),
                    "also unregistered · not selected, its files stay".to_owned(),
                ],
                width,
            )
            .into_iter()
            .map(|line| line.style(warn()))
        }));
    }
    lines
}

/// The preview's choices with one cursor at `focus`: the loss acknowledgment when a removal
/// lists losses, then Cancel, then the action.
fn preview_choices(batch: &Batch, focus: usize) -> Vec<Line<'static>> {
    let changing = batch
        .targets
        .iter()
        .filter(|target| target.blocked.is_none() && target.noop.is_none())
        .count();
    let hidden = batch.targets.iter().filter(|target| target.hidden).count();
    let with_hidden = |hint: &str| match (hint, hidden) {
        (hint, 0) => hint.to_owned(),
        ("", hidden) => format!("{hidden} hidden by filter"),
        (hint, hidden) => format!("{hint} · {hidden} hidden by filter"),
    };
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
            String::new(),
            focus == 0,
        ));
    }
    let cancel = lines.len();
    lines.push(choice("Cancel".to_owned(), String::new(), focus == cancel));
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
            with_hidden(if run.is_empty() {
                "nothing is eligible"
            } else {
                "cannot be undone"
            }),
            focus > cancel,
        )
    } else {
        choice(
            format!(
                "{} {changing} {}",
                batch.action.label(),
                if changing == 1 { "target" } else { "targets" }
            ),
            with_hidden(""),
            focus > cancel,
        )
    });
    lines
}

/// The preview popup's regions, its scrolling target list, the row below that list that
/// tells how much is left when it does not fit, and the choices pinned below it.
fn preview_regions(
    area: Rect,
    list: &[Line<'static>],
    choices: u16,
) -> ([Rect; 4], Rect, Option<Rect>, Rect) {
    let rows = Paragraph::new(list.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(popup_body_width(area, WIDE_POPUP));
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
    if rows > usize::from(list.height) && list.height > 1 {
        let [list, more] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(list);
        return (regions, list, Some(more), choices);
    }
    (regions, list, None, choices)
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
        model::CheckoutState::RemoteOnly => dim(),
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
    /// The furthest the preview list has been scrolled; acknowledging needs all of it seen.
    preview_seen: u16,
    add: Option<AddForm>,
    /// The open Worktrees popup; it keeps the repository it was opened on.
    worktrees: Option<worktrees::Popup>,
    /// The open GitHub dialog.
    github: Option<github::Dialog>,
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
        } else if let Some(popup) = &self.worktrees {
            popup.view()
        } else if let Some(dialog) = &self.github {
            dialog.view()
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

const HEADINGS: [&str; 5] = ["repository", "local", "branch", "status", "registration"];
const BRANCH_COLUMN: usize = 2;

/// One row's cells in the grid's column order, with their tints. The compact grid hides
/// `branch`, whose error then shows in `status`.
fn table_cells(
    row: &model::InventoryRow,
    name: String,
    home: &Path,
    compact: bool,
) -> [(String, Style); 5] {
    [
        (name, Style::default()),
        local_cell(row, home),
        branch_cell(row),
        status_cell(row, compact),
        (
            registration_text(&row.registration).to_owned(),
            registration_style(&row.registration),
        ),
    ]
}

fn local_cell(row: &model::InventoryRow, home: &Path) -> (String, Style) {
    let path = home_relative(Path::new(&row.path), home);
    match row.checkout {
        model::CheckoutState::Cloned => (path, Style::default()),
        model::CheckoutState::Missing => ("missing".to_owned(), warn()),
        model::CheckoutState::RemoteOnly => ("remote only".to_owned(), dim()),
        ref other => (
            format!("{path} · {}", checkout_text(other)),
            checkout_style(other),
        ),
    }
}

fn branch_cell(row: &model::InventoryRow) -> (String, Style) {
    let style = match row.branch {
        model::ObservationField::Error(_) => tint(Color::Red),
        _ => Style::default(),
    };
    (observation_text(&row.branch), style)
}

fn status_cell(row: &model::InventoryRow, compact: bool) -> (String, Style) {
    if compact && let model::ObservationField::Error(_) = row.branch {
        return branch_cell(row);
    }
    (
        observation_text(&row.changes),
        observation_style(&row.changes),
    )
}

fn repository_table(
    rows: &[&model::InventoryRow],
    names: &HashMap<model::RowKey, String>,
    marks: &Marks,
    hosted: Option<&HashSet<PathBuf>>,
    home: &Path,
    width: u16,
    selected: Option<usize>,
) -> Table<'static> {
    let compact = width < WIDE_TABLE;
    let mut headings = HEADINGS.to_vec();
    // Column widths, padding included.
    let (mut widths, flexible): (Vec<u16>, &[usize]) = if compact {
        headings.remove(BRANCH_COLUMN);
        (vec![23, 18, 12, 14], &[1])
    } else {
        (vec![22, 27, 18, 12, 14], &[1])
    };
    let session_slot = if hosted.is_some() { SESSION_SLOT } else { 0 };
    let slot = |column: usize| {
        if column == 0 {
            MARK_SLOT + session_slot
        } else {
            0
        }
    };
    let cells = rows
        .iter()
        .map(|row| {
            let mut cells = table_cells(row, names[&row.key].clone(), home, compact).to_vec();
            if compact {
                cells.remove(BRANCH_COLUMN);
            }
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
                        // The local cell keeps its tail: it tells duplicate checkouts apart.
                        let room = usize::from(widths[column]).saturating_sub(2 + slot(column));
                        let text = if column == 0 {
                            abbreviate_name(&value, room)
                        } else {
                            abbreviate(&value, room, column == 1)
                        };
                        // Reverse video carries the selection, so the selected row drops its tints.
                        let style = if selected == Some(index) {
                            Style::default()
                        } else {
                            style
                        };
                        let mark = match column {
                            0 if marks.contains(&rows[index].key) => "[x] ",
                            0 => "[ ] ",
                            _ => "",
                        };
                        if let (0, Some(hosted)) = (column, hosted) {
                            let open = rows[index]
                                .observed_path
                                .as_ref()
                                .is_some_and(|path| hosted.contains(path));
                            let dot = match (open, selected == Some(index)) {
                                (false, _) => Span::raw("  "),
                                (true, true) => Span::styled("● ", style),
                                (true, false) => Span::styled("● ", focus()),
                            };
                            return Cell::from(Line::from(vec![
                                Span::styled(format!(" {mark}"), style),
                                dot,
                                Span::styled(text, style),
                            ]));
                        }
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
            scan_revision: String::new(),
            complete: false,
            problems: Default::default(),
            marks: Default::default(),
            bindings: Bindings::load(Default::default()).expect("default bindings are valid"),
            notice: None,
            message: None,
            cached_report: None,
            batch: None,
            remote: super::RemoteDiscovery::new(
                super::ConfiguredProviders::default(),
                Vec::new(),
                super::RemoteBounds::default(),
            ),
            remote_rows: Vec::new(),
            remote_cached: 0,
            lookups: Default::default(),
            lookup_results: std::sync::mpsc::channel(),
            github: super::github::Remote::default(),
            session_layout: None,
            hosted: None,
            hosted_rows: Default::default(),
            host_answers: std::sync::mpsc::channel(),
            opening: None,
            host_notice: None,
            farewell: None,
            listing: 0,
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
            origin_fact: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: Some(key.to_owned()),
            warnings: Vec::new(),
            archived: None,
            remote_status: None,
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
            origin_fact: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: Some(name.to_owned()),
            warnings: Vec::new(),
            archived: None,
            remote_status: None,
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
            origin_fact: None,
            branch: ObservationField::NotApplicable,
            changes: ObservationField::NotApplicable,
            configured_destination: None,
            observed_path: None,
            declaration: None,
            warnings: Vec::new(),
            archived: None,
            remote_status: None,
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
            display_row(RowKey::Path("/r/x/org/proj".into()), CheckoutState::Cloned),
            checkout("github.com/org/proj", "/r/hub/proj"),
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
                "x/org/proj",
                "github.com/org/proj",
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
    fn column_chunks_wrap_on_spaces_and_cut_only_what_never_fits() {
        let chunks = super::column_chunks(
            "commits not on any remote: branch feature-1, branch feature-10, branch feature-11",
            36,
        );
        assert_eq!(
            chunks,
            [
                "commits not on any remote: branch",
                "feature-1, branch feature-10, branch",
                "feature-11"
            ]
        );
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 36));
        assert_eq!(
            super::column_chunks("see /very/long/path/that/never/fits ok", 10),
            ["see", "/very/long", "/path/that", "/never/fit", "s ok"]
        );
    }

    #[test]
    fn a_revision_that_fails_to_load_is_the_revision_already_tried() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let path = temp.path().join("config.toml");
        std::fs::create_dir_all(home.join("repos")).unwrap();
        std::fs::write(&path, "root = \"repos\"\n").unwrap();
        let (config, ..) = crate::infrastructure::config::load_inventory(&path).unwrap();
        let root = config.resolve_root(&home).unwrap();
        let mut session = InventorySession::new(config, path.clone(), home, root);
        std::fs::write(&path, "root = [\n").unwrap();
        assert!(session.refresh().is_err());
        assert_eq!(
            session.scan_revision,
            LocalInventoryFilesystem.config_revision(&path)
        );
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
