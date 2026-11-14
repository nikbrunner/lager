//! Remote discovery for the inventory: which providers are loading, ready or failed, what they
//! last returned, and which response still belongs to a current request.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use crate::application::ports::{ProviderAnswer, ProviderFailure, ProviderSource};
use crate::domain::config::Config;
use crate::domain::repository::{RepositoryRef, RepositorySummary, validate_reference_safety};

/// How long a provider request may run, and how long ending the requests may hold the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteBounds {
    pub request: Duration,
    pub cancellation: Duration,
}

impl Default for RemoteBounds {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(30),
            cancellation: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderPhase {
    /// No request ran in this session, or it was cancelled before it answered.
    Idle,
    Loading {
        since: Instant,
    },
    Ready,
    Failed(String),
}

/// What the provider last returned, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCache {
    pub members: Vec<RepositorySummary>,
    pub fetched: SystemTime,
}

#[derive(Debug)]
pub struct ProviderState {
    pub phase: ProviderPhase,
    /// The last successful answer of this session. It outlives failures, refreshes and
    /// disabling remote discovery.
    pub cached: Option<ProviderCache>,
    /// A reloaded configuration changed what this provider's last request was built from.
    pub config_changed: bool,
    pending: Option<Pending>,
}

impl ProviderState {
    /// The cached members are older than the request that is running or failed.
    pub fn stale(&self) -> bool {
        self.cached.is_some() && self.phase != ProviderPhase::Ready
    }
}

#[derive(Debug)]
struct Pending {
    request: u64,
    cancelled: Arc<AtomicBool>,
}

struct Response {
    host: String,
    request: u64,
    answer: ProviderAnswer,
}

/// The wildcard declarations of the configuration; their members join the provider catalogs.
pub fn wildcards(config: &Config) -> Vec<RepositoryRef> {
    config
        .repositories
        .iter()
        .filter_map(|declaration| RepositoryRef::parse(&declaration.url).ok())
        .filter(RepositoryRef::is_wildcard)
        .collect()
}

fn host_wildcards(wildcards: &[RepositoryRef], host: &str) -> Vec<RepositoryRef> {
    wildcards
        .iter()
        .filter(|wildcard| wildcard.host.eq_ignore_ascii_case(host))
        .cloned()
        .collect()
}

/// Fetches provider catalogs on background threads and keeps the session's answers. Fetching
/// only reads from providers: it never registers, clones or runs hooks.
pub struct RemoteDiscovery<S: ProviderSource> {
    source: Arc<S>,
    wildcards: Vec<RepositoryRef>,
    bounds: RemoteBounds,
    enabled: bool,
    include_archived: bool,
    providers: BTreeMap<String, ProviderState>,
    next_request: u64,
    /// Requests whose thread has not answered yet, current or not.
    running: usize,
    sender: mpsc::Sender<Response>,
    receiver: mpsc::Receiver<Response>,
}

