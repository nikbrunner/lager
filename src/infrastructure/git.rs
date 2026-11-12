use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path};
use std::process::{Command, ExitStatus, Stdio};

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Dir, Mode, OFlags, mkdirat, openat, unlinkat};

use crate::application::ports::{
    GitClient, GitRemoval, GitState, WorktreeCommandError, WorktreeEntry, WorktreeFault,
    WorktreeGit, WorktreeLocation, WorktreeState,
};
use crate::domain::repository::normalize_remote;
use crate::domain::state::LocalState;

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeGit;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("unsafe clone destination {path}: {message}")]
    UnsafePath { path: String, message: String },
    #[error("could not prepare Git directory {path}: {source}")]
    Parent {
        path: String,
        source: std::io::Error,
    },
    #[error("could not start git: {0}")]
    Start(#[source] std::io::Error),
    #[error("git exited with {}", super::exit_status::describe(.0))]
    Failed(ExitStatus),
    #[error("git failed and its fresh destination could not be safely cleaned: {0}")]
    Cleanup(#[source] std::io::Error),
}

// Preserve the native status even when cleanup replaces the public error variant.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
struct CloneCleanupError {
    status: ExitStatus,
    #[source]
    source: std::io::Error,
}

pub fn clone_repository(
    url: &str,
    home: &Path,
    root: &Path,
    destination: &Path,
) -> Result<(), GitError> {
    let root_relative = root.strip_prefix(home).map_err(|_| {
        unsafe_path(
            root,
            "configured root is outside the trusted home directory",
        )
    })?;
    let destination_relative = destination
        .strip_prefix(root)
        .map_err(|_| unsafe_path(destination, "destination is outside the configured root"))?;
    let mut destination_components = normal_components(destination_relative, destination)?;
    let destination_name = destination_components
        .pop()
        .ok_or_else(|| unsafe_path(destination, "destination has no repository name"))?;

    let canonical_home = std::fs::canonicalize(home).map_err(|source| GitError::Parent {
        path: home.display().to_string(),
        source,
    })?;
    let home_fd = open_absolute_directory(&canonical_home)?;
    let root_fd = walk_or_create(&home_fd, root_relative, root)?;
    let parent_fd = walk_components_or_create(&root_fd, &destination_components, destination)?;

    mkdirat(&parent_fd, destination_name, directory_mode()).map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            unsafe_path(destination, "destination already exists")
        } else {
            parent_error(destination, error)
        }
    })?;
    let destination_fd = open_directory_at(&parent_fd, destination_name, destination)?;
    let child_directory = rustix::io::fcntl_dupfd_cloexec(&destination_fd, 0)
        .map_err(|error| parent_error(destination, error))?;

    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg(url)
        .arg(".")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // SAFETY: fchdir is async-signal-safe, performs no allocation, and the owned
    // descriptor remains captured by the closure until after the child execs.
    unsafe {
        command.pre_exec(move || {
            rustix::process::fchdir(&child_directory).map_err(std::io::Error::from)
        });
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(source) => {
            cleanup_destination(&parent_fd, destination_name, &destination_fd)
                .map_err(GitError::Cleanup)?;
            return Err(GitError::Start(source));
        }
    };
    if status.success() {
        Ok(())
    } else {
        cleanup_destination(&parent_fd, destination_name, &destination_fd).map_err(|source| {
            GitError::Cleanup(std::io::Error::new(
                source.kind(),
                CloneCleanupError { status, source },
            ))
        })?;
        Err(GitError::Failed(status))
    }
}

fn open_absolute_directory(path: &Path) -> Result<OwnedFd, GitError> {
    let mut directory: OwnedFd = File::open("/")
        .map_err(|source| GitError::Parent {
            path: "/".to_owned(),
            source,
        })?
        .into();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = open_directory_at(&directory, name, path)?;
            }
            _ => return Err(unsafe_path(path, "trusted home path is not normalized")),
        }
    }
    Ok(directory)
}

