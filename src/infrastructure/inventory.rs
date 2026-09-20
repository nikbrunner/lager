use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::application::inventory::{
    CheckoutState, InventoryFilesystem, LocalCheckout, LocalScan, ObservationField, OriginFact,
    ScanDiagnostic,
};
use crate::domain::repository::normalize_remote;

pub struct LocalInventoryFilesystem;

impl InventoryFilesystem for LocalInventoryFilesystem {
    fn destination_state(&self, destination: &Path) -> CheckoutState {
        match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => CheckoutState::Missing,
            Ok(metadata) if metadata.file_type().is_symlink() => CheckoutState::Unreadable,
            Ok(metadata) if metadata.is_dir() => match std::fs::read_dir(destination) {
                Ok(_) => CheckoutState::Unknown,
                Err(_) => CheckoutState::Unreadable,
            },
            Ok(_) | Err(_) => CheckoutState::Unreadable,
        }
    }

    fn config_revision(&self, path: &Path) -> String {
        match std::fs::metadata(path).and_then(|metadata| {
            let modified = metadata.modified()?;
            let elapsed = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            Ok((metadata.len(), elapsed.as_nanos()))
        }) {
            Ok((length, nanos)) => format!("{}:{length}:{nanos}", path.to_string_lossy()),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }
}

