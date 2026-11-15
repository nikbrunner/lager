#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

/// Logs `$GH_HOST $*` per call and keeps the repository's state in `$GH_STATE`.
const FAKE_GH: &str = r##"#!/bin/sh
printf '%s %s\n' "$GH_HOST" "$*" >> "$GH_ARGS_LOG"
case "$1 $2" in
  "auth status")
    if [ -e "$GH_STATE/offline" ]; then
      printf '%s\n' "error connecting to api.github.com" >&2
      exit 1
    fi
    if [ -e "$GH_STATE/logged-out" ]; then
      printf '%s\n' "You are not logged into any GitHub hosts. To log in, run: gh auth login" >&2
      exit 1
    fi
    ;;
  "repo view")
    if [ -e "$GH_STATE/missing" ]; then
      printf '%s\n' "GraphQL: Could not resolve to a Repository with the name '$3'. (repository)" >&2
      exit 1
    fi
    printf '{"isArchived":%s,"visibility":"%s"}\n' "$(cat "$GH_STATE/archived")" "$(cat "$GH_STATE/visibility")"
    ;;
  "repo archive")
    if [ -e "$GH_STATE/archive-fails" ]; then
      printf '%s\n' "HTTP 403: Must have admin rights to Repository." >&2
      exit 1
    fi
    printf 'true' > "$GH_STATE/archived"
    ;;
  "repo unarchive") printf 'false' > "$GH_STATE/archived" ;;
  "repo edit") printf '%s' "$5" | tr 'a-z' 'A-Z' > "$GH_STATE/visibility" ;;
  "repo create") ;;
  *) printf 'unexpected gh call: %s\n' "$*" >&2; exit 1 ;;
esac
"##;

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
        let bin = temp.path().join("bin");
        let state = temp.path().join("state");
        for directory in [&home, &bin, &state] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let gh = bin.join("gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(state.join("archived"), "false").unwrap();
        std::fs::write(state.join("visibility"), "PRIVATE").unwrap();
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
    let output = fixture.run(&["visibility", "org/demo", "private"]);
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
    let view = "github.com repo view org/demo --json isArchived,visibility";
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
fn private_needs_no_prompt_and_public_needs_yes_outside_a_tty() {
    let fixture = Fixture::github("\"visibility\"");
    fixture.set("visibility", "PUBLIC");

    let private = fixture.run(&["visibility", "org/demo", "private"]);
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
    let output = fixture.run(&["visibility", "org/demo", "private"]);
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
            "ghe.example.com repo view org/demo --json isArchived,visibility",
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
            "github.com repo view org/demo --json isArchived,visibility",
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
