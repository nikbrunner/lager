//! Removal proves that nothing is lost. Whatever it cannot prove is named and needs `--force`.
mod support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    url: String,
    /// The checkout Lager removes: a clone of `origin.git` with `main` pushed.
    checkout: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let source = temp.path().join("source");
        let remote = temp.path().join("origin.git");
        let checkout = home.join("repos/origin");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(home.join("repos")).unwrap();
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q", "-b", "main"]);
        fs::write(source.join("README"), "one\n").unwrap();
        git(&source, &["add", "README"]);
        git(&source, &["commit", "-qm", "initial"]);
        git(
            temp.path(),
            &["clone", "-q", "--bare", "source", "origin.git"],
        );
        let url = format!("file://{}", remote.display());
        git(temp.path(), &["clone", "-q", &url, text(&checkout)]);
        fs::write(
            &config,
            format!("root = \"repos\"\n\n[[repositories]]\nurl = \"{url}\"\n"),
        )
        .unwrap();
        Self {
            temp,
            home,
            config,
            url,
            checkout,
        }
    }

    fn remove(&self, extra: &[&str]) -> Output {
        support::lager(&self.home, &self.config)
            .args(["remove", &self.url, "--keep-registered", "--yes"])
            .args(extra)
            .output()
            .unwrap()
    }

    /// Removal without `--force` is refused, names every part of `named` and points at
    /// `--force`; with `--force` the same checkout goes and the accepted parts are printed.
    fn assert_named(&self, case: &str, named: &[&str]) {
        let refused = self.remove(&[]);
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert_eq!(refused.status.code(), Some(1), "{case}: {stderr}");
        assert!(self.checkout.exists(), "{case}: checkout was deleted");
        for part in named
            .iter()
            .chain(&["rerun with --force to accept these losses"])
        {
            assert!(stderr.contains(part), "{case}: {part:?} not in {stderr}");
        }
        let forced = self.remove(&["--force"]);
        let stderr = String::from_utf8_lossy(&forced.stderr);
        assert!(forced.status.success(), "{case}: {stderr}");
        assert!(!self.checkout.exists(), "{case}: checkout stayed");
        assert!(
            stderr.contains("removing with --force; this is lost: "),
            "{case}: {stderr}"
        );
        // The forced notice drops the hints: the commands they name stop applying.
        for part in named {
            let loss = part.split(" (git ").next().unwrap();
            assert!(stderr.contains(loss), "{case}: {loss:?} not in {stderr}");
            assert!(!stderr.contains(" (git "), "{case}: hint in {stderr}");
        }
    }

    fn assert_removed(&self, case: &str) {
        let output = self.remove(&[]);
        assert!(
            output.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!self.checkout.exists(), "{case}: checkout stayed");
    }

    fn commit(&self, directory: &Path, file: &str) {
        fs::write(directory.join(file), format!("{file}\n")).unwrap();
        git(directory, &["add", file]);
        git(directory, &["commit", "-qm", file]);
    }

    /// A second clone that pushes one more commit to `main`.
    fn advance_remote(&self) {
        let other = self.temp.path().join("other");
        git(self.temp.path(), &["clone", "-q", &self.url, "other"]);
        self.commit(&other, "remote-change");
        git(&other, &["push", "-q"]);
    }

    /// Adds a pushed submodule at `sub` with one tracked file, populated in the checkout.
    fn submodule(&self) -> PathBuf {
        let library = self.library("library");
        self.add_submodule(&self.checkout, &library, "sub");
        git(&self.checkout, &["push", "-q"]);
        self.checkout.join("sub")
    }

    /// A repository with one commit on `main` that submodules clone from.
    fn library(&self, name: &str) -> PathBuf {
        let library = self.temp.path().join(name);
        fs::create_dir_all(&library).unwrap();
        git(&library, &["init", "-q", "-b", "main"]);
        self.commit(&library, "lib");
        library
    }

    /// Adds `library` as the submodule `path` of `directory` and commits it.
    fn add_submodule(&self, directory: &Path, library: &Path, path: &str) {
        let url = format!("file://{}", library.display());
        git(
            directory,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                path,
            ],
        );
        git(directory, &["commit", "-qm", &format!("add {path}")]);
    }

    /// Replaces the checkout with a fresh clone made with `args`.
    fn reclone(&self, args: &[&str]) {
        fs::remove_dir_all(&self.checkout).unwrap();
        let mut command = vec!["-c", "protocol.file.allow=always", "clone", "-q"];
        command.extend(args);
        command.extend([self.url.as_str(), text(&self.checkout)]);
        git(self.temp.path(), &command);
    }

    /// A linked worktree of the checkout on a pushed branch.
    fn worktree(&self) -> PathBuf {
        let path = self.home.join("repos/origin.worktrees/side");
        git(
            &self.checkout,
            &["worktree", "add", "-q", "-b", "side", text(&path)],
        );
        git(&path, &["push", "-q", "-u", "origin", "side"]);
        path
    }

    fn remove_worktree(&self, path: &Path, extra: &[&str]) -> Output {
        support::lager(&self.home, &self.config)
            .current_dir(&self.checkout)
            .args(["worktree", "remove", text(path), "--yes"])
            .args(extra)
            .output()
            .unwrap()
    }

    fn assert_worktree_named(&self, case: &str, path: &Path, named: &str) -> String {
        let refused = self.remove_worktree(path, &[]);
        let shown = format!(
            "{}{}",
            String::from_utf8_lossy(&refused.stdout),
            String::from_utf8_lossy(&refused.stderr)
        );
        assert_eq!(refused.status.code(), Some(1), "{case}: {shown}");
        assert!(path.exists(), "{case}: worktree was deleted");
        for part in [named, "rerun with --force to accept these losses"] {
            assert!(shown.contains(part), "{case}: {part:?} not in {shown}");
        }
        let forced = self.remove_worktree(path, &["--force"]);
        let forced_out = String::from_utf8_lossy(&forced.stdout);
        assert!(
            forced.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&forced.stderr)
        );
        assert!(
            forced_out.contains(named),
            "{case}: {named:?} not in {forced_out}"
        );
        assert!(!path.exists(), "{case}: worktree stayed");
        shown
    }
}

