#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    state: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new(providers: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (bin, state) = support::install_fake_gh(temp.path());
        let config = temp.path().join("config.toml");
        std::fs::write(&config, format!("root = \"repos\"\n\n{providers}")).unwrap();
        Self {
            log: temp.path().join("gh-args"),
            _temp: temp,
            home,
            bin,
            config,
            state,
        }
    }

    fn github(actions: &str) -> Self {
        Self::new(&format!(
            "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [{actions}]\n"
        ))
    }

    fn run(&self, args: &[&str]) -> Output {
        support::lager_with_path(&self.home, &self.config, Some(&self.bin))
            .env("GH_ARGS_LOG", &self.log)
            .env("GH_STATE", &self.state)
            .args(args)
            .output()
            .unwrap()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn set(&self, name: &str, value: &str) {
        std::fs::write(self.state.join(name), value).unwrap();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[track_caller]
fn assert_outcome(output: &Output, code: i32, stdout: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {} stderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(
        text(&output.stdout),
        stdout,
        "stderr: {}",
        text(&output.stderr)
    );
}

fn state(path: &Path, name: &str) -> String {
    std::fs::read_to_string(path.join(name)).unwrap()
}

#[test]
fn an_action_missing_from_remote_actions_never_calls_gh() {
    let fixture = Fixture::github("\"visibility\"");
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains(
            "`archive` is not enabled for github.com; add it to remote_actions under [providers.\"github.com\"]"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_provider_without_remote_actions_allows_nothing() {
    let fixture = Fixture::new("[providers.\"github.com\"]\npreset = \"github\"\n");
    let output = fixture.run(&["visibility", "org/demo", "private", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(text(&output.stderr).contains("`visibility` is not enabled for github.com"));
    assert!(fixture.calls().is_empty());
}

#[test]
fn remote_actions_on_a_bitbucket_preset_is_a_config_error() {
    let fixture = Fixture::new(
        "[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\nremote_actions = [\"archive\"]\n",
    );
    let output = fixture.run(&["archive", "bitbucket.org/org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("remote_actions is only supported for the github preset"),
        "{}",
        text(&output.stderr)
    );
    assert!(!text(&output.stderr).contains("bitbucket.org/org/demo:"));
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_missing_gh_keeps_the_install_message() {
    let fixture = Fixture::github("\"archive\"");
    std::fs::remove_file(fixture.bin.join("gh")).unwrap();
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("GitHub provider requires `gh`"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn gh_logged_out_names_the_login_command_before_any_change() {
    let fixture = Fixture::github("\"archive\"");
    fixture.set("logged-out", "");
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("gh auth login --hostname github.com"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(
        fixture.calls(),
        ["github.com auth status --hostname github.com"]
    );
}

#[test]
fn archive_is_idempotent_and_undo_unarchives() {
    let fixture = Fixture::github("\"archive\"");

    let archived = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&archived, 0, "org/demo: archived\n");
    assert_eq!(text(&archived.stderr), "");
    assert_eq!(state(&fixture.state, "archived"), "true");

    let again = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&again, 0, "org/demo: already archived\n");

    let undone = fixture.run(&["archive", "org/demo", "--undo"]);
    assert_outcome(&undone, 0, "org/demo: unarchived\n");
    assert_eq!(state(&fixture.state, "archived"), "false");

    let not_archived = fixture.run(&["archive", "org/demo", "--undo"]);
    assert_outcome(&not_archived, 0, "org/demo: not archived\n");

    let check = "github.com auth status --hostname github.com";
    let view = "github.com repo view org/demo --json nameWithOwner,isArchived,visibility";
    assert_eq!(
        fixture.calls(),
        [
            check,
            view,
            "github.com repo archive org/demo --yes",
            check,
            view,
            check,
            view,
            "github.com repo unarchive org/demo --yes",
            check,
            view,
        ]
    );
}

#[test]
fn a_repository_missing_on_github_fails_without_a_change() {
    let fixture = Fixture::github("\"archive\"");
    fixture.set("missing", "");
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("org/demo: not found on github.com"),
        "{}",
        text(&output.stderr)
    );
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|call| call.contains("repo archive"))
    );
}

#[test]
fn private_and_public_need_yes_outside_a_tty() {
    let fixture = Fixture::github("\"visibility\"");
    fixture.set("visibility", "PUBLIC");

    let refused = fixture.run(&["visibility", "org/demo", "private"]);
    assert_outcome(&refused, 2, "");
    assert!(
        text(&refused.stderr).contains("visibility private requires --yes"),
        "{}",
        text(&refused.stderr)
    );
    assert!(fixture.calls().is_empty(), "gh must not run");

    let private = fixture.run(&["visibility", "org/demo", "private", "--yes"]);
    assert_outcome(&private, 0, "org/demo: made private\n");
    assert_eq!(text(&private.stderr), "");
    assert_eq!(state(&fixture.state, "visibility"), "PRIVATE");

    let calls = fixture.calls().len();
    let refused = fixture.run(&["visibility", "org/demo", "public"]);
    assert_outcome(&refused, 2, "");
    assert!(
        text(&refused.stderr).contains("--yes"),
        "{}",
        text(&refused.stderr)
    );
    assert_eq!(fixture.calls().len(), calls, "gh must not run");

    let public = fixture.run(&["visibility", "org/demo", "public", "--yes"]);
    assert_outcome(&public, 0, "org/demo: made public\n");
    assert_eq!(state(&fixture.state, "visibility"), "PUBLIC");
    assert!(fixture.calls().contains(
        &"github.com repo edit org/demo --visibility public --accept-visibility-change-consequences"
            .to_owned()
    ));

    let again = fixture.run(&["visibility", "org/demo", "public", "--yes"]);
    assert_outcome(&again, 0, "org/demo: already public\n");
    let edits = fixture.calls();
    let edits = edits
        .iter()
        .filter(|call| call.contains("--visibility public"));
    assert_eq!(edits.count(), 1);
}

#[test]
fn an_internal_repository_can_be_made_private() {
    let fixture = Fixture::github("\"visibility\"");
    fixture.set("visibility", "INTERNAL");
    let output = fixture.run(&["visibility", "org/demo", "private", "--yes"]);
    assert_outcome(&output, 0, "org/demo: made private\n");
}

#[test]
fn an_enterprise_host_reaches_gh_through_gh_host() {
    let fixture = Fixture::new(
        "[providers.\"ghe.example.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n",
    );
    let output = fixture.run(&["archive", "ghe.example.com/org/demo"]);
    assert_outcome(&output, 0, "ghe.example.com/org/demo: archived\n");
    assert_eq!(
        fixture.calls(),
        [
            "ghe.example.com auth status --hostname ghe.example.com",
            "ghe.example.com repo view org/demo --json nameWithOwner,isArchived,visibility",
            "ghe.example.com repo archive org/demo --yes",
        ]
    );
}

#[test]
fn a_failing_auth_check_shows_gh_s_own_cause() {
    let fixture = Fixture::github("\"archive\"");
    fixture.set("offline", "");
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("error connecting to api.github.com"),
        "{stderr}"
    );
    assert!(
        stderr.contains("gh auth login --hostname github.com"),
        "{stderr}"
    );
}

#[test]
fn a_failing_change_passes_gh_s_error_through() {
    let fixture = Fixture::github("\"archive\"");
    fixture.set("archive-fails", "");
    let output = fixture.run(&["archive", "org/demo"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("org/demo: HTTP 403: Must have admin rights to Repository."),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(state(&fixture.state, "archived"), "false");
}

/// A fixture allowing `create` whose github.com clones come from bare repositories it holds.
fn create_fixture() -> (Fixture, PathBuf) {
    let fixture = Fixture::github("\"create\"");
    fixture.set("missing", "");
    let temp = fixture.home.parent().unwrap();
    let remotes = temp.join("remotes");
    std::fs::create_dir_all(remotes.join("org")).unwrap();
    let gitconfig = temp.join("gitconfig");
    std::fs::write(
        &gitconfig,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = git@github.com:\n",
            remotes.display()
        ),
    )
    .unwrap();
    (fixture, gitconfig)
}

/// The empty repository `gh repo create` leaves on GitHub.
fn remote_repository(fixture: &Fixture, name: &str) {
    let remote = fixture
        .home
        .parent()
        .unwrap()
        .join(format!("remotes/org/{name}.git"));
    let output = support::external(fixture.home.parent().unwrap(), "git")
        .args(["init", "--bare", "--quiet", remote.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
}

fn run_create(fixture: &Fixture, gitconfig: &Path, args: &[&str]) -> Output {
    support::lager_with_path(&fixture.home, &fixture.config, Some(&fixture.bin))
        .env("GH_ARGS_LOG", &fixture.log)
        .env("GH_STATE", &fixture.state)
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .args(args)
        .output()
        .unwrap()
}

fn created(fixture: &Fixture) -> bool {
    fixture
        .calls()
        .iter()
        .any(|call| call.contains("repo create"))
}

#[test]
fn create_makes_the_repository_then_clones_registers_and_runs_post_clone() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let output = run_create(
        &fixture,
        &gitconfig,
        &[
            "add",
            "org/demo",
            "--create",
            "--description=-a demo",
            "--post-clone",
            "printf hooked > hooked.txt",
        ],
    );
    assert_outcome(&output, 0, "org/demo: created\n");
    assert_eq!(
        fixture.calls(),
        [
            "github.com auth status --hostname github.com",
            "github.com repo view org/demo --json nameWithOwner,isArchived,visibility",
            "github.com repo create org/demo --private --description=-a demo",
        ]
    );
    let checkout = fixture.home.join("repos/org/demo");
    assert!(checkout.join(".git").is_dir());
    assert_eq!(state(&checkout, "hooked.txt"), "hooked");
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(config.contains("url = \"org/demo\""), "{config}");
    assert!(
        config.contains("post_clone = \"printf hooked > hooked.txt\""),
        "{config}"
    );
}

#[test]
fn create_public_without_registering() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--public", "--no-register"],
    );
    assert_outcome(&output, 0, "org/demo: created\n");
    assert_eq!(
        fixture.calls().last().unwrap(),
        "github.com repo create org/demo --public"
    );
    assert!(fixture.home.join("repos/org/demo/.git").is_dir());
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(!config.contains("[[repositories]]"), "{config}");
}

#[test]
fn create_refuses_an_existing_destination_before_touching_github() {
    let (fixture, gitconfig) = create_fixture();
    std::fs::create_dir_all(fixture.home.join("repos/org/demo")).unwrap();
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--no-register"],
    );
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("destination already exists"),
        "{}",
        text(&output.stderr)
    );
    assert!(!created(&fixture));
}

#[test]
fn create_refuses_a_taken_name() {
    let (fixture, gitconfig) = create_fixture();
    std::fs::remove_file(fixture.state.join("missing")).unwrap();
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--no-register"],
    );
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("org/demo: already exists on github.com"),
        "{}",
        text(&output.stderr)
    );
    assert!(!created(&fixture));
    assert!(!fixture.home.join("repos/org/demo").exists());
}

#[test]
fn create_needs_create_in_remote_actions() {
    let (fixture, gitconfig) = create_fixture();
    std::fs::write(
        &fixture.config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n",
    )
    .unwrap();
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--no-register"],
    );
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("`create` is not enabled for github.com"),
        "{}",
        text(&output.stderr)
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_clone_failing_after_create_says_what_happened_and_how_to_finish() {
    let (fixture, gitconfig) = create_fixture();
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--register"],
    );
    assert_outcome(&output, 1, "org/demo: created\n");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("created github.com/org/demo; clone failed:")
            && stderr.contains("rerun `lager add github.com/org/demo` without --create"),
        "{stderr}"
    );
    assert!(created(&fixture));
    assert!(!fixture.home.join("repos/org/demo").exists());
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(!config.contains("[[repositories]]"), "{config}");
}

