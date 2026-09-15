use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

fn command(directory: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .current_dir(directory)
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("command output is UTF-8")
        .trim()
        .to_owned()
}

#[test]
fn version_identifies_an_untagged_git_checkout() {
    let output = Command::new(env!("CARGO_BIN_EXE_lager"))
        .arg("--version")
        .output()
        .expect("run lager --version");

    assert!(output.status.success());

    let git = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .expect("read current Git commit");
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
    fs::copy(source.join("build.rs"), checkout.join("build.rs")).expect("copy build script");
    fs::copy(
        source.join("src/cli/args.rs"),
        checkout.join("src/cli/args.rs"),
    )
    .expect("copy CLI arguments");
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
    command(&checkout, "git", &["add", "build.rs", "src/cli/args.rs"]);
    command(
        &checkout,
        "git",
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "version test",
        ],
    );

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/version-e2e");
    let first_version = Command::new("cargo")
        .args(["run", "--offline", "--quiet", "--", "--version"])
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("build first version");
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
    let second_version = Command::new("cargo")
        .args(["run", "--offline", "--quiet", "--", "--version"])
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("build second version");
    assert!(second_version.status.success());

    let commit = command(&checkout, "git", &["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(
        String::from_utf8(second_version.stdout).expect("version output is UTF-8"),
        format!("lager {}-dev.{commit}\n", env!("CARGO_PKG_VERSION"))
    );

    command(&checkout, "git", &["tag", &tag]);
    let tagged_version = Command::new("cargo")
        .args(["run", "--offline", "--quiet", "--", "--version"])
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("build tagged version");
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
    fs::copy(source.join("build.rs"), archive.join("build.rs")).expect("copy build script");
    fs::copy(
        source.join("src/cli/args.rs"),
        archive.join("src/cli/args.rs"),
    )
    .expect("copy CLI arguments");
    fs::remove_dir_all(archive.join(".git")).expect("remove Git metadata");

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/version-e2e-no-git");
    let version = Command::new("cargo")
        .args(["run", "--offline", "--quiet", "--", "--version"])
        .current_dir(&archive)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("build Git-less version");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).expect("version output is UTF-8"),
        format!("lager {}\n", env!("CARGO_PKG_VERSION"))
    );
}