/// Runs every case and then fails with the names of all that failed, so one run shows each.
fn each_case<S, const N: usize>(
    cases: [(&'static str, (S, &'static str)); N],
    run: impl Fn(&'static str, (S, &'static str)),
) {
    let failed: Vec<&str> = cases
        .into_iter()
        .filter_map(|(case, input)| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(case, input)))
                .is_err()
                .then_some(case)
        })
        .collect();
    assert!(failed.is_empty(), "failed cases: {failed:?}");
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn git_command(directory: &Path, args: &[&str]) -> std::process::Command {
    let mut command = support::external(directory, "git");
    command
        .current_dir(directory)
        .env("GIT_AUTHOR_NAME", "lager")
        .env("GIT_AUTHOR_EMAIL", "lager@example.invalid")
        .env("GIT_COMMITTER_NAME", "lager")
        .env("GIT_COMMITTER_EMAIL", "lager@example.invalid")
        .args(args);
    command
}

fn git(directory: &Path, args: &[&str]) {
    let output = git_command(directory, args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(directory: &Path, args: &[&str]) -> String {
    let output = git_command(directory, args).output().unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Stops `git am` on its first patch, which conflicts; the patches then live only in the
/// Git directory of `directory`.
fn stop_am(fixture: &Fixture, directory: &Path) {
    let scratch = fixture.temp.path().join("patches");
    git(
        fixture.temp.path(),
        &["clone", "-q", &fixture.url, "patches"],
    );
    fs::write(scratch.join("README"), "theirs\n").unwrap();
    git(&scratch, &["commit", "-qam", "theirs"]);
    let patch = git_text(&scratch, &["format-patch", "--stdout", "-1"]);
    fs::write(directory.join("README"), "ours\n").unwrap();
    git(directory, &["commit", "-qam", "ours"]);
    git(directory, &["push", "-q"]);
    let mut am = git_command(directory, &["am"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    am.stdin
        .take()
        .unwrap()
        .write_all(patch.as_bytes())
        .unwrap();
    assert!(!am.wait().unwrap().success(), "git am did not stop");
    assert_eq!(git_text(directory, &["status", "--porcelain"]), "");
}

#[test]
fn ordinary_checkouts_are_removed_without_force() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup); 7] = [
        ("fresh clone", |_| {}),
        ("after git maintenance prefetch", |fixture| {
            fixture.advance_remote();
            git(
                &fixture.checkout,
                &["maintenance", "run", "--task=prefetch", "--quiet"],
            );
            assert!(
                git_text(&fixture.checkout, &["for-each-ref", "refs/prefetch/"]).contains("main")
            );
        }),
        ("after fetch", |fixture| {
            fixture.advance_remote();
            git(&fixture.checkout, &["fetch", "-q"]);
            git(&fixture.checkout, &["merge", "-q", "--ff-only"]);
            assert!(fixture.checkout.join(".git/FETCH_HEAD").exists());
        }),
        ("after a completed rebase", |fixture| {
            fixture.advance_remote();
            fixture.commit(&fixture.checkout, "local-change");
            git(&fixture.checkout, &["pull", "-q", "--rebase"]);
            git(&fixture.checkout, &["push", "-q"]);
            assert!(fixture.checkout.join(".git/ORIG_HEAD").exists());
        }),
        ("after a completed merge", |fixture| {
            fixture.advance_remote();
            fixture.commit(&fixture.checkout, "local-change");
            git(
                &fixture.checkout,
                &["pull", "-q", "--no-rebase", "--no-edit"],
            );
            git(&fixture.checkout, &["push", "-q"]);
        }),
        ("packed refs", |fixture| {
            git(&fixture.checkout, &["pack-refs", "--all"]);
            git(&fixture.checkout, &["gc", "-q"]);
            assert!(fixture.checkout.join(".git/packed-refs").exists());
        }),
        ("sample hooks only", |fixture| {
            let hooks = fixture.checkout.join(".git/hooks");
            fs::create_dir_all(&hooks).unwrap();
            fs::write(hooks.join("pre-commit.sample"), "#!/bin/sh\n").unwrap();
        }),
    ];
    each_case(
        cases.map(|(case, setup)| (case, (setup, ""))),
        |case, (setup, _)| {
            let fixture = Fixture::new();
            setup(&fixture);
            fixture.assert_removed(case);
        },
    );
}

#[test]
fn commits_on_another_branch_or_a_tag_are_named() {
    let fixture = Fixture::new();
    let checkout = &fixture.checkout;
    git(checkout, &["switch", "-qc", "feature"]);
    fixture.commit(checkout, "feature-work");
    git(checkout, &["switch", "-q", "--detach"]);
    fixture.commit(checkout, "tagged-work");
    git(checkout, &["tag", "keep"]);
    git(checkout, &["switch", "-q", "main"]);
    fixture.assert_named(
        "other branch and tag",
        &["commits not on any remote: branch feature, tag keep"],
    );
}

#[test]
fn a_branch_tracking_a_local_branch_is_named() {
    let fixture = Fixture::new();
    let checkout = &fixture.checkout;
    fixture.commit(checkout, "never-pushed");
    git(checkout, &["switch", "-qc", "topic", "--track", "main"]);
    assert_eq!(
        git_text(checkout, &["config", "branch.topic.remote"]).trim(),
        "."
    );
    fixture.assert_named(
        "local upstream",
        &["commits not on any remote: branch main, branch topic"],
    );
}

#[test]
fn submodule_edits_hidden_by_an_ignore_setting_are_named() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup); 3] = [
        ("submodule.sub.ignore", |fixture| {
            git(
                &fixture.checkout,
                &["config", "submodule.sub.ignore", "dirty"],
            );
        }),
        (".gitmodules ignore", |fixture| {
            let checkout = &fixture.checkout;
            git(
                checkout,
                &["config", "-f", ".gitmodules", "submodule.sub.ignore", "all"],
            );
            git(checkout, &["commit", "-qam", "ignore sub"]);
            git(checkout, &["push", "-q"]);
        }),
        ("diff.ignoreSubmodules", |fixture| {
            git(
                &fixture.checkout,
                &["config", "diff.ignoreSubmodules", "dirty"],
            );
        }),
    ];
    each_case(
        cases.map(|(case, setup)| (case, (setup, ""))),
        |case, (setup, _)| {
            let fixture = Fixture::new();
            let sub = fixture.submodule();
            setup(&fixture);
            fs::write(sub.join("lib"), "edited\n").unwrap();
            fs::write(sub.join("scratch"), "untracked\n").unwrap();
            assert_eq!(
                git_text(&fixture.checkout, &["status", "--porcelain"]),
                "",
                "{case}"
            );
            fixture.assert_named(case, &["unstaged changes"]);
        },
    );
}