fn walk_or_create(parent: &OwnedFd, relative: &Path, display: &Path) -> Result<OwnedFd, GitError> {
    let components = normal_components(relative, display)?;
    walk_components_or_create(parent, &components, display)
}

fn walk_components_or_create(
    parent: &OwnedFd,
    components: &[&OsStr],
    display: &Path,
) -> Result<OwnedFd, GitError> {
    let mut directory =
        rustix::io::fcntl_dupfd_cloexec(parent, 0).map_err(|error| parent_error(display, error))?;
    for name in components {
        directory = match open_directory_at(&directory, name, display) {
            Ok(child) => child,
            Err(GitError::Parent { source, .. })
                if source.raw_os_error() == Some(rustix::io::Errno::NOENT.raw_os_error()) =>
            {
                mkdirat(&directory, *name, directory_mode())
                    .map_err(|error| parent_error(display, error))?;
                open_directory_at(&directory, name, display)?
            }
            Err(error) => return Err(error),
        };
    }
    Ok(directory)
}

fn open_directory_at(parent: &OwnedFd, name: &OsStr, display: &Path) -> Result<OwnedFd, GitError> {
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if matches!(error, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
            unsafe_path(
                display,
                "an existing ancestor is a symlink or not a directory",
            )
        } else {
            parent_error(display, error)
        }
    })
}

fn normal_components<'a>(path: &'a Path, display: &Path) -> Result<Vec<&'a OsStr>, GitError> {
    path.components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(unsafe_path(
                display,
                "destination contains a non-portable component",
            )),
        })
        .collect()
}

fn cleanup_destination(
    parent: &OwnedFd,
    name: &OsStr,
    destination: &OwnedFd,
) -> Result<(), std::io::Error> {
    clear_directory(destination)?;
    unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(std::io::Error::from)
}