pub struct ScanHandle {
    pub receiver: mpsc::Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ScanHandle {
    #[cfg(test)]
    pub(crate) fn for_test(receiver: mpsc::Receiver<ScanEvent>) -> Self {
        Self {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancel_and_join(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        // The worker owns every probe worker and waits for their children to be reaped. This
        // makes dropping a session a real cancellation boundary rather than stale-result hiding.
        self.cancel_and_join();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanContext {
    pub generation: u64,
    pub config_revision: String,
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSnapshot {
    pub path: PathBuf,
    receiver_token: Option<String>,
    content_token: Option<u64>,
}

impl TargetSnapshot {
    fn capture(path: PathBuf) -> Self {
        Self {
            receiver_token: target_metadata_token(&path),
            content_token: config_contents_token(&path),
            path,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_for_test(path: PathBuf) -> Self {
        Self::capture(path)
    }

    /// This runs on the terminal event owner, so it is limited to metadata identity checks.
    pub fn matches_current(&self) -> bool {
        self.receiver_token == target_metadata_token(&self.path)
    }

    /// Probe workers verify config contents immediately before publishing their observations.
    fn contents_match_current(&self) -> bool {
        self.content_token == config_contents_token(&self.path)
    }

    #[cfg(test)]
    pub(crate) fn contents_match_current_for_test(&self) -> bool {
        self.contents_match_current()
    }
}

pub enum ScanEvent {
    Failure,
    Discovered {
        path: PathBuf,
        context: ScanContext,
        target: TargetSnapshot,
    },
    Checkout {
        checkout: LocalCheckout,
        context: ScanContext,
        target: TargetSnapshot,
        content_current: bool,
    },
    Diagnostic {
        diagnostic: ScanDiagnostic,
        context: ScanContext,
    },
    Complete {
        incomplete: bool,
        context: ScanContext,
    },
}

fn target_metadata_token(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let git_path = path.join(".git");
    let git = std::fs::symlink_metadata(&git_path).ok();
    let config = std::fs::symlink_metadata(git_path.join("config")).ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Git commands can update a .git directory's timestamps while probing. Its device/inode
        // identifies a replacement without rejecting the probe's own harmless bookkeeping.
        let identity = |entry: Option<std::fs::Metadata>| {
            entry
                .map(|entry| format!("{}:{}", entry.dev(), entry.ino()))
                .unwrap_or_default()
        };
        let config_identity = config
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}:{}:{}:{}",
                    entry.dev(),
                    entry.ino(),
                    entry.len(),
                    entry.mtime(),
                    entry.mtime_nsec(),
                    entry.ctime(),
                    entry.ctime_nsec()
                )
            })
            .unwrap_or_default();
        Some(format!(
            "{}:{}:{config_identity}",
            identity(Some(metadata)),
            identity(git),
        ))
    }
    #[cfg(not(unix))]
    {
        Some(format!(
            "{:?}:{:?}:{:?}",
            metadata.modified().ok(),
            git.and_then(|marker| marker.modified().ok()),
            config.and_then(|entry| entry.modified().ok())
        ))
    }
}

fn config_contents_token(path: &Path) -> Option<u64> {
    let contents = std::fs::read(path.join(".git/config")).ok()?;
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    Some(hasher.finish())
}

const MAX_CONCURRENT_PROBES: usize = 4;

/// Runs traversal and Git observations away from the terminal owner. Discovered paths are sent
/// to bounded probe workers as traversal finds them, so slow later paths cannot delay early rows.
pub fn start_local_scan(root: PathBuf) -> ScanHandle {
    start_local_scan_with_context(ScanContext {
        generation: 1,
        config_revision: root.to_string_lossy().into_owned(),
        root,
    })
}

pub fn start_local_scan_with_context(context: ScanContext) -> ScanHandle {
    let root = context.root.clone();
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker = std::thread::spawn(move || {
        let (paths, path_receiver) = mpsc::channel::<TargetSnapshot>();
        let path_receiver = Arc::new(Mutex::new(path_receiver));
        let mut probes = Vec::new();
        for _ in 0..MAX_CONCURRENT_PROBES {
            let sender = sender.clone();
            let context = context.clone();
            let cancel = Arc::clone(&worker_cancel);
            let path_receiver = Arc::clone(&path_receiver);
            probes.push(std::thread::spawn(move || {
                while !cancel.load(Ordering::Relaxed) {
                    let next = path_receiver
                        .lock()
                        .expect("probe queue lock poisoned")
                        .recv_timeout(std::time::Duration::from_millis(20));
                    let path = match next {
                        Ok(path) => path,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(checkout) = inspect_checkout(&path.path, &cancel, || {
                        let _ = sender.send(ScanEvent::Failure);
                    }) else {
                        continue;
                    };
                    let content_current = path.contents_match_current();
                    let _ = sender.send(ScanEvent::Checkout {
                        checkout,
                        context: context.clone(),
                        target: path,
                        content_current,
                    });
                }
            }));
        }
        let traversal_sender = sender.clone();
        let traversal_context = context.clone();
        let incomplete = Arc::new(AtomicBool::new(false));
        let traversal_incomplete = Arc::clone(&incomplete);
        discover_checkout_paths(
            &root,
            &worker_cancel,
            |path| {
                let target = TargetSnapshot::capture(path);
                traversal_sender
                    .send(ScanEvent::Discovered {
                        path: target.path.clone(),
                        context: traversal_context.clone(),
                        target: target.clone(),
                    })
                    .is_ok()
                    && paths.send(target).is_ok()
            },
            |diagnostic| {
                if diagnostic.incomplete {
                    traversal_incomplete.store(true, Ordering::Relaxed);
                }
                traversal_sender
                    .send(ScanEvent::Diagnostic {
                        diagnostic,
                        context: traversal_context.clone(),
                    })
                    .is_ok()
            },
        );
        drop(paths);
        for probe in probes {
            let _ = probe.join();
        }
        let _ = sender.send(ScanEvent::Complete {
            incomplete: incomplete.load(Ordering::Relaxed),
            context,
        });
    });
    ScanHandle {
        receiver,
        cancel,
        worker: Some(worker),
    }
}

pub fn scan_local_repositories(root: &Path) -> LocalScan {
    let mut scan = LocalScan::default();
    let handle = start_local_scan(root.to_path_buf());
    while let Ok(event) = handle.receiver.recv() {
        match event {
            ScanEvent::Failure => {}
            ScanEvent::Discovered { path, .. } => scan.pending_paths.push(path),
            ScanEvent::Checkout { checkout, .. } => scan.checkouts.push(checkout),
            ScanEvent::Diagnostic { diagnostic, .. } => {
                scan.incomplete |= diagnostic.incomplete;
                scan.diagnostics.push(diagnostic);
            }
            ScanEvent::Complete { .. } => break,
        }
    }
    scan.complete = true;
    scan.checkouts
        .sort_by(|left, right| left.path.cmp(&right.path));
    scan.diagnostics
        .sort_by(|left, right| left.path.cmp(&right.path));
    scan
}

fn discover_checkout_paths(
    root: &Path,
    cancel: &AtomicBool,
    mut emit_path: impl FnMut(PathBuf) -> bool,
    mut emit_diagnostic: impl FnMut(ScanDiagnostic) -> bool,
) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if !emit_diagnostic(diagnostic(path, format!("unreadable path: {error}"))) {
                    break;
                }
                continue;
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        if path.join("HEAD").is_file()
            && path.join("objects").is_dir()
            && path.join("refs").is_dir()
        {
            match git_dir_output(&path, ["rev-parse", "--is-bare-repository"], cancel) {
                Ok(value) if value.trim() == "true" => {
                    if !emit_diagnostic(exclusion(path, "bare repository ignored")) {
                        break;
                    }
                    continue;
                }
                Err(ProbeError::Cancelled) => break,
                Err(error) => {
                    if !emit_diagnostic(diagnostic(
                        path,
                        format!("could not inspect bare repository: {error}"),
                    )) {
                        break;
                    }
                    continue;
                }
                Ok(_) => {}
            }
        }
        let git_marker = path.join(".git");
        if let Ok(marker) = std::fs::symlink_metadata(&git_marker) {
            if marker.is_dir() {
                if !emit_path(path.clone()) {
                    break;
                }
                continue;
            }
            if marker.is_file() {
                if !emit_diagnostic(exclusion(path, "linked worktree or submodule ignored")) {
                    break;
                }
                continue;
            }
        }
        match std::fs::read_dir(&path) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => stack.push(entry.path()),
                        Err(error) => {
                            if !emit_diagnostic(diagnostic(
                                path.clone(),
                                format!("unreadable directory entry: {error}"),
                            )) {
                                return;
                            }
                        }
                    }
                }
            }
            Err(error) => {
                if !emit_diagnostic(diagnostic(path, format!("unreadable path: {error}"))) {
                    break;
                }
            }
        }
    }
}