#[test]
fn state_inside_a_submodule_repository_is_named() {
    type Setup = fn(&Fixture, &Path);
    let cases: [(&str, Setup, &str); 6] = [
        (
            "stash in submodule",
            |_, sub| {
                fs::write(sub.join("lib"), "edited\n").unwrap();
                git(sub, &["stash", "-q"]);
            },
            "submodule sub: stashes",
        ),
        (
            "unpushed branch in submodule",
            |fixture, sub| {
                git(sub, &["switch", "-qc", "local"]);
                fixture.commit(sub, "local-only");
                git(sub, &["switch", "-q", "--detach", "main"]);
            },
            "submodule sub: commits not on any remote: branch local",
        ),
        (
            "deinitialized submodule",
            |fixture, _| {
                git(
                    &fixture.checkout,
                    &["submodule", "deinit", "-q", "-f", "sub"],
                )
            },
            "submodule repositories in .git/modules: sub",
        ),
        (
            // The superproject pushed the new gitlink, so its own status and refs are clean.
            "commit only at the submodule HEAD",
            |fixture, sub| {
                git(sub, &["switch", "-q", "--detach"]);
                fixture.commit(sub, "detached-work");
                git(&fixture.checkout, &["commit", "-qam", "bump sub"]);
                git(&fixture.checkout, &["push", "-q"]);
                assert_eq!(git_text(&fixture.checkout, &["status", "--porcelain"]), "");
            },
            "submodule sub: commits not on any remote: HEAD",
        ),
        (
            "untracked file in submodule",
            |_, sub| fs::write(sub.join("scratch"), "untracked\n").unwrap(),
            "submodule sub: untracked files",
        ),
        (
            "unknown entry in the submodule's Git directory",
            |fixture, _| {
                fs::write(fixture.checkout.join(".git/modules/sub/planted"), "x\n").unwrap()
            },
            "submodule sub: unknown entries in .git/modules/sub: planted",
        ),
    ];
    each_case(
        cases.map(|(case, setup, named)| (case, (setup, named))),
        |case, (setup, named)| {
            let fixture = Fixture::new();
            let sub = fixture.submodule();
            setup(&fixture, &sub);
            assert_named_once(&fixture, case);
            fixture.assert_named(case, &[named]);
        },
    );
}