#[test]
fn create_takes_exactly_one_explicit_repository() {
    let (fixture, gitconfig) = create_fixture();
    for args in [
        &["add", "org/demo", "--public", "--no-register"][..],
        &["add", "org/demo", "--description", "x", "--no-register"],
        &["add", "--create", "--no-register"],
        &["add", "org/one", "org/two", "--create", "--no-register"],
        &["add", "org/*", "--create", "--no-register"],
        &["add", "org/demo", "--from", "scratch", "--no-register"],
    ] {
        let output = run_create(&fixture, &gitconfig, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&output.stderr)
        );
    }
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_post_clone_failing_after_create_reports_the_creation_without_a_rerun_hint() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let output = run_create(
        &fixture,
        &gitconfig,
        &["add", "org/demo", "--create", "--post-clone", "exit 3"],
    );
    assert_outcome(&output, 1, "org/demo: created\n");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("created github.com/org/demo; ") && !stderr.contains("rerun"),
        "{stderr}"
    );
    assert!(fixture.home.join("repos/org/demo/.git").is_dir());
}

fn git(fixture: &Fixture, directory: &Path, args: &[&str]) -> String {
    let output = support::external(fixture.home.parent().unwrap(), "git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout).trim().to_owned()
}

/// A folder at `path` holding a README, optionally a repository and optionally committed.
fn source_folder(fixture: &Fixture, path: &Path, repository: bool, commit: bool) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join("README.md"), "demo\n").unwrap();
    if repository {
        git(fixture, path, &["init", "--quiet"]);
    }
    if commit {
        git(fixture, path, &["add", "README.md"]);
        git(
            fixture,
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--quiet",
                "-m",
                "start",
            ],
        );
    }
}