fn inspect_checkout(
    path: &Path,
    cancel: &AtomicBool,
    mut failed: impl FnMut(),
) -> Option<LocalCheckout> {
    let origin = match git_output(path, ["remote", "get-url", "origin"], cancel) {
        Ok(origin) => {
            let original = origin.trim().to_owned();
            match normalize_remote(&original) {
                Ok(identity) => OriginFact::Supported { original, identity },
                Err(_) => OriginFact::Unsupported(original),
            }
        }
        Err(ProbeError::Cancelled) => return None,
        Err(error @ ProbeError::Exit { code: Some(2), .. }) => {
            match git_output(path, ["remote"], cancel) {
                Ok(remotes) if !remotes.lines().any(|remote| remote == "origin") => {
                    OriginFact::Absent
                }
                Err(ProbeError::Cancelled) => return None,
                Ok(_) | Err(_) => {
                    failed();
                    OriginFact::Error(error.to_string())
                }
            }
        }
        Err(error) => {
            failed();
            OriginFact::Error(error.to_string())
        }
    };
    let identity = match &origin {
        OriginFact::Supported { identity, .. } => Some(identity.clone()),
        _ => None,
    };
    Some(LocalCheckout {
        path: path.to_path_buf(),
        identity,
        origin,
        branch: match branch_output(path, cancel) {
            Ok(branch) => ObservationField::Known(branch.trim().to_owned()),
            Err(ProbeError::Cancelled) => return None,
            Err(error) => {
                failed();
                ObservationField::Error(error.to_string())
            }
        },
        changes: match git_output(
            path,
            ["status", "--porcelain=v1", "--untracked-files=all"],
            cancel,
        ) {
            Ok(status) if status.trim().is_empty() => ObservationField::Clean,
            Ok(status) => ObservationField::Dirty(format!("{} modified", status.lines().count())),
            Err(ProbeError::Cancelled) => return None,
            Err(error) => {
                failed();
                ObservationField::Error(error.to_string())
            }
        },
    })
}

fn branch_output(path: &Path, cancel: &AtomicBool) -> Result<String, ProbeError> {
    match git_output(path, ["symbolic-ref", "--quiet", "--short", "HEAD"], cancel) {
        Err(ProbeError::Exit {
            code: Some(1),
            stderr,
        }) if stderr.is_empty() => {
            git_output(path, ["rev-parse", "--short", "--verify", "HEAD"], cancel)
                .map(|commit| format!("detached HEAD {}", commit.trim()))
        }
        result => result,
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum ProbeError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
    #[error("git command failed (exit {code:?}): {stderr}")]
    Exit { code: Option<i32>, stderr: String },
}

impl From<std::io::Error> for ProbeError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

fn git_output<const N: usize>(
    directory: &Path,
    args: [&str; N],
    cancel: &AtomicBool,
) -> Result<String, ProbeError> {
    let mut command = Command::new("git");
    command.args(["-C"]).arg(directory).args(args);
    clean_git_environment(&mut command);
    command_output(command, cancel)
}

fn git_dir_output<const N: usize>(
    directory: &Path,
    args: [&str; N],
    cancel: &AtomicBool,
) -> Result<String, ProbeError> {
    let mut command = Command::new("git");
    command.arg("--git-dir").arg(directory).args(args);
    clean_git_environment(&mut command);
    command_output(command, cancel)
}

fn clean_git_environment(command: &mut Command) {
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        command.env_remove(OsStr::new(name));
    }
}