#[test]
fn a_stopped_am_in_a_checkout_is_named() {
    let fixture = Fixture::new();
    stop_am(&fixture, &fixture.checkout);
    fixture.assert_named(
        "stopped am",
        &["stopped git am (git am --continue or git am --abort)"],
    );
}

#[test]
fn state_git_status_does_not_show_is_named() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup, &str); 8] = [
        (
            "skip-worktree edit",
            |fixture| {
                git(
                    &fixture.checkout,
                    &["update-index", "--skip-worktree", "README"],
                );
                fs::write(fixture.checkout.join("README"), "hidden\n").unwrap();
            },
            "tracked files hidden from git status",
        ),
        (
            "assume-unchanged edit",
            |fixture| {
                git(
                    &fixture.checkout,
                    &["update-index", "--assume-unchanged", "README"],
                );
                fs::write(fixture.checkout.join("README"), "hidden\n").unwrap();
            },
            "tracked files hidden from git status",
        ),
        (
            "repository nested in a tracked directory",
            |fixture| {
                let checkout = &fixture.checkout;
                fs::create_dir_all(checkout.join("dir")).unwrap();
                fixture.commit(checkout, "dir/tracked");
                git(checkout, &["push", "-q"]);
                let inner = checkout.join("dir/inner");
                fs::create_dir_all(&inner).unwrap();
                git(&inner, &["init", "-q", "-b", "main"]);
                fixture.commit(&inner, "work");
            },
            "nested repository dir/inner",
        ),
        (
            "files in an unpopulated gitlink directory",
            |fixture| {
                let sub = fixture.submodule();
                let checkout = &fixture.checkout;
                git(checkout, &["submodule", "deinit", "-q", "-f", "sub"]);
                fs::remove_dir_all(checkout.join(".git/modules")).unwrap();
                fs::write(sub.join("stray"), "unreported\n").unwrap();
                assert_eq!(git_text(checkout, &["status", "--porcelain"]), "");
            },
            "sub: unpopulated submodule directory holds files Git does not report",
        ),
        (
            "rebase-merge autostash",
            |fixture| {
                let state = fixture.checkout.join(".git/rebase-merge");
                fs::create_dir_all(&state).unwrap();
                fs::write(state.join("autostash"), "0123\n").unwrap();
            },
            "autostash of a stopped rebase",
        ),
        (
            "rebase-apply autostash",
            |fixture| {
                let state = fixture.checkout.join(".git/rebase-apply");
                fs::create_dir_all(&state).unwrap();
                fs::write(state.join("autostash"), "0123\n").unwrap();
            },
            "autostash of a stopped rebase",
        ),
        (
            "merge autostash",
            |fixture| fs::write(fixture.checkout.join(".git/MERGE_AUTOSTASH"), "0123\n").unwrap(),
            "autostash of a stopped merge",
        ),
        (
            "checkout autostash",
            |fixture| {
                fs::write(
                    fixture.checkout.join(".git/CHECKOUT_AUTOSTASH_HEAD"),
                    "0123\n",
                )
                .unwrap()
            },
            "autostash of an interrupted checkout",
        ),
    ];
    each_case(
        cases.map(|(case, setup, named)| (case, (setup, named))),
        |case, (setup, named)| {
            let fixture = Fixture::new();
            setup(&fixture);
            fixture.assert_named(case, &[named]);
        },
    );
}