fn clear_directory(directory: &OwnedFd) -> Result<(), std::io::Error> {
    let mut entries = Dir::read_from(directory).map_err(std::io::Error::from)?;
    let mut names = Vec::<CString>::new();
    for entry in &mut entries {
        let entry = entry.map_err(std::io::Error::from)?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    for name in names {
        match openat(
            directory,
            name.as_c_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(child) => {
                clear_directory(&child)?;
                unlinkat(directory, name.as_c_str(), AtFlags::REMOVEDIR)
                    .map_err(std::io::Error::from)?;
            }
            Err(_) => unlinkat(directory, name.as_c_str(), AtFlags::empty())
                .map_err(std::io::Error::from)?,
        }
    }
    Ok(())
}

fn directory_mode() -> Mode {
    Mode::from_bits_truncate(0o755)
}

fn parent_error(path: &Path, error: rustix::io::Errno) -> GitError {
    GitError::Parent {
        path: path.display().to_string(),
        source: error.into(),
    }
}

fn unsafe_path(path: &Path, message: &str) -> GitError {
    GitError::UnsafePath {
        path: path.display().to_string(),
        message: message.to_owned(),
    }
}

impl GitClient for NativeGit {
    type Error = GitError;

    fn is_cancelled(&self, error: &Self::Error) -> bool {
        match error {
            GitError::Failed(status) => super::exit_status::is_cancelled(*status),
            GitError::Cleanup(error) => error
                .get_ref()
                .and_then(|source| source.downcast_ref::<CloneCleanupError>())
                .is_some_and(|error| super::exit_status::is_cancelled(error.status)),
            _ => false,
        }
    }

    fn clone_repository(
        &self,
        url: &str,
        home: &Path,
        root: &Path,
        destination: &Path,
    ) -> Result<(), Self::Error> {
        clone_repository(url, home, root, destination)
    }
}

impl GitState for NativeGit {
    fn classify_destination(&self, destination: &Path, expected_url: &str) -> LocalState {
        classify_destination(destination, expected_url)
    }
}

impl GitRemoval for NativeGit {
    fn inspect_removal(
        &self,
        destination: &Path,
        expected_url: &str,
    ) -> Result<Vec<String>, String> {
        inspect_removal(destination, expected_url)
    }

    fn origin_identity(&self, destination: &Path) -> Result<String, String> {
        git_output(destination, &["remote", "get-url", "origin"])
            .map(|output| normalize_remote(output.trim()).map_err(|error| error.to_string()))?
    }
}

pub fn inspect_removal(destination: &Path, expected_url: &str) -> Result<Vec<String>, String> {
    let git_marker = destination.join(".git");
    if !git_marker.is_dir() {
        return Err("destination does not contain a real .git directory".to_owned());
    }
    let inside = git_output(destination, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return Err("destination is not inside a Git work tree".to_owned());
    }
    let bare = git_output(destination, &["rev-parse", "--is-bare-repository"])?;
    if bare.trim() != "false" {
        return Err("bare repositories cannot be removed".to_owned());
    }
    let root = git_output(destination, &["rev-parse", "--show-toplevel"])?;
    let requested = std::fs::canonicalize(destination).map_err(|error| error.to_string())?;
    let actual_root = std::fs::canonicalize(root.trim()).map_err(|error| error.to_string())?;
    if requested != actual_root {
        return Err("destination is not the Git work-tree root".to_owned());
    }
    let git_dir = git_output(destination, &["rev-parse", "--git-dir"])?;
    let common_dir = git_output(destination, &["rev-parse", "--git-common-dir"])?;
    let git_dir = std::fs::canonicalize(destination.join(git_dir.trim()))
        .or_else(|_| std::fs::canonicalize(git_dir.trim()))
        .map_err(|error| error.to_string())?;
    let common_dir = std::fs::canonicalize(destination.join(common_dir.trim()))
        .or_else(|_| std::fs::canonicalize(common_dir.trim()))
        .map_err(|error| error.to_string())?;
    if git_dir != common_dir {
        return Err("linked worktrees cannot be removed".to_owned());
    }
    let origin = git_output(destination, &["remote", "get-url", "origin"])?;
    let actual = normalize_remote(origin.trim()).map_err(|error| error.to_string())?;
    let expected = normalize_remote(expected_url).map_err(|error| error.to_string())?;
    if actual != expected {
        return Err("origin does not match the requested repository".to_owned());
    }

    let worktrees = common_dir.join("worktrees");
    match std::fs::symlink_metadata(&worktrees) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect dependent worktree metadata: {error}"
            ));
        }
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err("dependent worktree metadata is not a real directory".to_owned());
            }
            let mut entries = std::fs::read_dir(&worktrees)
                .map_err(|error| format!("cannot read dependent worktree metadata: {error}"))?;
            if let Some(entry) = entries.next() {
                entry.map_err(|error| {
                    format!("cannot read dependent worktree metadata entry: {error}")
                })?;
                // Even stale or malformed entries may own linked-checkout state.
                return Err(
                    "dependent worktree metadata prevents primary checkout removal".to_owned(),
                );
            }
        }
    }

    let mut warnings = Vec::new();
    let status = git_output(
        destination,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    if status.lines().any(|line| line.starts_with("!!")) {
        warnings.push("ignored files".to_owned());
    }
    if status.lines().any(|line| {
        let bytes = line.as_bytes();
        !line.starts_with("!!") && bytes.len() >= 2 && bytes[0] != b' ' && bytes[0] != b'?'
    }) {
        warnings.push("staged changes".to_owned());
    }
    if status.lines().any(|line| {
        let bytes = line.as_bytes();
        !line.starts_with("!!") && bytes.len() >= 2 && bytes[1] != b' ' && bytes[1] != b'?'
    }) {
        warnings.push("unstaged changes".to_owned());
    }
    if status.lines().any(|line| line.starts_with("??")) {
        warnings.push("untracked files".to_owned());
    }
    if git_output(
        destination,
        &["rev-parse", "--verify", "--quiet", "refs/stash"],
    )
    .is_ok()
    {
        warnings.push("stashes".to_owned());
    }
    if git_output(destination, &["symbolic-ref", "--quiet", "--short", "HEAD"]).is_err() {
        warnings.push("detached HEAD".to_owned());
    }
    if git_output(destination, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        warnings.push("unsafe branch tip".to_owned());
    }
    if git_output(
        destination,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .is_err()
    {
        warnings.push("no upstream".to_owned());
    } else {
        let ahead = git_output(destination, &["rev-list", "--count", "@{u}..HEAD"])?;
        if ahead.trim().parse::<u64>().unwrap_or(1) > 0 {
            warnings.push("local commits absent from the remote-tracking ref".to_owned());
        }
    }
    let submodules = git_output(destination, &["submodule", "status", "--recursive"])?;
    if submodules
        .lines()
        .any(|line| matches!(line.chars().next(), Some('+') | Some('-') | Some('U')))
    {
        warnings.push("dirty submodules".to_owned());
    }
    Ok(warnings)
}

fn git_output(directory: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(["-C"]).arg(directory).args(args);
    crate::infrastructure::inventory::clean_git_environment(&mut command);
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            "git command failed".to_owned()
        } else {
            message
        })
    }
}

