mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    primary: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        let primary = home.join("repos/primary");
        fs::create_dir_all(&primary).unwrap();
        git(&primary, &["init", "-q"]);
        git(&primary, &["config", "user.email", "lager@example.invalid"]);
        git(&primary, &["config", "user.name", "lager"]);
        fs::write(primary.join("README"), "preserve me\n").unwrap();
        git(&primary, &["add", "README"]);
        git(&primary, &["commit", "-qm", "initial"]);
        git(&primary, &["remote", "add", "origin", "file:///primary"]);
        fs::write(
            &config,
            "root = \"repos\"\n[[repositories]]\nurl = \"file:///primary\"\n",
        )
        .unwrap();
        Self {
            temp,
            home,
            config,
            primary,
        }
    }

    fn dependent(&self, external: bool) -> PathBuf {
        let path = if external {
            self.temp.path().join("external")
        } else {
            self.home.join("repos/dependent")
        };
        git(
            &self.primary,
            &["worktree", "add", "-q", "--detach", path.to_str().unwrap()],
        );
        path
    }

    fn remove(&self) -> Command {
        let mut command = support::lager(&self.home, &self.config);
        command.args([
            "remove",
            "file:///primary",
            "--yes",
            "--force",
            "--unregister",
        ]);
        command
    }

    fn assert_blocked(&self, output: Output, config_before: &[u8]) {
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Filesystem safety checks may reject symlinks or unreadability before
        // Git inspection; both layers must fail closed with a useful reason.
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            ["worktree", "symlink", "Permission denied"]
                .iter()
                .any(|reason| diagnostic.contains(reason)),
            "{diagnostic}"
        );
        assert_eq!(fs::read(&self.config).unwrap(), config_before);
        assert_eq!(
            fs::read_to_string(self.primary.join("README")).unwrap(),
            "preserve me\n"
        );
        assert!(self.primary.join(".git").is_dir());
    }
}

fn git(directory: &Path, args: &[&str]) {
    let output = support::external(directory, "git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn live_dependent_blocks_forced_primary_removal_and_unregister() {
    let fixture = Fixture::new();
    let dependent = fixture.dependent(false);
    let before = fs::read(&fixture.config).unwrap();
    fixture.assert_blocked(fixture.remove().output().unwrap(), &before);
    assert!(dependent.join("README").is_file());
    git(&dependent, &["status", "--porcelain"]);
}

#[test]
fn external_locked_and_stale_dependencies_are_not_pruned_or_bypassed() {
    for state in ["external", "locked", "stale"] {
        let fixture = Fixture::new();
        let dependent = fixture.dependent(true);
        if state == "locked" {
            git(
                &fixture.primary,
                &["worktree", "lock", dependent.to_str().unwrap()],
            );
        } else if state == "stale" {
            fs::remove_dir_all(&dependent).unwrap();
        }
        let metadata = fixture.primary.join(".git/worktrees/external");
        let gitdir_before = fs::read(metadata.join("gitdir")).unwrap();
        let before = fs::read(&fixture.config).unwrap();
        fixture.assert_blocked(fixture.remove().output().unwrap(), &before);
        assert_eq!(fs::read(metadata.join("gitdir")).unwrap(), gitdir_before);
        if state == "locked" {
            assert!(metadata.join("locked").is_file());
        }
        if state != "stale" {
            assert!(dependent.join("README").is_file());
            git(&dependent, &["status", "--porcelain"]);
        } else {
            assert!(!dependent.exists());
        }
    }
}

#[test]
fn absent_or_empty_worktree_metadata_permits_removal() {
    for empty_directory in [false, true] {
        let fixture = Fixture::new();
        if empty_directory {
            fs::create_dir(fixture.primary.join(".git/worktrees")).unwrap();
        }
        let output = fixture.remove().output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!fixture.primary.exists());
        assert!(
            !fs::read_to_string(&fixture.config)
                .unwrap()
                .contains("file:///primary")
        );
    }
}

#[test]
fn malformed_worktree_directory_or_entry_blocks_removal() {
    for malformed_directory in [false, true] {
        let fixture = Fixture::new();
        let metadata = fixture.primary.join(".git/worktrees");
        let file = if malformed_directory {
            metadata
        } else {
            fs::create_dir(&metadata).unwrap();
            metadata.join("broken")
        };
        fs::write(&file, "not Git metadata\n").unwrap();
        let before = fs::read(&fixture.config).unwrap();
        fixture.assert_blocked(fixture.remove().output().unwrap(), &before);
        assert_eq!(fs::read_to_string(file).unwrap(), "not Git metadata\n");
    }
}

#[cfg(unix)]
#[test]
fn symlinked_worktree_directory_or_entry_blocks_removal() {
    for directory_link in [false, true] {
        for dangling in [false, true] {
            let fixture = Fixture::new();
            let metadata = fixture.primary.join(".git/worktrees");
            let outside = fixture.temp.path().join("outside");
            if !dangling {
                fs::create_dir(&outside).unwrap();
            }
            let link = if directory_link {
                metadata
            } else {
                fs::create_dir(&metadata).unwrap();
                metadata.join("broken")
            };
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let before = fs::read(&fixture.config).unwrap();
            fixture.assert_blocked(fixture.remove().output().unwrap(), &before);
            assert_eq!(fs::read_link(link).unwrap(), outside);
            assert_eq!(outside.exists(), !dangling);
        }
    }
}