#[test]
fn publish_pushes_the_folder_moves_it_into_root_and_registers_it() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let source = fixture.home.parent().unwrap().join("scratch/demo");
    source_folder(&fixture, &source, true, true);
    let canonical = std::fs::canonicalize(&source).unwrap();
    let output = run_create(
        &fixture,
        &gitconfig,
        &[
            "add",
            "org/demo",
            "--create",
            "--from",
            source.to_str().unwrap(),
            "--description=-a demo",
            "--post-clone",
            "printf hooked > hooked.txt",
        ],
    );
    assert_outcome(&output, 0, "org/demo: published\n");
    assert_eq!(
        fixture.calls(),
        [
            "github.com auth status --hostname github.com".to_owned(),
            "github.com repo view org/demo --json nameWithOwner,isArchived,visibility".to_owned(),
            format!(
                "github.com repo create org/demo --private --description=-a demo --source {} --remote origin --push",
                canonical.display()
            ),
        ]
    );
    assert!(!source.exists());
    let checkout = fixture.home.join("repos/org/demo");
    assert_eq!(state(&checkout, "README.md"), "demo\n");
    assert_eq!(state(&checkout, "hooked.txt"), "hooked");
    assert_eq!(
        git(&fixture, &checkout, &["config", "remote.origin.url"]),
        "git@github.com:org/demo.git"
    );
    let remote = fixture.home.parent().unwrap().join("remotes/org/demo.git");
    assert!(!git(&fixture, &remote, &["for-each-ref"]).is_empty());
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(config.contains("url = \"org/demo\""), "{config}");
}