pub fn classify_destination(destination: &Path, expected_url: &str) -> LocalState {
    let metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LocalState::Missing,
        Err(_) => return LocalState::Unreadable,
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return LocalState::Conflict;
    }

    let inside = match Command::new("git")
        .args(["-C"])
        .arg(destination)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return LocalState::Unreadable,
    };
    if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return LocalState::Conflict;
    }

    let origin = match Command::new("git")
        .args(["-C"])
        .arg(destination)
        .args(["remote", "get-url", "origin"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return LocalState::Unreadable,
    };
    if !origin.status.success() {
        return LocalState::Conflict;
    }
    let actual = String::from_utf8_lossy(&origin.stdout);
    match (
        normalize_remote(actual.trim()),
        normalize_remote(expected_url),
    ) {
        (Ok(actual), Ok(expected)) if actual == expected => LocalState::Cloned,
        (Ok(_), Ok(_)) => LocalState::Conflict,
        _ => LocalState::Conflict,
    }
}

fn git_at(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory);
    crate::infrastructure::inventory::clean_git_environment(&mut command);
    command
}

fn captured(command: &mut Command) -> Result<Vec<u8>, String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if message.is_empty() {
        format!(
            "git exited with {}",
            super::exit_status::describe(&output.status)
        )
    } else {
        message
    })
}

fn captured_text(command: &mut Command) -> Result<String, String> {
    captured(command).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
}

/// Runs a mutation on the caller's own streams.
fn native(command: &mut Command) -> Result<(), WorktreeCommandError> {
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(WorktreeCommandError {
            message: format!("git exited with {}", super::exit_status::describe(&status)),
            cancelled: super::exit_status::is_cancelled(status),
        }),
        Err(error) => Err(WorktreeCommandError {
            message: format!("could not start git: {error}"),
            cancelled: false,
        }),
    }
}

fn fault(path: &Path, code: &'static str, error: impl ToString) -> WorktreeFault {
    WorktreeFault {
        path: Some(path.to_path_buf()),
        code,
        error: error.to_string(),
    }
}