#[test]
fn unknown_entries_in_the_git_directory_are_named() {
    let fixture = Fixture::new();
    let git_dir = fixture.checkout.join(".git");
    fs::create_dir(git_dir.join("toolstate")).unwrap();
    fs::write(git_dir.join("toolstate/session"), "state\n").unwrap();
    fs::write(git_dir.join("lint-staged"), "state\n").unwrap();
    fs::write(git_dir.join("nano-staged.patch"), "state\n").unwrap();
    fixture.assert_named(
        "planted entries",
        &["unknown entries in .git: lint-staged, nano-staged.patch, toolstate"],
    );
}

#[test]
fn refusal_and_forced_removal_name_every_warning() {
    let fixture = Fixture::new();
    fs::write(fixture.checkout.join("scratch"), "untracked\n").unwrap();
    fs::write(fixture.checkout.join(".git/planted"), "state\n").unwrap();

    let refused = fixture.remove(&[]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&refused.stderr).trim(),
        format!(
            "lager: {}: unsafe local state: untracked files; unknown entries in .git: planted; rerun with --force to accept these losses",
            fixture.url
        )
    );
    assert!(fixture.checkout.exists());

    let forced = fixture.remove(&["--force"]);
    assert!(forced.status.success());
    assert_eq!(
        String::from_utf8_lossy(&forced.stderr).trim(),
        format!(
            "lager: {}: removing with --force; this is lost: untracked files; unknown entries in .git: planted",
            fixture.url
        )
    );
    assert_eq!(
        String::from_utf8_lossy(&forced.stdout).trim(),
        format!("{}: removed", fixture.url)
    );
    assert!(!fixture.checkout.exists());
}

#[test]
fn an_ordinary_linked_worktree_is_removed_without_force() {
    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    fixture.advance_remote();
    fixture.commit(&worktree, "side-work");
    git(&worktree, &["fetch", "-q"]);
    git(&worktree, &["rebase", "-q", "origin/main"]);
    git(&worktree, &["merge", "-q", "origin/main"]);
    git(&worktree, &["push", "-q", "--force"]);
    let output = fixture.remove_worktree(&worktree, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!worktree.exists());
}

