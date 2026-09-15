use std::{fs, process::Command};

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())?
        .map(|value| value.trim().to_owned())
}

fn watch_git_path(path: &str) {
    if let Some(path) = git_output(&["rev-parse", "--git-path", path]) {
        println!("cargo::rerun-if-changed={path}");
    }
}

fn main() {
    watch_git_path("HEAD");
    watch_git_path("index");
    watch_git_path("packed-refs");
    watch_git_path("refs/tags");

    if let Some(head) = git_output(&["rev-parse", "--git-path", "HEAD"])
        && let Ok(contents) = fs::read_to_string(head)
        && let Some(reference) = contents.strip_prefix("ref: ")
    {
        watch_git_path(reference.trim());
    }

    let package_version = env!("CARGO_PKG_VERSION");
    let release_tag = format!("v{package_version}");
    let version = match git_output(&[
        "describe",
        "--tags",
        "--exact-match",
        "--match",
        &release_tag,
    ]) {
        Some(tag) if tag == release_tag => package_version.to_owned(),
        _ => match git_output(&["rev-parse", "--short=7", "HEAD"]) {
            Some(commit) => format!("{package_version}-dev.{commit}"),
            None => package_version.to_owned(),
        },
    };

    println!("cargo::rustc-env=LAGER_VERSION={version}");
}