/// Reads `git worktree list --porcelain -z`: NUL ends an attribute, an empty one ends a record.
fn parse_worktree_entries(bytes: &[u8]) -> Result<Vec<WorktreeEntry>, String> {
    let text = |value: Option<&[u8]>| {
        String::from_utf8(value.unwrap_or_default().to_vec())
            .map_err(|_| "worktree attribute is not UTF-8".to_owned())
    };
    let reason = |value: Option<&[u8]>| value.map(|bytes| String::from_utf8_lossy(bytes).into());
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            entries.extend(current.take());
            continue;
        }
        let (key, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(space) => (&field[..space], Some(&field[space + 1..])),
            None => (field, None),
        };
        if key == b"worktree" {
            let path = value.filter(|path| !path.is_empty());
            if current.is_some() || path.is_none() {
                return Err("worktree record is malformed".to_owned());
            }
            current = Some(WorktreeEntry {
                path: std::path::PathBuf::from(OsStr::from_bytes(path.unwrap_or_default())),
                ..WorktreeEntry::default()
            });
            continue;
        }
        let entry = current
            .as_mut()
            .ok_or_else(|| "worktree attribute precedes its record".to_owned())?;
        match key {
            b"HEAD" => {
                let id = text(value)?;
                entry.head = (!id.bytes().all(|byte| byte == b'0')).then_some(id);
            }
            b"branch" => entry.branch = Some(text(value)?),
            b"bare" => entry.bare = true,
            b"locked" => {
                entry.locked = true;
                entry.lock_reason = reason(value);
            }
            b"prunable" => {
                entry.prunable = true;
                entry.prune_reason = reason(value);
            }
            _ => {}
        }
    }
    entries.extend(current);
    if entries.is_empty() {
        return Err("Git listed no worktree".to_owned());
    }
    Ok(entries)
}