impl<S: ProviderSource> RemoteDiscovery<S> {
    pub fn new(source: S, wildcards: Vec<RepositoryRef>, bounds: RemoteBounds) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut discovery = Self {
            source: Arc::new(source),
            wildcards: Vec::new(),
            bounds,
            enabled: false,
            include_archived: false,
            providers: BTreeMap::new(),
            next_request: 0,
            running: 0,
            sender,
            receiver,
        };
        discovery.install(wildcards);
        discovery
    }

    /// Takes the providers of a reloaded configuration without a request. Answers of providers
    /// that are still configured stay. A provider whose settings or wildcards changed drops its
    /// running request, and its cached answer turns stale.
    pub fn reconfigure(&mut self, source: S, wildcards: Vec<RepositoryRef>) {
        for (host, provider) in &mut self.providers {
            if source.settings(host) != self.source.settings(host)
                || host_wildcards(&wildcards, host) != host_wildcards(&self.wildcards, host)
            {
                if let Some(pending) = provider.pending.take() {
                    pending.cancelled.store(true, Ordering::Relaxed);
                }
                provider.phase = ProviderPhase::Idle;
                provider.config_changed = true;
            }
        }
        self.source = Arc::new(source);
        self.install(wildcards);
    }

    fn install(&mut self, wildcards: Vec<RepositoryRef>) {
        self.wildcards = wildcards;
        let hosts = self.source.hosts();
        self.providers.retain(|host, provider| {
            let kept = hosts.contains(host);
            if !kept && let Some(pending) = provider.pending.take() {
                pending.cancelled.store(true, Ordering::Relaxed);
            }
            kept
        });
        for host in hosts {
            self.providers.entry(host).or_insert(ProviderState {
                phase: ProviderPhase::Idle,
                cached: None,
                config_changed: false,
                pending: None,
            });
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn include_archived(&self) -> bool {
        self.include_archived
    }

    pub fn providers(&self) -> &BTreeMap<String, ProviderState> {
        &self.providers
    }

    pub fn loading(&self) -> bool {
        self.providers
            .values()
            .any(|provider| provider.pending.is_some())
    }

    /// The providers whose last request failed.
    pub fn failed(&self) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|(_, provider)| matches!(provider.phase, ProviderPhase::Failed(_)))
            .map(|(host, _)| host.as_str())
            .collect()
    }

    /// Every cached member while remote discovery is enabled, stale ones included.
    pub fn members(&self) -> Vec<RepositorySummary> {
        if !self.enabled {
            return Vec::new();
        }
        self.providers
            .values()
            .filter_map(|provider| provider.cached.as_ref())
            .flat_map(|cache| cache.members.iter().cloned())
            .collect()
    }

    /// Fetches every provider. Remote discovery stays enabled until `disable`.
    pub fn enable(&mut self) {
        self.enabled = true;
        self.fetch_all();
    }

    /// Invalidates every running request. The cache and the archive preference stay.
    pub fn disable(&mut self) {
        self.enabled = false;
        for provider in self.providers.values_mut() {
            if let Some(pending) = provider.pending.take() {
                pending.cancelled.store(true, Ordering::Relaxed);
                provider.phase = ProviderPhase::Idle;
            }
        }
    }

    /// Fetches every provider again; cached members stay, stale, until the answers arrive.
    pub fn refresh(&mut self) {
        if self.enabled {
            self.fetch_all();
        }
    }

    /// Fetches the failed providers again and leaves every other request alone.
    pub fn retry(&mut self) {
        if !self.enabled {
            return;
        }
        let failed = self
            .failed()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for host in failed {
            self.fetch(&host);
        }
    }

    /// Stores the archive preference. While enabled, a change invalidates the running requests
    /// and fetches every provider again.
    pub fn set_include_archived(&mut self, include_archived: bool) {
        let changed = self.include_archived != include_archived;
        self.include_archived = include_archived;
        if changed && self.enabled {
            self.fetch_all();
        }
    }

    fn fetch_all(&mut self) {
        let hosts = self.providers.keys().cloned().collect::<Vec<_>>();
        for host in hosts {
            self.fetch(&host);
        }
    }

    fn fetch(&mut self, host: &str) {
        let Some(provider) = self.providers.get_mut(host) else {
            return;
        };
        if let Some(previous) = provider.pending.take() {
            previous.cancelled.store(true, Ordering::Relaxed);
        }
        let request = self.next_request;
        self.next_request += 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let since = Instant::now();
        provider.phase = ProviderPhase::Loading { since };
        provider.config_changed = false;
        provider.pending = Some(Pending {
            request,
            cancelled: Arc::clone(&cancelled),
        });
        let source = Arc::clone(&self.source);
        let sender = self.sender.clone();
        let wildcards = host_wildcards(&self.wildcards, host);
        let include_archived = self.include_archived;
        let host = host.to_owned();
        self.running += 1;
        let bound = self.bounds.request;
        // The request runs on a thread of its own, so its bound holds whether or not anything
        // polls: at the bound it is cancelled and answered as failed, and what it returns
        // later has no receiver.
        std::thread::spawn(move || {
            let (finished, answered) = mpsc::channel();
            let (request_host, flag) = (host.clone(), Arc::clone(&cancelled));
            std::thread::spawn(move || {
                let _ =
                    finished.send(source.fetch(&request_host, &wildcards, include_archived, &flag));
            });
            let answer = answered.recv_timeout(bound).unwrap_or_else(|_| {
                cancelled.store(true, Ordering::Relaxed);
                ProviderAnswer {
                    members: Vec::new(),
                    error: Some(format!("no response within {} s", bound.as_secs())),
                }
            });
            let _ = sender.send(Response {
                host,
                request,
                answer,
            });
        });
    }

    /// Applies the answers that arrived, a request's failure at its bound among them. An answer
    /// to a request that was invalidated since is dropped. Returns whether any provider changed,
    /// and the failures to report.
    pub fn poll(&mut self) -> (bool, Vec<ProviderFailure>) {
        let mut changed = false;
        let mut failures = Vec::new();
        while let Ok(response) = self.receiver.try_recv() {
            self.running -= 1;
            changed |= self.accept(response, &mut failures);
        }
        (changed, failures)
    }

    fn accept(&mut self, response: Response, failures: &mut Vec<ProviderFailure>) -> bool {
        let Some(provider) = self.providers.get_mut(&response.host) else {
            return false;
        };
        if provider
            .pending
            .take_if(|pending| pending.request == response.request)
            .is_none()
        {
            return false;
        }
        let mut fail = |error: String| {
            failures.push(ProviderFailure {
                provider: response.host.clone(),
                error,
            })
        };
        let ProviderAnswer { mut members, error } = response.answer;
        let listed = members.len();
        // A clone URL carrying credentials, a query or a fragment is never shown or used.
        members.retain(|member| validate_reference_safety(&member.reference.clone_url).is_ok());
        match listed - members.len() {
            0 => {}
            1 => fail("1 repository skipped: unsafe clone URL".to_owned()),
            skipped => fail(format!("{skipped} repositories skipped: unsafe clone URL")),
        }
        // What a partly failed request did return replaces the cache; a request that returned
        // nothing keeps the members it could not replace.
        if error.is_none() || !members.is_empty() {
            provider.cached = Some(ProviderCache {
                members,
                fetched: SystemTime::now(),
            });
        }
        provider.phase = match error {
            Some(error) => {
                fail(error.clone());
                ProviderPhase::Failed(error)
            }
            None => ProviderPhase::Ready,
        };
        true
    }

    /// Whether a request thread has not answered yet, current or not.
    pub fn busy(&mut self) -> bool {
        while self.running > 0 && self.receiver.try_recv().is_ok() {
            self.running -= 1;
        }
        self.running > 0
    }

    /// Cancels every request and waits for the request threads, at most the cancellation bound.
    pub fn shutdown(&mut self) {
        self.shutdown_by(Instant::now() + self.bounds.cancellation);
    }

    /// Cancels every request and waits for the request threads until `deadline`. Threads that
    /// have not answered by then are no longer waited for.
    pub fn shutdown_by(&mut self, deadline: Instant) {
        self.disable();
        while self.running > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if self.receiver.recv_timeout(left).is_err() {
                break;
            }
            self.running -= 1;
        }
        self.running = 0;
    }
}