fn command_output(mut command: Command, cancel: &AtomicBool) -> Result<String, ProbeError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ProbeError::Cancelled);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Background probes get their own process group. A Git wrapper can leave descendants holding
    // the capture pipes, so killing only the immediate child would make reader joins unbounded.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let read = |mut pipe: std::process::ChildStdout| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let stdout_reader = read(stdout);
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut pipe = stderr;
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            #[cfg(unix)]
            terminate_background_group(child.id());
            #[cfg(not(unix))]
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(ProbeError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    // The immediate parent can exit while a descendant still owns the capture pipes. These
    // probes have a dedicated process group, so ending that owned group before joining readers
    // prevents a successful parent status from becoming an unbounded pipe wait.
    #[cfg(unix)]
    terminate_background_group(child.id());
    let stdout = stdout_reader
        .join()
        .map_err(|_| ProbeError::Failed("stdout reader panicked".to_owned()))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ProbeError::Failed("stderr reader panicked".to_owned()))??;
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        Err(ProbeError::Exit {
            code: status.code(),
            stderr,
        })
    }
}

#[cfg(unix)]
fn terminate_background_group(pid: u32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

fn diagnostic(path: PathBuf, message: impl Into<String>) -> ScanDiagnostic {
    ScanDiagnostic {
        path,
        message: message.into(),
        incomplete: true,
    }
}

fn exclusion(path: PathBuf, message: impl Into<String>) -> ScanDiagnostic {
    ScanDiagnostic {
        path,
        message: message.into(),
        incomplete: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    #[test]
    fn receiver_target_validation_uses_metadata_not_config_contents() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let config = checkout.join(".git/config");
        std::fs::write(&config, "origin = old\n").unwrap();
        let target = TargetSnapshot::capture(checkout);

        // Keep the config length stable while changing the existing inode in place.
        std::fs::write(config, "origin = new\n").unwrap();

        assert!(!target.matches_current());
        assert!(!target.contents_match_current());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_descendants_holding_capture_pipes() {
        let cancel = Arc::new(AtomicBool::new(false));
        let command_cancel = Arc::clone(&cancel);
        let started = Instant::now();
        let command = std::thread::spawn(move || {
            let mut command = Command::new("sh");
            command.args(["-c", "(sleep 10) & wait"]);
            command_output(command, &command_cancel)
        });
        std::thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(command.join().unwrap(), Err(ProbeError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn successful_parent_exit_reaps_descendant_holding_capture_pipes() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("descendant-pid");
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("(sleep 3) & echo $! > {} ; exit 0", pid_file.display()),
        ]);
        assert!(command_output(command, &cancel).is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let exists = || {
            Command::new("sh")
                .args(["-c", &format!("kill -0 {}", pid.trim())])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !exists(),
            "managed descendant still exists after successful parent exit"
        );
    }

    #[test]
    fn dirty_git_status_larger_than_a_pipe_completes() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repository");
        let init = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&repository)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        for index in 0..5_000 {
            std::fs::write(
                repository.join(format!(
                    "untracked-{index:04}-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                )),
                "x",
            )
            .unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = git_output(
                &repository,
                ["status", "--porcelain=v1", "--untracked-files=all"],
                &worker_cancel,
            );
            let _ = sender.send(result);
        });
        let deadline = Duration::from_secs(2);
        let result = match receiver.recv_timeout(deadline) {
            Ok(result) => result.map_err(|error| format!("real Git status failed: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Do not merely time out the assertion: cancel the managed Git process, require
                // its worker to report cleanup, then fail after joining it below.
                cancel.store(true, Ordering::Relaxed);
                match receiver.recv_timeout(deadline) {
                    Ok(Err(error)) => Err(format!(
                        "real large Git status exceeded its independent deadline; cleanup: {error}"
                    )),
                    Ok(Ok(_)) => {
                        Err("large status completed only after watchdog cancellation".to_owned())
                    }
                    Err(error) => Err(format!(
                        "large-status watchdog could not cancel and reap Git: {error}"
                    )),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("large-status worker disconnected before reporting".to_owned())
            }
        };
        worker.join().expect("large-status worker panicked");
        let output = result.unwrap_or_else(|error| panic!("{error}"));
        assert!(output.len() > 200_000, "status output was not pipe-sized");
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_pipe_drains_accept_large_status_output() {
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.args(["-c", "yes status | head -c 200000"]);
        assert_eq!(command_output(command, &cancel).unwrap().len(), 200000);
    }
}