#[test]
fn publish_refuses_an_unready_folder_before_touching_github() {
    let cases: [(&str, &str); 9] = [
        ("not a repository", "not a Git working tree"),
        ("no commit", "it has no commit yet"),
        ("has origin", "it already has a remote (origin)"),
        ("destination exists", "destination already exists"),
        ("inside root", "it is already inside"),
        ("subdirectory", "not the top of its repository"),
        (
            "linked worktree",
            "remove them first (lager worktree remove)",
        ),
        ("git dir elsewhere", "its Git directory lives outside it"),
        ("destination inside source", "lies inside it"),
    ];
    for (case, message) in cases {
        let (fixture, gitconfig) = create_fixture();
        let temp = fixture.home.parent().unwrap().to_owned();
        let source = match case {
            "inside root" => fixture.home.join("repos/scratch/demo"),
            "destination inside source" => fixture.home.clone(),
            _ => temp.join("scratch/demo"),
        };
        source_folder(
            &fixture,
            &source,
            case != "not a repository",
            !matches!(case, "not a repository" | "no commit"),
        );
        if case == "has origin" {
            git(
                &fixture,
                &source,
                &["remote", "add", "origin", "https://example.com/x.git"],
            );
        }
        if case == "linked worktree" {
            let linked = temp.join("scratch/linked");
            git(
                &fixture,
                &source,
                &["worktree", "add", "--quiet", linked.to_str().unwrap()],
            );
        }
        if case == "git dir elsewhere" {
            let elsewhere = temp.join("gitdir");
            std::fs::rename(source.join(".git"), &elsewhere).unwrap();
            std::fs::write(
                source.join(".git"),
                format!("gitdir: {}\n", elsewhere.display()),
            )
            .unwrap();
        }
        if case == "destination exists" {
            std::fs::create_dir_all(fixture.home.join("repos/org/demo")).unwrap();
        }
        let from = match case {
            "subdirectory" => source.join("nested"),
            _ => source.clone(),
        };
        std::fs::create_dir_all(&from).unwrap();
        let output = run_create(
            &fixture,
            &gitconfig,
            &[
                "add",
                "org/demo",
                "--create",
                "--from",
                from.to_str().unwrap(),
                "--no-register",
            ],
        );
        assert_outcome(&output, 1, "");
        assert!(
            text(&output.stderr).contains(message),
            "{case}: {}",
            text(&output.stderr)
        );
        assert!(!created(&fixture), "{case}");
        if case != "destination exists" {
            assert!(!fixture.home.join("repos/org").exists(), "{case}");
        }
        assert_eq!(state(&source, "README.md"), "demo\n", "{case}");
        if case != "not a repository" {
            let remotes = git(&fixture, &source, &["remote"]);
            assert_eq!(
                remotes,
                if case == "has origin" { "origin" } else { "" },
                "{case}"
            );
        }
    }
}

#[test]
fn a_move_failing_after_publish_says_what_was_published_and_leaves_the_folder() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let source = fixture.home.parent().unwrap().join("scratch/demo");
    source_folder(&fixture, &source, true, true);
    let owner = fixture.home.join("repos/org");
    std::fs::create_dir_all(&owner).unwrap();
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o555)).unwrap();
    let output = run_create(
        &fixture,
        &gitconfig,
        &[
            "add",
            "org/demo",
            "--create",
            "--from",
            source.to_str().unwrap(),
            "--register",
        ],
    );
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "org/demo: published github.com/org/demo from {}; move failed:",
            source.display()
        )) && stderr.contains("then `lager register github.com/org/demo`"),
        "{stderr}"
    );
    assert!(created(&fixture));
    assert_eq!(git(&fixture, &source, &["remote"]), "origin");
    assert!(!owner.join("demo").exists());
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(!config.contains("[[repositories]]"), "{config}");
}

