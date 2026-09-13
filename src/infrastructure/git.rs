use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path};
use std::process::{Command, ExitStatus, Stdio};

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Dir, Mode, OFlags, mkdirat, openat, unlinkat};

use crate::application::ports::{GitClient, GitRemoval, GitState};
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
    #[error("git exited with status {0}")]
    Failed(ExitStatus),
    #[error("git failed and its fresh destination could not be safely cleaned: {0}")]
    Cleanup(#[source] std::io::Error),
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
        cleanup_destination(&parent_fd, destination_name, &destination_fd)
            .map_err(GitError::Cleanup)?;
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
    if !Command::new("git")
        .args(["-C"])
        .arg(destination)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|error| error.to_string())?
        .status
        .success()
    {
        warnings.push("detached HEAD".to_owned());
    }
    if git_output(destination, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        warnings.push("unsafe branch tip".to_owned());
    }
    let upstream = Command::new("git")
        .args(["-C"])
        .arg(destination)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .map_err(|error| error.to_string())?;
    if !upstream.status.success() {
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
    let output = Command::new("git")
        .args(["-C"])
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
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
        command.current_dir(directory).args(args);
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