#[cfg(unix)]
#[test]
fn unreadable_worktree_metadata_fails_closed() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let metadata = fixture.primary.join(".git/worktrees");
    fs::create_dir(&metadata).unwrap();
    fs::set_permissions(&metadata, fs::Permissions::from_mode(0o000)).unwrap();
    let probe = fs::read_dir(&metadata);
    if probe.is_ok() {
        fs::set_permissions(&metadata, fs::Permissions::from_mode(0o700)).unwrap();
        panic!("unreadability coverage unavailable: this user can read a mode-000 directory");
    }
    assert_eq!(
        probe.err().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let before = fs::read(&fixture.config).unwrap();
    let output = fixture.remove().output().unwrap();
    fs::set_permissions(&metadata, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.assert_blocked(output, &before);
}

#[test]
fn blocked_primary_does_not_prevent_independent_eligible_removal() {
    let fixture = Fixture::new();
    let dependent = fixture.dependent(true);
    let eligible = fixture.home.join("repos/eligible");
    git(
        &fixture.primary,
        &["clone", "-q", ".", eligible.to_str().unwrap()],
    );
    git(
        &eligible,
        &["remote", "set-url", "origin", "file:///eligible"],
    );
    let before = fs::read_to_string(&fixture.config).unwrap();
    fs::write(
        &fixture.config,
        format!("{before}[[repositories]]\nurl = \"file:///eligible\"\n"),
    )
    .unwrap();
    let output = fixture.remove().arg("file:///eligible").output().unwrap();
    fixture.assert_blocked(output, before.as_bytes());
    assert!(!eligible.exists());
    git(&dependent, &["status", "--porcelain"]);
}

#[cfg(unix)]
fn script(path: &Path, source: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn vanished_exact_picker_checkout_never_falls_back_or_unregisters_same_origin() {
    let fixture = Fixture::new();
    let selected = fixture.home.join("repos/custom");
    git(
        &fixture.primary,
        &["clone", "-q", ".", selected.to_str().unwrap()],
    );
    git(
        &selected,
        &["remote", "set-url", "origin", "file:///primary"],
    );
    let before = fs::read(&fixture.config).unwrap();
    let bin = fixture.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    // Move rather than delete the fixture checkout after discovery. Return only
    // its row; another checkout with the same origin remains at the configured path.
    script(
        &bin.join("fzf"),
        "#!/bin/sh\ncat > \"$LABELS\"\n/bin/mv \"$SELECTED\" \"$MOVED\" || exit 99\nwhile IFS= read -r line; do\ncase \"$line\" in *'/custom') printf '%s\\n' \"$line\";; esac\ndone < \"$LABELS\"\n",
    );
    let moved = fixture.temp.path().join("moved");
    let labels = fixture.temp.path().join("labels");
    let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&bin))
        .args(["remove", "--yes", "--force", "--unregister"])
        .env("SELECTED", &selected)
        .env("MOVED", &moved)
        .env("LABELS", &labels)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("selected clone path no longer exists"),
        "{output:?}"
    );
    assert_eq!(fs::read(&fixture.config).unwrap(), before);
    assert!(fixture.primary.join("README").is_file());
    assert!(moved.join("README").is_file());
    assert!(!selected.exists());
    assert_eq!(fs::read_to_string(labels).unwrap().lines().count(), 2);
}

#[cfg(unix)]
#[test]
fn dependency_arriving_after_initial_inspection_blocks_final_removal() {
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    let bin = fixture.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let started = fixture.temp.path().join("started");
    let release = fixture.temp.path().join("release");
    // submodule status is the last Git call in the initial inspection, after
    // the dependency check. Markers coordinate the mutation, not timing.
    script(
        &bin.join("git"),
        "#!/bin/sh\nif [ \"$3\" = submodule ] && [ ! -e \"$STARTED\" ]; then\n  printf ready > \"$STARTED\"\n  while [ ! -e \"$RELEASE\" ]; do :; done\nfi\nexec \"$REAL_GIT\" \"$@\"\n",
    );
    let before = fs::read(&fixture.config).unwrap();
    let mut child = support::lager_with_path(&fixture.home, &fixture.config, Some(&bin))
        .args([
            "remove",
            "file:///primary",
            "--yes",
            "--force",
            "--unregister",
        ])
        .env("STARTED", &started)
        .env("RELEASE", &release)
        .env("REAL_GIT", support::real_tool("git"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.exists() {
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("initial inspection marker was not reached: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let dependent = fixture.dependent(true);
    fs::write(release, "continue").unwrap();
    fixture.assert_blocked(child.wait_with_output().unwrap(), &before);
    git(&dependent, &["status", "--porcelain"]);
}

#[cfg(unix)]
#[test]
fn picker_selection_revalidates_worktree_dependencies() {
    let fixture = Fixture::new();
    let bin = fixture.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let dependent = fixture.temp.path().join("external");
    let marker = fixture.temp.path().join("picker-ran");
    // Add a real dependency after discovery but before returning the selection.
    script(
        &bin.join("fzf"),
        "#!/bin/sh\n\"$REAL_GIT\" -C \"$PRIMARY\" worktree add -q --detach \"$DEPENDENT\" || exit 99\nprintf selected > \"$MARKER\"\ncat\n",
    );
    let before = fs::read(&fixture.config).unwrap();
    let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&bin))
        .args(["remove", "--yes", "--force", "--unregister"])
        .env("REAL_GIT", support::real_tool("git"))
        .env("PRIMARY", &fixture.primary)
        .env("DEPENDENT", &dependent)
        .env("MARKER", &marker)
        .output()
        .unwrap();
    assert!(marker.is_file());
    fixture.assert_blocked(output, &before);
    git(&dependent, &["status", "--porcelain"]);
}
