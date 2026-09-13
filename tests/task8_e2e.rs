mod support;

use std::fs;

fn lager(config: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    support::lager(home, config).args(args).output().unwrap()
}

fn lager_with_path(
    config: &std::path::Path,
    home: &std::path::Path,
    path: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    support::lager_with_path(home, config, Some(path))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn remove_requires_unregister_choice_and_yes_outside_tty() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();

    let output = lager(&config, &home, &["remove", "github.com/org/repo"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));
}

#[test]
fn no_argument_remove_has_empty_success_and_picker_cancellation_exit() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let empty = lager(&config, &home, &["remove", "--keep-registered", "--yes"]);
    assert_eq!(empty.status.code(), Some(0));

    let source = temp.path().join("source");
    let remote = temp.path().join("origin.git");
    let destination = home.join("repos/repo");
    fs::create_dir_all(&source).unwrap();
    run_git(&source, &["init", "-q"]);
    run_git(&source, &["config", "user.email", "lager@example.invalid"]);
    run_git(&source, &["config", "user.name", "lager"]);
    fs::write(source.join("README"), "picker\n").unwrap();
    run_git(&source, &["add", "README"]);
    run_git(&source, &["commit", "-qm", "initial"]);
    run_git_args(&[
        "clone",
        "-q",
        "--bare",
        source.to_str().unwrap(),
        remote.to_str().unwrap(),
    ]);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    run_git_args(&[
        "clone",
        "-q",
        remote.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let picker = bin.join("fzf");
    fs::write(&picker, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&picker, fs::Permissions::from_mode(0o755)).unwrap();
    let cancelled = lager_with_path(
        &config,
        &home,
        &bin,
        &["remove", "--keep-registered", "--yes"],
    );
    assert_eq!(cancelled.status.code(), Some(130));
    assert!(destination.exists());
}

#[test]
fn remove_rejects_aliases_and_force_without_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();

    assert_eq!(
        lager(&config, &home, &["delete", "github.com/org/repo"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        lager(
            &config,
            &home,
            &["remove", "github.com/org/repo", "--remove"]
        )
        .status
        .code(),
        Some(2)
    );
    let force_without_yes = lager(
        &config,
        &home,
        &["remove", "github.com/org/repo", "--unregister", "--force"],
    );
    assert_eq!(force_without_yes.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&force_without_yes.stderr).contains("outside a TTY"));
}

#[test]
fn missing_target_honors_unregister_or_keep_registered() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    let document = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/repo\"\n";
    fs::write(&config, document).unwrap();
    let removed = lager(
        &config,
        &home,
        &["remove", "github.com/org/repo", "--unregister", "--yes"],
    );
    assert!(removed.status.success());
    assert!(
        !fs::read_to_string(&config)
            .unwrap()
            .contains("repositories")
    );

    fs::write(&config, document).unwrap();
    let kept = lager(
        &config,
        &home,
        &[
            "remove",
            "github.com/org/repo",
            "--keep-registered",
            "--yes",
        ],
    );
    assert!(kept.status.success());
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("repositories")
    );
}

#[cfg(unix)]
#[test]
fn force_never_bypasses_git_boundary_or_origin_guards() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let source = temp.path().join("source");
    let outside = temp.path().join("outside");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&outside).unwrap();
    run_git(&source, &["init", "-q"]);
    run_git(&source, &["config", "user.email", "lager@example.invalid"]);
    run_git(&source, &["config", "user.name", "lager"]);
    fs::write(source.join("README"), "linked\n").unwrap();
    run_git(&source, &["add", "README"]);
    run_git(&source, &["commit", "-qm", "initial"]);
    run_git(&source, &["remote", "add", "origin", "file:///valid"]);
    let plain = root.join("org/plain");
    let bare = root.join("org/bare");
    let mismatch = root.join("org/mismatch");
    let linked = root.join("org/linked");
    let link = root.join("org/link");
    let valid = root.join("valid");
    fs::create_dir_all(&plain).unwrap();
    fs::create_dir_all(&mismatch).unwrap();
    run_git(&mismatch, &["init", "-q"]);
    run_git(&mismatch, &["remote", "add", "origin", "file:///wrong"]);
    run_git_args(&["init", "--bare", "-q", bare.to_str().unwrap()]);
    run_git(
        &source,
        &["worktree", "add", "-q", linked.to_str().unwrap(), "HEAD"],
    );
    run_git_args(&[
        "clone",
        "-q",
        source.to_str().unwrap(),
        valid.to_str().unwrap(),
    ]);
    run_git(&valid, &["remote", "set-url", "origin", "file:///valid"]);
    std::os::unix::fs::symlink(&outside, valid.join("internal-link")).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/plain\"\n[[repositories]]\nurl = \"github.com/org/bare\"\n[[repositories]]\nurl = \"github.com/org/mismatch\"\n[[repositories]]\nurl = \"github.com/org/linked\"\n[[repositories]]\nurl = \"github.com/org/link\"\n[[repositories]]\nurl = \"file:///valid\"\n",
    )
    .unwrap();

    for reference in [
        "github.com/org/plain",
        "github.com/org/bare",
        "github.com/org/mismatch",
        "github.com/org/linked",
        "github.com/org/link",
        "file:///valid",
    ] {
        let output = lager(
            &config,
            &home,
            &["remove", reference, "--keep-registered", "--yes", "--force"],
        );
        assert_eq!(output.status.code(), Some(1), "{reference}");
    }
    assert!(plain.exists());
    assert!(bare.exists());
    assert!(mismatch.exists());
    assert!(linked.exists());
    assert!(link.exists());
    assert!(valid.exists());
}