#[test]
fn a_stopped_am_in_a_linked_worktree_is_named() {
    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    stop_am(&fixture, &worktree);
    let shown = fs::canonicalize(&worktree).unwrap();
    let shown = text(&shown);
    fixture.assert_worktree_named(
        "stopped am",
        &worktree,
        &format!("stopped git am (git -C {shown} am --continue or --abort)"),
    );
}

#[test]
fn unknown_entries_in_a_linked_git_directory_are_named() {
    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    fs::write(
        fixture.checkout.join(".git/worktrees/side/planted"),
        "state\n",
    )
    .unwrap();
    let git_dir = fs::canonicalize(fixture.checkout.join(".git/worktrees/side")).unwrap();
    let shown = fixture.assert_worktree_named(
        "planted entry",
        &worktree,
        &format!("unknown entries in {}: planted", git_dir.display()),
    );
    assert!(
        shown.contains(
            "error: the listed state would be lost; rerun with --force to accept these losses"
        ),
        "{shown}"
    );
}

#[cfg(unix)]
#[test]
fn a_git_call_that_fails_during_the_proof_never_reads_as_safe() {
    use std::os::unix::fs::PermissionsExt;
    let cases = [
        "for-each-ref",
        "rev-list --all",
        "status --porcelain=v1",
        "ls-files -v",
        "ls-files -z --stage",
    ];
    each_case(cases.map(|call| (call, ((), ""))), |call, _| {
        let fixture = Fixture::new();
        let shim = fixture.temp.path().join("shim");
        fs::create_dir_all(&shim).unwrap();
        fs::write(
            shim.join("git"),
            format!(
                "#!/bin/sh\ncase \" $* \" in *\" {call} \"*) echo boom >&2; exit 3;; esac\nexec '{}' \"$@\"\n",
                support::real_tool("git").display()
            ),
        )
        .unwrap();
        fs::set_permissions(shim.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&shim))
            .args(["remove", &fixture.url, "--keep-registered", "--yes"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{call}: {stderr}");
        assert!(fixture.checkout.exists(), "{call}: checkout was deleted");
        assert!(stderr.contains("boom"), "{call}: {stderr}");
    });
}

/// A single finding in the submodule `sub` names it once.
fn assert_named_once(fixture: &Fixture, case: &str) {
    let refused = fixture.remove(&[]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let named = stderr
        .match_indices(" sub")
        .filter(|(at, _)| !stderr[at + 4..].starts_with('m'))
        .count();
    assert_eq!(named, 1, "{case}: {stderr}");
}

#[test]
fn submodules_that_hold_nothing_unpushed_are_removed_without_force() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup); 5] = [
        ("submodule add", |fixture| {
            fixture.submodule();
        }),
        ("recursive clone", |fixture| {
            fixture.submodule();
            fixture.reclone(&["--recurse-submodules"]);
            assert!(fixture.checkout.join("sub/lib").exists());
        }),
        ("clone, then submodule update --init", |fixture| {
            fixture.submodule();
            fixture.reclone(&[]);
            git(
                &fixture.checkout,
                &[
                    "-c",
                    "protocol.file.allow=always",
                    "submodule",
                    "update",
                    "-q",
                    "--init",
                ],
            );
            assert!(fixture.checkout.join("sub/lib").exists());
        }),
        ("plain clone, submodule never initialized", |fixture| {
            fixture.submodule();
            fixture.reclone(&[]);
            assert!(!fixture.checkout.join("sub/lib").exists());
        }),
        ("recursive clone with a nested submodule", |fixture| {
            let inner = fixture.library("inner-library");
            let library = fixture.library("library");
            fixture.add_submodule(&library, &inner, "inner");
            fixture.add_submodule(&fixture.checkout, &library, "sub");
            git(&fixture.checkout, &["push", "-q"]);
            fixture.reclone(&["--recurse-submodules"]);
            assert!(fixture.checkout.join("sub/inner/lib").exists());
        }),
    ];
    each_case(
        cases.map(|(case, setup)| (case, (setup, ""))),
        |case, (setup, _)| {
            let fixture = Fixture::new();
            setup(&fixture);
            fixture.assert_removed(case);
        },
    );
}