#[test]
fn a_push_failing_after_gh_created_the_repository_says_it_was_created() {
    let (fixture, gitconfig) = create_fixture();
    fixture.set("push-fails", "");
    let source = fixture.home.parent().unwrap().join("scratch/demo");
    source_folder(&fixture, &source, true, true);
    let output = run_create(
        &fixture,
        &gitconfig,
        &[
            "add",
            "org/demo",
            "--create",
            "--from",
            source.to_str().unwrap(),
            "--no-register",
        ],
    );
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("org/demo: created github.com/org/demo; push failed: ")
            && stderr.contains("rejected push")
            && stderr.contains("origin is not set and ")
            && stderr.contains("then `lager register github.com/org/demo`"),
        "{stderr}"
    );
    assert_eq!(state(&source, "README.md"), "demo\n");
    assert!(!fixture.home.join("repos/org/demo").exists());
}

#[test]
fn a_post_clone_failing_after_the_move_says_what_was_done() {
    let (fixture, gitconfig) = create_fixture();
    remote_repository(&fixture, "demo");
    let source = fixture.home.parent().unwrap().join("scratch/demo");
    source_folder(&fixture, &source, true, true);
    let output = run_create(
        &fixture,
        &gitconfig,
        &[
            "add",
            "org/demo",
            "--create",
            "--from",
            source.to_str().unwrap(),
            "--post-clone",
            "exit 3",
        ],
    );
    assert_outcome(&output, 1, "org/demo: published\n");
    let checkout = fixture.home.join("repos/org/demo");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "published org/demo from {}; moved to {}; post_clone failed",
            source.display(),
            checkout.display()
        )),
        "{stderr}"
    );
    assert!(checkout.join(".git").is_dir());
    let config = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(config.contains("post_clone = \"exit 3\""), "{config}");
}

/// A fixture allowing `delete` whose login holds the delete_repo scope.
fn delete_fixture() -> Fixture {
    let fixture = Fixture::github("\"delete\"");
    fixture.set("scopes", "repo, delete_repo, gist");
    fixture
}

/// Declares org/demo and checks it out cleanly, its branch level with its remote-tracking ref.
fn declared_checkout(fixture: &Fixture) -> PathBuf {
    let checkout = clean_checkout(fixture);
    declare(fixture);
    checkout
}

