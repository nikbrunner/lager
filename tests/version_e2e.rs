use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use tempfile::tempdir;

fn checked_output(command: &mut Command) -> Output {
    command.env("GIT_EDITOR", "true");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed ({}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn command(directory: &Path, program: &str, args: &[&str]) -> String {
    let output = checked_output(Command::new(program).args(args).current_dir(directory));

    String::from_utf8(output.stdout)
        .expect("command output is UTF-8")
        .trim()
        .to_owned()
}

fn overlay_current_sources(source: &Path, destination: &Path) {
    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src/application/inventory.rs",
        "src/application/mod.rs",
        "src/cli/args.rs",
        "src/cli/controller.rs",
        "src/cli/inventory.rs",
        "src/cli/inventory/keys.rs",
        "src/cli/mod.rs",
        "src/infrastructure/config.rs",
        "src/infrastructure/inventory.rs",
        "src/infrastructure/mod.rs",
    ] {
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create source overlay parent");
        }
        fs::copy(source.join(relative), target)
            .unwrap_or_else(|error| panic!("copy {relative} into version fixture: {error}"));
    }
}

#[test]
fn version_identifies_an_untagged_git_checkout() {
    let output = checked_output(Command::new(env!("CARGO_BIN_EXE_lager")).arg("--version"));

    assert!(output.status.success());

    let git = checked_output(Command::new("git").args(["rev-parse", "--short=7", "HEAD"]));
    assert!(git.status.success());
    let commit = String::from_utf8(git.stdout)
        .expect("Git commit is UTF-8")
        .trim()
        .to_owned();

    assert_eq!(
        String::from_utf8(output.stdout).expect("version output is UTF-8"),
        format!("lager {}-dev.{commit}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_changes_after_an_empty_commit() {
    let checkout = tempdir().expect("create temporary checkout");
    let checkout = checkout.path().join("lager");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));

    command(
        source,
        "git",
        &[
            "clone",
            "--quiet",
            ".",
            checkout.to_str().expect("checkout path is UTF-8"),
        ],
    );
    overlay_current_sources(source, &checkout);
    command(&checkout, "git", &["config", "user.name", "Lager test"]);
    command(
        &checkout,
        "git",
        &["config", "user.email", "lager@example.invalid"],
    );
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    if !command(&checkout, "git", &["tag", "--list", &tag]).is_empty() {
        command(&checkout, "git", &["tag", "--delete", &tag]);
    }
    command(&checkout, "git", &["add", "."]);
    command(
        &checkout,
        "git",
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "version test",
        ],
    );

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/version-e2e");
    let first_version = checked_output(
        Command::new("cargo")
            .args(["run", "--offline", "--quiet", "--", "--version"])
            .current_dir(&checkout)
            .env("CARGO_TARGET_DIR", &target),
    );
    assert!(first_version.status.success());

    command(
        &checkout,
        "git",
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "next",
        ],
    );
    let second_version = checked_output(
        Command::new("cargo")
            .args(["run", "--offline", "--quiet", "--", "--version"])
            .current_dir(&checkout)
            .env("CARGO_TARGET_DIR", &target),
    );
    assert!(second_version.status.success());

    let commit = command(&checkout, "git", &["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(
        String::from_utf8(second_version.stdout).expect("version output is UTF-8"),
        format!("lager {}-dev.{commit}\n", env!("CARGO_PKG_VERSION"))
    );

    command(&checkout, "git", &["tag", &tag]);
    let tagged_version = checked_output(
        Command::new("cargo")
            .args(["run", "--offline", "--quiet", "--", "--version"])
            .current_dir(&checkout)
            .env("CARGO_TARGET_DIR", &target),
    );
    assert!(tagged_version.status.success());
    assert_eq!(
        String::from_utf8(tagged_version.stdout).expect("version output is UTF-8"),
        format!("lager {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_falls_back_to_package_version_without_git_metadata() {
    let archive = tempdir().expect("create temporary archive");
    let archive = archive.path().join("lager");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));

    command(
        source,
        "git",
        &[
            "clone",
            "--quiet",
            ".",
            archive.to_str().expect("archive path is UTF-8"),
        ],
    );
    overlay_current_sources(source, &archive);
    fs::remove_dir_all(archive.join(".git")).expect("remove Git metadata");

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/version-e2e-no-git");
    let version = checked_output(
        Command::new("cargo")
            .args(["run", "--offline", "--quiet", "--", "--version"])
            .current_dir(&archive)
            .env("CARGO_TARGET_DIR", target),
    );
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).expect("version output is UTF-8"),
        format!("lager {}\n", env!("CARGO_PKG_VERSION"))
    );
}