#[test]
fn batch_removal_continues_after_failure_and_aggregates_status() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let destination = home.join("repos/org/plain");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&destination).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/plain\"\n[[repositories]]\nurl = \"github.com/org/missing\"\n",
    )
    .unwrap();
    let output = lager(
        &config,
        &home,
        &[
            "remove",
            "github.com/org/plain",
            "github.com/org/missing",
            "--unregister",
            "--yes",
            "--force",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("github.com/org/plain"));
    let remaining = fs::read_to_string(config).unwrap();
    assert!(!remaining.contains("github.com/org/missing"));
    assert!(remaining.contains("github.com/org/plain"));
}

#[test]
fn force_remove_deletes_real_clone_before_unregistering_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let source = temp.path().join("source");
    let remote = temp.path().join("origin.git");
    let root = home.join("repos");
    let destination = root.join("origin");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&source).unwrap();
    run_git(&source, &["init", "-q"]);
    run_git(&source, &["config", "user.email", "lager@example.invalid"]);
    run_git(&source, &["config", "user.name", "lager"]);
    fs::write(source.join("README"), "clone me\n").unwrap();
    run_git(&source, &["add", "README"]);
    run_git(&source, &["commit", "-qm", "initial"]);
    run_git_args(&[
        "clone",
        "-q",
        "--bare",
        source.to_str().unwrap(),
        remote.to_str().unwrap(),
    ]);
    fs::create_dir_all(&root).unwrap();
    run_git_args(&[
        "clone",
        "-q",
        &format!("file://{}", remote.display()),
        destination.to_str().unwrap(),
    ]);
    fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[[repositories]]\nurl = \"file://{}\"\n",
            remote.display()
        ),
    )
    .unwrap();

    fs::write(destination.join("README"), "dirty\n").unwrap();
    let dirty_output = lager(
        &config,
        &home,
        &[
            "remove",
            &format!("file://{}", remote.display()),
            "--keep-registered",
            "--yes",
        ],
    );
    assert_eq!(dirty_output.status.code(), Some(1));
    assert!(destination.exists());
    run_git(&destination, &["checkout", "--", "README"]);

    let attached_output = lager(
        &config,
        &home,
        &[
            "remove",
            &format!("file://{}", remote.display()),
            "--keep-registered",
            "--yes",
        ],
    );
    assert!(
        attached_output.status.success(),
        "{}",
        String::from_utf8_lossy(&attached_output.stderr)
    );
    assert!(!destination.exists());
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("repositories")
    );

    run_git_args(&[
        "clone",
        "-q",
        &format!("file://{}", remote.display()),
        destination.to_str().unwrap(),
    ]);
    let output = lager(
        &config,
        &home,
        &[
            "remove",
            &format!("file://{}", remote.display()),
            "--unregister",
            "--yes",
            "--force",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!destination.exists());
    assert!(!fs::read_to_string(config).unwrap().contains("repositories"));
}

fn run_git(directory: &std::path::Path, args: &[&str]) {
    let output = support::external(directory, "git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_git_args(args: &[&str]) {
    let scope = args
        .iter()
        .map(std::path::Path::new)
        .find(|path| path.is_absolute())
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| std::path::Path::new("."));
    let output = support::external(scope, "git").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