#[test]
fn a_commit_in_a_nested_submodule_is_named_with_its_path() {
    let fixture = Fixture::new();
    let inner = fixture.library("inner-library");
    let library = fixture.library("library");
    fixture.add_submodule(&library, &inner, "inner");
    fixture.add_submodule(&fixture.checkout, &library, "sub");
    git(&fixture.checkout, &["push", "-q"]);
    fixture.reclone(&["--recurse-submodules"]);
    let nested = fixture.checkout.join("sub/inner");
    git(&nested, &["switch", "-qc", "local"]);
    fixture.commit(&nested, "local-only");
    git(&nested, &["switch", "-q", "--detach", "main"]);
    let refused = fixture.remove(&[]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        stderr.matches("sub/inner").count(),
        1,
        "named once: {stderr}"
    );
    fixture.assert_named(
        "nested",
        &["submodule sub/inner: commits not on any remote: branch local"],
    );
}

#[test]
fn git_directory_entries_that_hold_nothing_are_removed_without_force() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup); 7] = [
        ("opencode", |fixture| {
            fs::write(
                fixture.checkout.join(".git/opencode"),
                "0123456789abcdef0123456789abcdef01234567\n",
            )
            .unwrap()
        }),
        ("gc.log", |fixture| {
            fs::write(fixture.checkout.join(".git/gc.log"), "warning\n").unwrap()
        }),
        ("reftable", |fixture| {
            fixture.reclone(&["--ref-format=reftable"])
        }),
        ("fsmonitor daemon state", |fixture| {
            let git_dir = fixture.checkout.join(".git");
            fs::create_dir_all(git_dir.join("fsmonitor--daemon/cookies")).unwrap();
            fs::write(git_dir.join("fsmonitor--daemon/cookies/1"), "x").unwrap();
            fs::write(git_dir.join("fsmonitor--daemon.ipc"), "x").unwrap();
        }),
        ("CLAUDE_BASE", |fixture| {
            fs::write(fixture.checkout.join(".git/CLAUDE_BASE"), "0123abcd\n").unwrap()
        }),
        ("empty tool directory", |fixture| {
            fs::create_dir(fixture.checkout.join(".git/lint-staged")).unwrap()
        }),
        ("empty file after rerere", |fixture| {
            fs::write(fixture.checkout.join(".git/MERGE_RR"), "").unwrap()
        }),
    ];
    each_case(
        cases.map(|(case, setup)| (case, (setup, ""))),
        |case, (setup, _)| {
            let fixture = Fixture::new();
            setup(&fixture);
            fixture.assert_removed(case);
        },
    );

    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    let git_dir = fixture.checkout.join(".git/worktrees/side");
    fs::write(git_dir.join("CLAUDE_BASE"), "0123abcd\n").unwrap();
    fs::create_dir(git_dir.join("lint-staged")).unwrap();
    let output = fixture.remove_worktree(&worktree, &[]);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_per_worktree_ref_in_a_linked_worktree_is_named() {
    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    git(&worktree, &["switch", "-q", "--detach"]);
    fixture.commit(&worktree, "kept-only-by-a-worktree-ref");
    git(&worktree, &["update-ref", "refs/worktree/keep", "HEAD"]);
    git(&worktree, &["switch", "-q", "side"]);
    // Only the ref holds the commit, and nothing else in the worktree is a loss.
    fixture.assert_worktree_named(
        "refs/worktree",
        &worktree,
        "per-worktree ref refs/worktree/keep",
    );
}

#[test]
fn worktree_losses_show_one_loss_per_line() {
    let fixture = Fixture::new();
    let worktree = fixture.worktree();
    git(&worktree, &["update-ref", "refs/worktree/keep", "HEAD"]);
    fs::write(worktree.join("scratch"), "untracked\n").unwrap();
    let shown = fixture.assert_worktree_named(
        "files and a ref",
        &worktree,
        "per-worktree ref refs/worktree/keep",
    );
    // The files on one line, then each named state, under the value column.
    assert!(
        shown.contains(
            "  losses    1 untracked file\n            per-worktree ref refs/worktree/keep\n"
        ),
        "{shown}"
    );
}