impl<S: ProviderSource> Drop for RemoteDiscovery<S> {
    fn drop(&mut self) {
        // An interruptible request, such as a provider CLI, is ended before the process exits.
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex};

    use super::*;

    /// A provider source whose answers the test releases one by one.
    #[derive(Default)]
    struct Gated {
        hosts: Vec<String>,
        /// Requests in arrival order: host and archive preference.
        requests: Mutex<Vec<(String, bool)>>,
        /// Answers waiting per request index.
        answers: Mutex<BTreeMap<usize, ProviderAnswer>>,
        released: Condvar,
        cancellations: Mutex<Vec<usize>>,
        settings: String,
    }

    #[derive(Clone)]
    struct Stub(Arc<Gated>);

    impl Stub {
        fn new(hosts: &[&str]) -> Self {
            Self(Arc::new(Gated {
                hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                ..Gated::default()
            }))
        }

        fn requests(&self) -> Vec<(String, bool)> {
            self.0.requests.lock().unwrap().clone()
        }

        fn answer(&self, request: usize, result: Result<Vec<RepositorySummary>, String>) {
            let answer = match result {
                Ok(members) => ProviderAnswer {
                    members,
                    error: None,
                },
                Err(error) => ProviderAnswer {
                    members: Vec::new(),
                    error: Some(error),
                },
            };
            self.answer_with(request, answer);
        }