/// Checks out org/demo cleanly without declaring it.
fn clean_checkout(fixture: &Fixture) -> PathBuf {
    let checkout = fixture.home.join("repos/org/demo");
    source_folder(fixture, &checkout, true, true);
    git(fixture, &checkout, &["branch", "-M", "main"]);
    git(
        fixture,
        &checkout,
        &["remote", "add", "origin", "git@github.com:org/demo.git"],
    );
    git(
        fixture,
        &checkout,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(
        fixture,
        &checkout,
        &["branch", "--set-upstream-to=origin/main"],
    );
    checkout
}

fn declare(fixture: &Fixture) {
    let mut config = std::fs::read_to_string(&fixture.config).unwrap();
    config.push_str("\n[[repositories]]\nurl = \"org/demo\"\n");
    std::fs::write(&fixture.config, config).unwrap();
}

fn declared(fixture: &Fixture) -> bool {
    std::fs::read_to_string(&fixture.config)
        .unwrap()
        .contains("org/demo")
}

fn deleted(fixture: &Fixture) -> bool {
    fixture
        .calls()
        .iter()
        .any(|call| call.contains("repo delete"))
}

#[test]
fn delete_with_yes_deletes_on_github_then_the_checkout_and_the_declaration() {
    let fixture = delete_fixture();
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(
        &output,
        0,
        "org/demo: deleted remote, removed, unregistered\n",
    );
    assert_eq!(
        fixture.calls(),
        [
            "github.com auth status --hostname github.com",
            "github.com api --hostname github.com -i user",
            "github.com repo view org/demo --json nameWithOwner,isArchived,visibility",
            "github.com repo delete org/demo --yes",
        ]
    );
    assert!(!checkout.exists());
    assert!(!declared(&fixture));
}

#[test]
fn delete_refuses_local_work_before_touching_github_unless_forced() {
    let fixture = delete_fixture();
    let checkout = declared_checkout(&fixture);
    std::fs::write(checkout.join("notes.txt"), "unsaved\n").unwrap();
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains(
            "org/demo: local work would be lost (untracked files); GitHub was not touched; rerun with --force to accept these losses"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(!deleted(&fixture));
    assert_eq!(state(&checkout, "notes.txt"), "unsaved\n");
    assert!(declared(&fixture));

    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes", "--force"]);
    assert_outcome(
        &output,
        0,
        "org/demo: deleted remote, removed, unregistered\n",
    );
    assert!(
        text(&output.stderr).contains("this is lost: untracked files"),
        "{}",
        text(&output.stderr)
    );
    assert!(!checkout.exists());
}

#[test]
fn delete_without_the_delete_repo_scope_names_the_refresh_command() {
    let fixture = delete_fixture();
    fixture.set("scopes", "repo, read:org");
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains(
            "deleting on github.com needs the delete_repo scope; run `gh auth refresh --hostname github.com --scopes delete_repo`"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(!deleted(&fixture));
    assert!(checkout.exists());
}

#[test]
fn delete_with_a_fine_grained_token_leaves_the_scope_to_gh() {
    let fixture = delete_fixture();
    std::fs::remove_file(fixture.state.join("scopes")).unwrap();
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 0, "org/demo: deleted remote\n");
    assert!(deleted(&fixture));
}

#[test]
fn delete_of_a_repository_without_checkout_unregisters_only() {
    let fixture = delete_fixture();
    declare(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 0, "org/demo: deleted remote, unregistered\n");
    assert!(deleted(&fixture));
    assert!(!declared(&fixture));
}

#[test]
fn delete_outside_a_tty_needs_yes() {
    let fixture = delete_fixture();
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote"]);
    assert_outcome(&output, 2, "");
    assert!(
        text(&output.stderr).contains("remove --remote requires --yes outside a TTY"),
        "{}",
        text(&output.stderr)
    );
    assert!(fixture.calls().is_empty());
    assert!(checkout.exists());
}

#[test]
fn delete_needs_delete_in_remote_actions() {
    let fixture = Fixture::github("\"archive\"");
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(text(&output.stderr).contains("`delete` is not enabled for github.com"));
    assert!(fixture.calls().is_empty());
    assert!(checkout.exists());
    assert!(declared(&fixture));
}

#[test]
fn delete_conflicts_with_keep_registered_and_takes_one_repository() {
    let fixture = delete_fixture();
    for args in [
        &[
            "remove",
            "org/demo",
            "--remote",
            "--yes",
            "--keep-registered",
        ][..],
        &["remove", "org/demo", "org/other", "--remote", "--yes"][..],
        &["remove", "--remote", "--yes"][..],
    ] {
        let output = fixture.run(args);
        assert_outcome(&output, 2, "");
    }
    assert!(fixture.calls().is_empty());
}

#[test]
fn delete_of_a_repository_missing_on_github_names_the_local_cleanup() {
    let fixture = delete_fixture();
    fixture.set("missing", "");
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains(
            "org/demo: not found on github.com; run `lager remove github.com/org/demo --unregister` for the local side"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(!deleted(&fixture));
    assert!(checkout.exists());
}

#[test]
fn a_checkout_changing_after_the_github_delete_says_what_remains_and_how_to_finish() {
    let fixture = delete_fixture();
    let checkout = declared_checkout(&fixture);
    fixture.set(
        "dirty-on-delete",
        checkout.join("late.txt").to_str().unwrap(),
    );
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("org/demo: deleted github.com/org/demo; local removal failed:"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "and its declaration remain; finish with `lager remove github.com/org/demo --unregister`"
        ),
        "{stderr}"
    );
    assert!(deleted(&fixture));
    assert!(checkout.join("late.txt").exists());
    assert!(declared(&fixture));
}

/// A fixture allowing `rename` where org/demo exists on GitHub and org/demo-cli is free.
fn rename_fixture() -> Fixture {
    let fixture = Fixture::github("\"rename\"");
    fixture.set("free", "org/demo-cli");
    fixture
}

fn config_text(fixture: &Fixture) -> String {
    std::fs::read_to_string(&fixture.config).unwrap()
}

fn append_config(fixture: &Fixture, text: &str) {
    let config = config_text(fixture) + text;
    std::fs::write(&fixture.config, config).unwrap();
}

fn renamed(fixture: &Fixture) -> bool {
    fixture
        .calls()
        .iter()
        .any(|call| call.contains("repo rename"))
}

#[test]
fn rename_renames_on_github_then_repoints_origin_moves_the_checkout_and_redeclares() {
    let fixture = rename_fixture();
    let checkout = clean_checkout(&fixture);
    append_config(
        &fixture,
        "\n# the demo\n[[repositories]]\nurl = \"org/demo\" # short\npost_clone = \"make\"\n",
    );
    let before = config_text(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(
        &output,
        0,
        "org/demo: renamed to org/demo-cli, moved, redeclared\n",
    );
    assert_eq!(
        fixture.calls(),
        [
            "github.com auth status --hostname github.com",
            "github.com repo view org/demo --json nameWithOwner,isArchived,visibility",
            "github.com repo view org/demo-cli --json nameWithOwner,isArchived,visibility",
            "github.com repo rename demo-cli --repo org/demo --yes",
        ]
    );
    assert!(!checkout.exists());
    let moved = fixture.home.join("repos/org/demo-cli");
    assert_eq!(state(&moved, "README.md"), "demo\n");
    assert_eq!(
        git(&fixture, &moved, &["config", "remote.origin.url"]),
        "git@github.com:org/demo-cli.git"
    );
    assert_eq!(
        config_text(&fixture),
        before.replace("\"org/demo\"", "\"org/demo-cli\"")
    );
}

#[test]
fn rename_of_a_wildcard_member_leaves_the_config_untouched() {
    let fixture = rename_fixture();
    clean_checkout(&fixture);
    append_config(&fixture, "\n[[repositories]]\nurl = \"github.com/org/*\"\n");
    let before = config_text(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 0, "org/demo: renamed to org/demo-cli, moved\n");
    assert!(fixture.home.join("repos/org/demo-cli/.git").is_dir());
    assert_eq!(config_text(&fixture), before);
}

#[test]
fn rename_renames_a_wildcard_exclude_naming_the_repository() {
    let fixture = rename_fixture();
    append_config(
        &fixture,
        "\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"demo\", \"other\"]\n",
    );
    let before = config_text(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(
        &output,
        0,
        "org/demo: renamed to org/demo-cli, redeclared\n",
    );
    assert_eq!(
        config_text(&fixture),
        before.replace("[\"demo\",", "[\"demo-cli\",")
    );
}

#[test]
fn rename_refuses_linked_worktrees_before_touching_github() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    let linked = fixture.home.join("worktrees/demo-feature");
    git(
        &fixture,
        &checkout,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("it has linked worktrees (feature at ")
            && stderr
                .contains("demo-feature); remove them first (lager worktree remove), then rename"),
        "{stderr}"
    );
    assert!(!renamed(&fixture));
    assert!(checkout.join(".git").is_dir());
    assert!(declared(&fixture));
}

#[test]
fn rename_refuses_a_taken_name() {
    let fixture = rename_fixture();
    std::fs::remove_file(fixture.state.join("free")).unwrap();
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("github.com/org/demo-cli already exists on github.com"),
        "{}",
        text(&output.stderr)
    );
    assert!(!renamed(&fixture));
    assert!(checkout.exists());
}

#[test]
fn rename_without_checkout_renames_on_github_and_redeclares() {
    let fixture = rename_fixture();
    declare(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(
        &output,
        0,
        "org/demo: renamed to org/demo-cli, redeclared\n",
    );
    assert!(renamed(&fixture));
    assert!(config_text(&fixture).contains("url = \"org/demo-cli\""));
    assert!(!fixture.home.join("repos/org/demo-cli").exists());
}

#[test]
fn rename_outside_a_tty_needs_yes() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli"]);
    assert_outcome(&output, 2, "");
    assert!(text(&output.stderr).contains("rename requires --yes outside a TTY"));
    assert!(fixture.calls().is_empty());
    assert!(checkout.exists());
}

#[test]
fn rename_needs_rename_in_remote_actions() {
    let fixture = Fixture::github("\"archive\"");
    let checkout = declared_checkout(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(text(&output.stderr).contains("`rename` is not enabled for github.com"));
    assert!(fixture.calls().is_empty());
    assert!(checkout.exists());
}

#[test]
fn rename_refuses_an_invalid_or_unchanged_name_as_usage() {
    let fixture = rename_fixture();
    declare(&fixture);
    for (args, error) in [
        (
            &["rename", "org/demo", "org/demo-cli", "--yes"][..],
            "invalid repository name `org/demo-cli`",
        ),
        (
            &["rename", "org/demo", "..", "--yes"],
            "invalid repository name `..`",
        ),
        (
            &["rename", "org/demo", "bad name", "--yes"],
            "invalid repository name `bad name`",
        ),
        (
            &["rename", "org/demo", "demo.git", "--yes"],
            "invalid repository name `demo.git`",
        ),
        (
            &["rename", "--yes", "org/demo", "--", "-demo"],
            "invalid repository name `-demo`",
        ),
        (
            &["rename", "org/demo", "Demo", "--yes"],
            "org/demo: already named Demo",
        ),
    ] {
        let output = fixture.run(args);
        assert_outcome(&output, 2, "");
        assert!(
            text(&output.stderr).contains(error),
            "{}",
            text(&output.stderr)
        );
    }
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_move_failing_after_the_github_rename_says_what_was_done_and_how_to_finish() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    let blocked = fixture.home.join("repos/org/demo-cli");
    fixture.set("on-rename", &format!(": > '{}'", blocked.display()));
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "renamed github.com/org/demo to github.com/org/demo-cli; origin set to git@github.com:org/demo-cli.git; moving the checkout failed: "
        ) && stderr.contains("GitHub redirects the old URL meanwhile; finish with: move ")
            && stderr.contains(", then replace github.com/org/demo with github.com/org/demo-cli in "),
        "{stderr}"
    );
    assert!(checkout.join(".git").is_dir());
    assert!(config_text(&fixture).contains("url = \"org/demo\""));
}

#[test]
fn rename_back_to_a_former_name_that_only_redirects_is_allowed() {
    let fixture = rename_fixture();
    std::fs::remove_file(fixture.state.join("free")).unwrap();
    fixture.set("redirects", "org/demo-cli org/demo\n");
    declare(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(
        &output,
        0,
        "org/demo: renamed to org/demo-cli, redeclared\n",
    );
    assert!(renamed(&fixture));
}

#[test]
fn delete_of_a_former_name_that_redirects_elsewhere_is_not_found() {
    let fixture = delete_fixture();
    fixture.set("redirects", "org/demo org/other\n");
    declare(&fixture);
    let output = fixture.run(&["remove", "org/demo", "--remote", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("org/demo: not found on github.com"),
        "{}",
        text(&output.stderr)
    );
    assert!(!deleted(&fixture));
    assert!(declared(&fixture));
}

#[test]
fn rename_refuses_a_differently_cased_name_before_touching_github() {
    let fixture = rename_fixture();
    fixture.set("redirects", "org/demo org/Demo\n");
    declare(&fixture);
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains("use the exact name org/Demo"),
        "{}",
        text(&output.stderr)
    );
    assert!(!renamed(&fixture));
}

#[test]
fn rename_refuses_a_checkout_whose_origin_is_another_repository() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    git(
        &fixture,
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:org/other.git",
        ],
    );
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr)
            .contains("its origin is not github.com/org/demo; GitHub was not touched"),
        "{}",
        text(&output.stderr)
    );
    assert!(!renamed(&fixture));
}