#[test]
fn the_ref_warning_names_eight_refs_and_counts_the_rest() {
    let fixture = Fixture::new();
    fixture.commit(&fixture.checkout, "local");
    for number in 0..10 {
        git(&fixture.checkout, &["branch", &format!("b{number}")]);
    }
    git(&fixture.checkout, &["reset", "-q", "--hard", "origin/main"]);
    fixture.assert_named(
        "ten branches",
        &["commits not on any remote: branch b0, branch b1, branch b2, branch b3, branch b4, branch b5, branch b6, branch b7, 2 more"],
    );

    let fixture = Fixture::new();
    let blob = git_text(&fixture.checkout, &["hash-object", "-w", "README"]);
    git(&fixture.checkout, &["tag", "blobtag", blob.trim()]);
    fixture.assert_named("tag on a blob", &["commits not on any remote: tag blobtag"]);
}

#[test]
fn the_ref_warning_sorts_numbers_naturally_and_reads_the_same_twice() {
    let fixture = Fixture::new();
    fixture.commit(&fixture.checkout, "local");
    for name in ["feature-10", "feature-2", "feature-1", "feature-11"] {
        git(&fixture.checkout, &["branch", name]);
    }
    git(&fixture.checkout, &["reset", "-q", "--hard", "origin/main"]);
    let first = fixture.remove(&[]).stderr;
    let second = fixture.remove(&[]).stderr;
    assert_eq!(first, second);
    let shown = String::from_utf8_lossy(&first);
    assert!(
        shown.contains(
            "commits not on any remote: branch feature-1, branch feature-2, branch feature-10, branch feature-11"
        ),
        "{shown}"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_rebase_directory_still_names_its_autostash() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let state = fixture.checkout.join(".git/rebase-merge");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("autostash"), "0123\n").unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();
    let refused = fixture.remove(&[]);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("autostash of a stopped rebase"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn one_inspection_runs_git_status_once() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let shim = fixture.temp.path().join("shim");
    let log = fixture.temp.path().join("calls");
    fs::create_dir_all(&shim).unwrap();
    fs::write(
        shim.join("git"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            support::real_tool("git").display()
        ),
    )
    .unwrap();
    fs::set_permissions(shim.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&shim))
        .args(["remove", &fixture.url, "--keep-registered", "--yes"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&log).unwrap();
    // `remove` inspects twice: before it asks and right before it deletes.
    assert_eq!(calls.matches("status --porcelain").count(), 2, "{calls}");
}

#[test]
fn worktree_remove_help_names_every_loss_force_accepts() {
    let fixture = Fixture::new();
    let output = support::lager(&fixture.home, &fixture.config)
        .args(["worktree", "remove", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("Accept the listed losses; safety checks still apply"),
        "{help}"
    );
    assert!(help.contains("named Git state"), "{help}");
}

#[test]
fn a_submodule_moved_off_its_recorded_commit_is_named_under_ignore_all() {
    let fixture = Fixture::new();
    let library = fixture.library("library");
    fixture.commit(&library, "second");
    fixture.add_submodule(&fixture.checkout, &library, "sub");
    let checkout = &fixture.checkout;
    git(
        checkout,
        &["config", "-f", ".gitmodules", "submodule.sub.ignore", "all"],
    );
    git(checkout, &["commit", "-qam", "ignore sub"]);
    git(checkout, &["push", "-q"]);
    git(&checkout.join("sub"), &["checkout", "-q", "HEAD~1"]);
    assert_eq!(git_text(checkout, &["status", "--porcelain"]), "");
    fixture.assert_named("gitlink moved", &["unstaged changes"]);
}

#[test]
fn a_submodule_left_at_a_new_commit_is_named_once_beside_its_status_line() {
    let fixture = Fixture::new();
    let sub = fixture.submodule();
    git(&sub, &["switch", "-q", "--detach"]);
    fixture.commit(&sub, "new-commit");
    let refused = fixture.remove(&[]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&refused.stderr).trim(),
        format!(
            "lager: {}: unsafe local state: unstaged changes; submodule sub: commits not on any remote: HEAD; rerun with --force to accept these losses",
            fixture.url
        )
    );
    assert!(fixture.checkout.exists());
}