        fn answer_with(&self, request: usize, answer: ProviderAnswer) {
            self.0.answers.lock().unwrap().insert(request, answer);
            self.0.released.notify_all();
        }

        /// Answers the provider's request number `nth`; requests of two providers that start
        /// together arrive in either order.
        fn answer_host(
            &self,
            host: &str,
            nth: usize,
            result: Result<Vec<RepositorySummary>, String>,
        ) {
            let request = self
                .requests()
                .iter()
                .enumerate()
                .filter(|(_, (requested, _))| requested == host)
                .nth(nth)
                .map(|(index, _)| index)
                .expect("the request was made");
            self.answer(request, result);
        }

        fn wait_for_requests(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.requests().len() < count {
                assert!(Instant::now() < deadline, "{:?}", self.requests());
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl ProviderSource for Stub {
        fn hosts(&self) -> Vec<String> {
            self.0.hosts.clone()
        }

        fn settings(&self, _host: &str) -> String {
            self.0.settings.clone()
        }

        fn fetch(
            &self,
            host: &str,
            _: &[RepositoryRef],
            include_archived: bool,
            cancelled: &AtomicBool,
        ) -> ProviderAnswer {
            let index = {
                let mut requests = self.0.requests.lock().unwrap();
                requests.push((host.to_owned(), include_archived));
                requests.len() - 1
            };
            let mut answers = self.0.answers.lock().unwrap();
            loop {
                if let Some(answer) = answers.remove(&index) {
                    return answer;
                }
                if cancelled.load(Ordering::Relaxed) {
                    let mut cancellations = self.0.cancellations.lock().unwrap();
                    if !cancellations.contains(&index) {
                        cancellations.push(index);
                    }
                }
                answers = self
                    .0
                    .released
                    .wait_timeout(answers, Duration::from_millis(2))
                    .unwrap()
                    .0;
            }
        }
    }

    fn member(identity: &str) -> RepositorySummary {
        RepositorySummary {
            reference: RepositoryRef::parse(identity).unwrap(),
            archived: false,
        }
    }

    fn identities(discovery: &RemoteDiscovery<Stub>) -> Vec<String> {
        discovery
            .members()
            .iter()
            .map(|member| member.reference.identity())
            .collect()
    }

    /// Polls until the provider left `Loading`, and returns every failure reported on the way.
    fn settle(discovery: &mut RemoteDiscovery<Stub>, hosts: &[&str]) -> Vec<ProviderFailure> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut failures = Vec::new();
        loop {
            failures.extend(discovery.poll().1);
            if hosts.iter().all(|host| {
                !matches!(
                    discovery.providers()[*host].phase,
                    ProviderPhase::Loading { .. }
                )
            }) {
                return failures;
            }
            assert!(Instant::now() < deadline, "providers did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The stub never ends a request on its own, so dropping waits out the cancellation bound.
    fn discovery(stub: &Stub) -> RemoteDiscovery<Stub> {
        RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_secs(30),
                cancellation: Duration::from_millis(20),
            },
        )
    }

    #[test]
    fn enable_fetches_every_provider_and_keeps_partial_results_beside_a_failure() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        assert!(!remote.enabled() && remote.members().is_empty());
        remote.enable();
        stub.wait_for_requests(2);
        assert!(remote.loading());
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert!(matches!(
            remote.providers()["b.example"].phase,
            ProviderPhase::Loading { .. }
        ));
        stub.answer_host("b.example", 0, Err("refused".to_owned()));
        let failures = settle(&mut remote, &["b.example"]);
        assert_eq!(
            failures,
            [ProviderFailure {
                provider: "b.example".to_owned(),
                error: "refused".to_owned()
            }]
        );
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert_eq!(remote.failed(), ["b.example"]);
        assert!(remote.poll().1.is_empty(), "a failure is reported once");
    }

    #[test]
    fn retry_requests_only_the_failed_provider_and_refresh_keeps_stale_members() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(2);
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        stub.answer_host("b.example", 0, Err("refused".to_owned()));
        settle(&mut remote, &["a.example", "b.example"]);

        remote.retry();
        stub.wait_for_requests(3);
        assert_eq!(stub.requests()[2].0, "b.example");
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        stub.answer(2, Ok(vec![member("b.example/org/two")]));
        settle(&mut remote, &["b.example"]);
        assert!(remote.failed().is_empty());
        remote.retry();
        assert_eq!(stub.requests().len(), 3, "nothing failed, nothing to retry");

        remote.refresh();
        stub.wait_for_requests(5);
        assert!(remote.providers()["a.example"].stale());
        assert_eq!(
            identities(&remote),
            ["a.example/org/one", "b.example/org/two"]
        );
        // A failed refresh keeps the members it could not replace.
        stub.answer_host("a.example", 1, Err("refused".to_owned()));
        stub.answer_host("b.example", 2, Ok(Vec::new()));
        settle(&mut remote, &["a.example", "b.example"]);
        assert!(remote.providers()["a.example"].stale());
        assert!(!remote.providers()["b.example"].stale());
        assert_eq!(identities(&remote), ["a.example/org/one"]);
    }