#[test]
fn rename_refuses_an_existing_destination_even_without_checkout() {
    let fixture = rename_fixture();
    declare(&fixture);
    let taken = fixture.home.join("repos/org/demo-cli");
    std::fs::create_dir_all(&taken).unwrap();
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    assert!(
        text(&output.stderr).contains(&format!(
            "destination already exists: {}; GitHub was not touched",
            taken.display()
        )),
        "{}",
        text(&output.stderr)
    );
    assert!(!renamed(&fixture));
}

#[test]
fn setting_origin_failing_after_the_github_rename_names_every_step_left() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    fixture.set(
        "on-rename",
        &format!(": > '{}'", checkout.join(".git/config.lock").display()),
    );
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("renamed github.com/org/demo to github.com/org/demo-cli; setting origin failed: ")
            && stderr.contains(&format!(
                "finish with: `git -C {} remote set-url origin git@github.com:org/demo-cli.git`, then move {} to ",
                checkout.display(),
                checkout.display()
            ))
            && stderr.contains(", then replace github.com/org/demo with github.com/org/demo-cli in "),
        "{stderr}"
    );
    assert!(checkout.join(".git").is_dir());
    assert!(config_text(&fixture).contains("url = \"org/demo\""));
}

#[test]
fn a_declaration_failing_after_the_move_names_what_was_done_and_what_is_left() {
    let fixture = rename_fixture();
    let checkout = declared_checkout(&fixture);
    fixture.set(
        "on-rename",
        &format!("printf '[[' >> '{}'", fixture.config.display()),
    );
    let output = fixture.run(&["rename", "org/demo", "demo-cli", "--yes"]);
    assert_outcome(&output, 1, "");
    let moved = fixture.home.join("repos/org/demo-cli");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "renamed github.com/org/demo to github.com/org/demo-cli; origin set to git@github.com:org/demo-cli.git; moved to {}; rewriting the declaration failed: ",
            moved.display()
        )) && stderr.contains(&format!(
            "finish with: replace github.com/org/demo with github.com/org/demo-cli in {}",
            fixture.config.display()
        )),
        "{stderr}"
    );
    assert!(!checkout.exists());
    assert_eq!(
        git(&fixture, &moved, &["config", "remote.origin.url"]),
        "git@github.com:org/demo-cli.git"
    );
}