impl WorktreeGit for NativeGit {
    fn locate(&self, path: &Path) -> Result<WorktreeLocation, String> {
        let output = captured_text(git_at(path).args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ]))?;
        let [top_level, git_dir, common_dir] = output.lines().collect::<Vec<_>>()[..] else {
            return Err("unexpected Git location output".to_owned());
        };
        let canonical = |path: &str| std::fs::canonicalize(path).map_err(|error| error.to_string());
        Ok(WorktreeLocation {
            top_level: canonical(top_level)?,
            git_dir: canonical(git_dir)?,
            common_dir: canonical(common_dir)?,
        })
    }

    fn entries(&self, repository: &Path) -> Result<Vec<WorktreeEntry>, WorktreeFault> {
        let output = captured(git_at(repository).args(["worktree", "list", "--porcelain", "-z"]))
            .map_err(|error| fault(repository, "git_failed", error))?;
        parse_worktree_entries(&output)
            .map_err(|error| fault(repository, "invalid_metadata", error))
    }

    fn metadata_faults(&self, common_dir: &Path) -> Vec<WorktreeFault> {
        use std::io::ErrorKind;
        let directory = common_dir.join("worktrees");
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
            Err(error) => return vec![fault(&directory, "unreadable", error)],
        };
        let mut faults = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    faults.push(fault(&directory, "unreadable", error));
                    continue;
                }
            };
            match std::fs::read_to_string(path.join("gitdir")) {
                Ok(text) if text.trim().is_empty() => {
                    faults.push(fault(&path, "invalid_metadata", "gitdir file is empty"));
                }
                Ok(_) => {}
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    faults.push(fault(
                        &path,
                        "invalid_metadata",
                        "worktree entry has no gitdir file",
                    ));
                }
                Err(error) => faults.push(fault(&path, "unreadable", error)),
            }
        }
        faults
    }

    fn origin_identity(&self, repository: &Path) -> Result<Option<String>, String> {
        let remotes = captured_text(git_at(repository).arg("remote"))?;
        if !remotes.lines().any(|remote| remote == "origin") {
            return Ok(None);
        }
        let url = captured_text(git_at(repository).args(["remote", "get-url", "origin"]))?;
        Ok(normalize_remote(&url).ok())
    }

    fn commit(&self, repository: &Path, revision: &str) -> Option<String> {
        captured_text(git_at(repository).args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ]))
        .ok()
        .filter(|commit| !commit.is_empty())
    }

    fn branch_exists(&self, repository: &Path, name: &str) -> Result<bool, String> {
        let output = git_at(repository)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{name}"))
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("could not start git: {error}"))?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
        }
    }

    fn valid_branch_name(&self, name: &str) -> bool {
        let mut command = Command::new("git");
        crate::infrastructure::inventory::clean_git_environment(&mut command);
        !name.starts_with('-')
            && name != "HEAD"
            && captured(command.args(["check-ref-format", &format!("refs/heads/{name}")])).is_ok()
    }

    fn add(
        &self,
        repository: &Path,
        destination: &Path,
        branch: &str,
        start: Option<&str>,
    ) -> Result<(), WorktreeCommandError> {
        let mut command = git_at(repository);
        command.args(["worktree", "add"]);
        if start.is_some() {
            command.args(["-b", branch]);
        }
        command
            .arg("--")
            .arg(destination)
            .arg(start.unwrap_or(branch));
        native(&mut command)
    }

    fn linked_path(&self, git_dir: &Path) -> Result<std::path::PathBuf, String> {
        let text =
            std::fs::read_to_string(git_dir.join("gitdir")).map_err(|error| error.to_string())?;
        // Git writes this pointer absolute, or relative to the linked Git directory.
        let pointer = git_dir.join(text.trim_end_matches(['\n', '\r']));
        let worktree = pointer
            .parent()
            .filter(|_| pointer.file_name() == Some(OsStr::new(".git")))
            .ok_or_else(|| "gitdir file does not name a .git file".to_owned())?;
        std::fs::canonicalize(worktree).map_err(|error| error.to_string())
    }

    fn local_state(&self, worktree: &Path) -> Result<WorktreeState, String> {
        let output = captured(git_at(worktree).args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ]))?;
        let mut state = WorktreeState::default();
        // Every directory below the root that holds a tracked or untracked path.
        let mut directories = std::collections::BTreeSet::new();
        let mut holds = |path: &[u8]| {
            let mut directory = Path::new(OsStr::from_bytes(path)).parent();
            while let Some(parent) = directory.filter(|parent| !parent.as_os_str().is_empty()) {
                if !directories.insert(parent.to_path_buf()) {
                    break;
                }
                directory = parent.parent();
            }
        };
        let mut fields = output
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty());
        while let Some(field) = fields.next() {
            let (Some(code), Some(path)) = (field.get(..2), field.get(3..)) else {
                return Err("unexpected Git status output".to_owned());
            };
            let directory = path
                .strip_suffix(b"/")
                .map(|path| Path::new(OsStr::from_bytes(path)));
            match (code, directory) {
                // With every untracked file listed, a directory entry is an embedded repository.
                (b"??", Some(directory)) => state
                    .nested_repositories
                    .push(directory.display().to_string()),
                (b"??", None) => {
                    holds(path);
                    state.losses.untracked += 1;
                }
                (b"!!", directory) => {
                    state.losses.ignored = true;
                    // Only the ignored directory itself: its contents are ignored-file loss.
                    if let Some(directory) = directory
                        && std::fs::symlink_metadata(worktree.join(directory).join(".git")).is_ok()
                    {
                        state
                            .nested_repositories
                            .push(directory.display().to_string());
                    }
                }
                _ => {
                    state.losses.modified += 1;
                    // A rename or copy carries its source path as a second field.
                    if code.contains(&b'R') || code.contains(&b'C') {
                        fields.next();
                    }
                }
            }
        }
        // Status skips files marked skip-worktree (`S`) or assume-unchanged (lower case),
        // edited or not; a present one counts as modified.
        let hidden = captured(git_at(worktree).args(["ls-files", "-v", "-z"]))?;
        for entry in hidden.split(|byte| *byte == 0) {
            let (Some(tag), Some(path)) = (entry.first(), entry.get(2..)) else {
                continue;
            };
            holds(path);
            if (*tag == b'S' || tag.is_ascii_lowercase())
                && std::fs::symlink_metadata(worktree.join(OsStr::from_bytes(path))).is_ok()
            {
                state.losses.modified += 1;
            }
        }
        // Status names a repository only where the directory is untracked or ignored; one
        // beside tracked files, or one Git does not accept, shows as a `.git` entry alone.
        for directory in directories {
            match std::fs::symlink_metadata(worktree.join(&directory).join(".git")) {
                Ok(_) => {
                    let name = directory.display().to_string();
                    if !state.nested_repositories.contains(&name) {
                        state.nested_repositories.push(name);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => return Err(format!("{}: {error}", directory.display())),
            }
        }
        Ok(state)
    }

    fn populated_submodules(&self, worktree: &Path) -> Result<Vec<String>, String> {
        let output = captured(git_at(worktree).args(["ls-files", "-z", "--stage"]))?;
        let mut populated = Vec::new();
        for field in output.split(|byte| *byte == 0) {
            let Some(tab) = field.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            if !field.starts_with(b"160000 ") {
                continue;
            }
            let path = Path::new(OsStr::from_bytes(&field[tab + 1..]));
            match std::fs::symlink_metadata(worktree.join(path).join(".git")) {
                Ok(_) => {
                    let name = path.display().to_string();
                    if !populated.contains(&name) {
                        populated.push(name);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Git does not walk a submodule path, so its status names nothing in here.
                    // A symlink there is not a directory to read.
                    let directory = worktree.join(path);
                    if std::fs::symlink_metadata(&directory).is_ok_and(|entry| entry.is_dir()) {
                        match std::fs::read_dir(&directory) {
                            Ok(mut entries) => {
                                if entries.next().is_some() {
                                    return Err(format!(
                                        "{}: unpopulated submodule directory holds files Git does not report",
                                        path.display()
                                    ));
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::NotFound
                                        | std::io::ErrorKind::NotADirectory
                                ) => {}
                            Err(error) => return Err(format!("{}: {error}", path.display())),
                        }
                    }
                }
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        Ok(populated)
    }

    fn protecting_ref(&self, repository: &Path, commit: &str) -> Result<Option<String>, String> {
        // Replacement refs rewrite ancestry, so the proof reads the real objects.
        // A grafts file rewrites ancestry the same way; the proof reads none.
        let protecting =
            captured_text(git_at(repository).env("GIT_GRAFT_FILE", "/dev/null").args([
                "--no-replace-objects",
                "for-each-ref",
                "--count=1",
                "--format=%(refname)",
                "--contains",
                commit,
                "refs/heads/",
                "refs/tags/",
                "refs/remotes/",
            ]))?;
        Ok((!protecting.is_empty()).then_some(protecting))
    }

    fn unpushed(&self, repository: &Path, branch: &str) -> Option<usize> {
        let remote = captured_text(git_at(repository).args([
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "refs/remotes/",
        ]))
        .ok()?;
        if remote.is_empty() {
            return None;
        }
        captured_text(git_at(repository).args([
            "rev-list",
            "--count",
            branch,
            "--not",
            "--remotes",
        ]))
        .ok()?
        .parse()
        .ok()
    }

    fn remove(
        &self,
        repository: &Path,
        worktree: &Path,
        force: bool,
    ) -> Result<(), WorktreeCommandError> {
        let mut command = git_at(repository);
        command.args(["worktree", "remove"]);
        if force {
            command.arg("--force");
        }
        native(command.arg("--").arg(worktree))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn reports_local_state_warnings_without_network_access() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let sub_source = temp.path().join("sub-source");
        let remote = temp.path().join("origin.git");
        let clone = temp.path().join("clone");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&sub_source).unwrap();
        run_git(&source, &["init", "-q"]);
        run_git(&source, &["config", "user.email", "lager@example.invalid"]);
        run_git(&source, &["config", "user.name", "lager"]);
        run_git(&sub_source, &["init", "-q"]);
        run_git(
            &sub_source,
            &["config", "user.email", "lager@example.invalid"],
        );
        run_git(&sub_source, &["config", "user.name", "lager"]);
        fs::write(sub_source.join("README"), "submodule\n").unwrap();
        run_git(&sub_source, &["add", "README"]);
        run_git(&sub_source, &["commit", "-qm", "initial"]);
        fs::write(source.join("README"), "initial\n").unwrap();
        run_git_args(&[
            "-c",
            "protocol.file.allow=always",
            "-C",
            source.to_str().unwrap(),
            "submodule",
            "add",
            "-q",
            sub_source.to_str().unwrap(),
            "modules/sub",
        ]);
        run_git(&source, &["add", "."]);
        run_git(&source, &["commit", "-qm", "initial"]);
        run_git_args(&[
            "clone",
            "-q",
            "--bare",
            source.to_str().unwrap(),
            remote.to_str().unwrap(),
        ]);
        run_git_args(&[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ]);
        run_git(&clone, &["config", "user.email", "lager@example.invalid"]);
        run_git(&clone, &["config", "user.name", "lager"]);
        let origin = super::git_output(&clone, &["remote", "get-url", "origin"]).unwrap();

        fs::write(clone.join("stash.txt"), "stashed\n").unwrap();
        run_git(&clone, &["add", "stash.txt"]);
        run_git(&clone, &["stash", "push", "-qm", "preserve"]);
        let stashed = super::inspect_removal(&clone, origin.trim()).unwrap();
        assert!(stashed.iter().any(|warning| warning == "stashes"));
        assert!(stashed.iter().any(|warning| warning == "dirty submodules"));

        fs::write(clone.join("README"), "staged\n").unwrap();
        run_git(&clone, &["add", "README"]);
        fs::write(clone.join("README"), "unstaged\n").unwrap();
        fs::write(clone.join("new.txt"), "untracked\n").unwrap();
        fs::write(clone.join(".git/info/exclude"), "*.ignored\n").unwrap();
        fs::write(clone.join("local.ignored"), "ignored\n").unwrap();
        let states = super::inspect_removal(&clone, origin.trim()).unwrap();
        for expected in [
            "staged changes",
            "unstaged changes",
            "untracked files",
            "ignored files",
        ] {
            assert!(
                states.iter().any(|warning| warning == expected),
                "missing {expected}: {states:?}"
            );
        }

        run_git(&clone, &["add", "README"]);
        fs::write(clone.join("commit.txt"), "ahead\n").unwrap();
        run_git(&clone, &["add", "commit.txt"]);
        run_git(&clone, &["commit", "-qm", "ahead"]);
        let ahead = super::inspect_removal(&clone, origin.trim()).unwrap();
        assert!(
            ahead
                .iter()
                .any(|warning| warning.contains("absent from the remote-tracking ref"))
        );

        run_git(&clone, &["checkout", "-qb", "local-only"]);
        let no_upstream = super::inspect_removal(&clone, origin.trim()).unwrap();
        assert!(no_upstream.iter().any(|warning| warning == "no upstream"));
    }

    #[test]
    fn detached_head_is_unsafe_but_clean_attached_clone_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let remote = temp.path().join("origin.git");
        let clone = temp.path().join("clone");
        fs::create_dir_all(&source).unwrap();
        run_git(&source, &["init", "-q"]);
        run_git(&source, &["config", "user.email", "lager@example.invalid"]);
        run_git(&source, &["config", "user.name", "lager"]);
        fs::write(source.join("README"), "safe\n").unwrap();
        run_git(&source, &["add", "README"]);
        run_git(&source, &["commit", "-qm", "initial"]);
        run_git_args(&[
            "clone",
            "-q",
            "--bare",
            source.to_str().unwrap(),
            remote.to_str().unwrap(),
        ]);
        run_git_args(&[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ]);
        let origin = super::git_output(&clone, &["remote", "get-url", "origin"]).unwrap();

        let attached = super::inspect_removal(&clone, origin.trim()).unwrap();
        assert!(!attached.iter().any(|warning| warning == "detached HEAD"));

        run_git(&clone, &["checkout", "--detach", "HEAD"]);
        let detached = super::inspect_removal(&clone, origin.trim()).unwrap();
        assert!(detached.iter().any(|warning| warning == "detached HEAD"));
    }

    fn run_git(directory: &std::path::Path, args: &[&str]) {
        let mut command = std::process::Command::new("git");
        command
            .current_dir(directory)
            .args(args)
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL");
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_git_args(args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