    #[test]
    fn answers_to_invalidated_requests_are_dropped() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);

        // Disabling invalidates the request; its answer changes nothing.
        remote.disable();
        stub.answer(0, Ok(vec![member("a.example/org/late")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Idle);
        assert!(remote.providers()["a.example"].cached.is_none());

        // An archive toggle supersedes the request that ran with the other preference.
        remote.enable();
        stub.wait_for_requests(2);
        remote.set_include_archived(true);
        stub.wait_for_requests(3);
        assert_eq!(
            stub.requests()[1..],
            [
                ("a.example".to_owned(), false),
                ("a.example".to_owned(), true)
            ]
        );
        stub.answer(1, Ok(vec![member("a.example/org/without")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert!(remote.loading());
        stub.answer(2, Ok(vec![member("a.example/org/with")]));
        settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/with"]);

        // So does a refresh.
        remote.refresh();
        stub.wait_for_requests(4);
        remote.refresh();
        stub.wait_for_requests(5);
        stub.answer(3, Ok(vec![member("a.example/org/first")]));
        stub.answer(4, Ok(vec![member("a.example/org/second")]));
        settle(&mut remote, &["a.example"]);
        std::thread::sleep(Duration::from_millis(20));
        remote.poll();
        assert_eq!(identities(&remote), ["a.example/org/second"]);
    }

    #[test]
    fn disabling_keeps_the_cache_and_the_archive_preference_and_requests_nothing() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.set_include_archived(true);
        assert!(
            stub.requests().is_empty(),
            "the preference alone fetches nothing"
        );
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer(0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);

        remote.disable();
        assert!(remote.members().is_empty());
        assert!(remote.include_archived());
        assert!(remote.providers()["a.example"].cached.is_some());
        remote.refresh();
        remote.retry();
        remote.set_include_archived(false);
        remote.reconfigure(stub.clone(), Vec::new());
        assert_eq!(stub.requests().len(), 1);

        // Enabling again shows the cached members, stale, while the providers load.
        remote.enable();
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert!(remote.providers()["a.example"].stale());
    }

    #[test]
    fn a_request_past_its_bound_fails_is_cancelled_and_its_late_answer_is_dropped() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_millis(30),
                cancellation: Duration::from_millis(30),
            },
        );
        remote.enable();
        stub.wait_for_requests(1);
        // The bound holds while nothing polls: the request is cancelled on its own.
        let deadline = Instant::now() + Duration::from_secs(10);
        while stub.0.cancellations.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the request was not cancelled");
            std::thread::sleep(Duration::from_millis(1));
        }
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "no response within 0 s");
        stub.answer(0, Ok(vec![member("a.example/org/late")]));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remote.poll(), (false, Vec::new()));
        assert_eq!(remote.failed(), ["a.example"]);
    }

    #[test]
    fn shutdown_waits_for_a_stalled_request_no_longer_than_the_cancellation_bound() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds {
                request: Duration::from_secs(30),
                cancellation: Duration::from_millis(50),
            },
        );
        remote.enable();
        stub.wait_for_requests(1);
        let started = Instant::now();
        drop(remote);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(50), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
        assert_eq!(*stub.0.cancellations.lock().unwrap(), [0]);
        stub.answer(0, Ok(Vec::new()));

        // Without a running request there is nothing to wait for.
        let started = Instant::now();
        drop(RemoteDiscovery::new(
            stub.clone(),
            Vec::new(),
            RemoteBounds::default(),
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn members_with_unsafe_clone_urls_are_skipped_and_reported_without_their_value() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        let mut unsafe_member = member("a.example/org/leak");
        unsafe_member.reference.clone_url = "https://user:secret@a.example/org/leak.git".to_owned();
        stub.answer(0, Ok(vec![member("a.example/org/one"), unsafe_member]));
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "1 repository skipped: unsafe clone URL");
    }

    #[test]
    fn a_partly_failed_answer_keeps_its_members_and_leaves_the_provider_failed_for_retry() {
        let stub = Stub::new(&["a.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer_with(
            0,
            ProviderAnswer {
                members: vec![member("a.example/me/own")],
                error: Some("a.example/org/*: refused".to_owned()),
            },
        );
        let failures = settle(&mut remote, &["a.example"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error, "a.example/org/*: refused");
        assert_eq!(identities(&remote), ["a.example/me/own"]);
        assert_eq!(remote.failed(), ["a.example"]);

        remote.retry();
        stub.wait_for_requests(2);
        stub.answer(
            1,
            Ok(vec![
                member("a.example/me/own"),
                member("a.example/org/one"),
            ]),
        );
        settle(&mut remote, &["a.example"]);
        assert!(remote.failed().is_empty());
        assert_eq!(
            identities(&remote),
            ["a.example/me/own", "a.example/org/one"]
        );
    }

    #[test]
    fn a_reloaded_configuration_keeps_known_providers_and_drops_removed_ones() {
        let stub = Stub::new(&["a.example", "b.example"]);
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(2);
        stub.answer_host("a.example", 0, Ok(vec![member("a.example/org/one")]));
        stub.answer_host("b.example", 0, Ok(vec![member("b.example/org/two")]));
        settle(&mut remote, &["a.example", "b.example"]);

        let next = Stub::new(&["b.example", "c.example"]);
        remote.reconfigure(next.clone(), Vec::new());
        assert_eq!(identities(&remote), ["b.example/org/two"]);
        assert_eq!(remote.providers()["c.example"].phase, ProviderPhase::Idle);
        assert!(next.requests().is_empty() && stub.requests().len() == 2);
    }

    #[test]
    fn a_reloaded_configuration_turns_an_answer_stale_when_its_provider_settings_changed() {
        let settled = |settings: &str| {
            Stub(Arc::new(Gated {
                hosts: vec!["a.example".to_owned()],
                settings: settings.to_owned(),
                ..Gated::default()
            }))
        };
        let stub = settled("api one");
        let mut remote = discovery(&stub);
        remote.enable();
        stub.wait_for_requests(1);
        stub.answer(0, Ok(vec![member("a.example/org/one")]));
        settle(&mut remote, &["a.example"]);

        remote.reconfigure(settled("api one"), Vec::new());
        assert_eq!(remote.providers()["a.example"].phase, ProviderPhase::Ready);
        remote.reconfigure(settled("api two"), Vec::new());
        let provider = &remote.providers()["a.example"];
        assert_eq!(provider.phase, ProviderPhase::Idle);
        assert!(provider.stale() && provider.config_changed);
        assert_eq!(identities(&remote), ["a.example/org/one"]);
    }
}
