#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const SESSION: &str = r#"
[session]
host = "rex"

[session.layouts.default]
focus = "Shells"

[session.layouts.default.tabs.Agents]
order = 1
root = { pane = "Agent I" }
splits = [{ source = "Agent I", pane = "Agent II", direction = "right" }]

[session.layouts.default.tabs.Shells]
order = 2
root = { pane = "Shell I" }
splits = [{ source = "Shell I", pane = "Shell II", direction = "down", ratio = 0.25 }]
"#;

/// A temporary HOME with one repository at `repos/acme/core` and the fake `rex` first on
/// `PATH`. No call can reach a real Rex: the environment is cleared, and `REX_SERVER` names a
/// socket that does not exist.
struct Fixture {
    _temp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
    primary: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

fn git(directory: &Path, args: &[&str]) {
    let output = support::external(directory, "git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=lager",
            "-c",
            "user.email=lager@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

impl Fixture {
    fn new() -> Self {
        Self::with_config(&format!("root = \"repos\"\n{SESSION}"))
    }

    fn with_config(config: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let home = base.join("home");
        let primary = home.join("repos/acme/core");
        fs::create_dir_all(&primary).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        fs::write(primary.join("README"), "initial\n").unwrap();
        git(&primary, &["add", "README"]);
        git(&primary, &["commit", "-qm", "initial"]);
        let config_path = base.join("config.toml");
        fs::write(&config_path, config).unwrap();
        let (bin, state) = support::install_fake_rex(&base.join("rex"));
        Self {
            _temp: temp,
            base,
            home,
            config: config_path,
            primary,
            bin,
            state,
        }
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = support::lager_with_path(&self.home, &self.config, Some(&self.bin));
        command
            .current_dir(cwd)
            .env("REX_ARGS_LOG", self.state.join("args.log"))
            .env("REX_STATE", &self.state)
            .env(
                "REX_SERVER",
                format!("unix://{}", self.base.join("none.sock").display()),
            )
            .env("REX_RUNTIME_DIR", self.base.join("rex-runtime"));
        command
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().unwrap()
    }

    fn session(&self, args: &[&str]) -> Output {
        self.run_in(&self.base, &[&["session"], args].concat())
    }

    /// Every rex call so far, global flags removed.
    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.state.join("args.log")).unwrap_or_default();
        log.lines()
            .map(|line| {
                line.strip_prefix("--autostart=false --timeout 8s ")
                    .unwrap_or_else(|| panic!("rex call without the global flags: {line}"))
                    .to_owned()
            })
            .collect()
    }

    fn creates(&self) -> Vec<Value> {
        self.calls()
            .iter()
            .filter_map(|call| call.strip_prefix("do - --json --args "))
            .map(|args| serde_json::from_str(args).unwrap())
            .collect()
    }

    fn mode(&self, name: &str) {
        fs::write(self.state.join(name), "").unwrap();
    }

    fn linked(&self, name: &str) -> PathBuf {
        let path = self.home.join("repos/acme/core.worktrees").join(name);
        git(
            &self.primary,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }
}

fn block(pane: &str, cwd: &Path) -> Value {
    json!({ "block": {
        "flavor": "com.superlogical.terminal.shell",
        "options": { "cwd": cwd.to_str().unwrap() },
        "label": pane,
    }})
}

fn split(direction: &str, ratio: f64, before: Value, after: Value) -> Value {
    json!({ "split": { "direction": direction, "ratio": ratio, "before": before, "after": after } })
}

fn expected_create(label: &str, cwd: &Path) -> Value {
    json!({
        "label": label,
        "initial_windows": [
            {
                "window_label": "Agents",
                "layout": split("horizontal", 0.5, block("Agent I", cwd), block("Agent II", cwd)),
            },
            {
                "window_label": "Shells",
                "layout": split("vertical", 0.75, block("Shell I", cwd), block("Shell II", cwd)),
            },
        ],
        "focus": 2,
    })
}

#[test]
fn create_makes_one_session_with_the_configured_layout() {
    let fixture = Fixture::new();

    let output = fixture.session(&["create", "acme/core"]);
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), "Created session core\n");
    assert!(output.status.success());
    assert_eq!(
        fixture.creates(),
        [expected_create("core", &fixture.primary)]
    );
    let script = fs::read_to_string(fixture.state.join("script.lua")).unwrap();
    for method in [
        "session.create",
        "session.attach",
        "session.disown_block",
        "session.focus_window",
    ] {
        assert!(script.contains(method), "{method} missing from {script}");
    }

    let again = fixture.session(&["create", "acme/core"]);
    assert_eq!(stdout(&again), "Session core already exists\n");
    assert!(again.status.success());
    assert_eq!(fixture.creates().len(), 1);
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|call| call.contains("client.perform") || call.contains("session.destroy")),
        "{:?}",
        fixture.calls()
    );
}

#[test]
fn create_json_returns_the_panes_by_name() {
    let fixture = Fixture::new();
    let primary = fixture.primary.to_str().unwrap();
    let expected = |created: bool| {
        json!({
            "schema_version": 1,
            "created": created,
            "path": primary,
            "id": "session:1",
            "label": "core",
            "tabs": [
                { "name": "Agents", "panes": { "Agent I": "block:1-1-1", "Agent II": "block:1-1-2" } },
                { "name": "Shells", "panes": { "Shell I": "block:1-2-1", "Shell II": "block:1-2-2" } },
            ],
        })
    };

    for created in [true, false] {
        let output = fixture.session(&["create", "acme/core", "--json"]);
        assert!(output.status.success(), "{}", stderr(&output));
        let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(document, expected(created));
    }
}

#[test]
fn open_creates_once_and_selects_on_the_newest_connected_app() {
    let fixture = Fixture::new();
    let select = "api call client.perform {\"action\":\"session.select\",\"args\":{\"session_id\":\"session:1\"},\"client_id\":\"client:app\"}";

    for _ in 0..2 {
        let output = fixture.run_in(&fixture.primary, &["session", "open", "."]);
        assert_eq!(stderr(&output), "");
        assert_eq!(stdout(&output), "Opened session core\n");
        assert!(output.status.success());
    }
    assert_eq!(fixture.creates().len(), 1);
    let selects: Vec<_> = fixture
        .calls()
        .into_iter()
        .filter(|call| call.contains("client.perform"))
        .collect();
    assert_eq!(selects, [select, select]);
}

#[test]
fn open_without_an_app_fails_on_a_running_host() {
    let fixture = Fixture::new();
    fixture.mode("no-app");

    let output = fixture.session(&["open", "acme/core"]);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "error: session host failed: no Rex app connected\n"
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn close_ends_the_session_and_forgets_it() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());

    let output = fixture.session(&["close", "acme/core"]);
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), "Closed session core\n");
    assert!(output.status.success());
    let destroy = "api call session.destroy {\"session_id\":\"session:1\"}";
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call.contains("destroy"))
            .collect::<Vec<_>>(),
        [destroy]
    );

    let again = fixture.session(&["close", "acme/core"]);
    assert_eq!(
        stdout(&again),
        format!("No session for {}\n", fixture.primary.display())
    );
    assert!(again.status.success());
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call.contains("destroy"))
            .count(),
        1
    );
}

#[test]
fn a_failed_close_keeps_the_session_recorded() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    fixture.mode("destroy-fails");

    let output = fixture.session(&["close", "acme/core"]);
    assert_eq!(
        stderr(&output),
        "error: session host failed: stream response \"control-1\" failed: internal error (status=500 code=internal)\n"
    );
    assert_eq!(output.status.code(), Some(1));

    let listed = fixture.session(&["list"]);
    assert!(stdout(&listed).contains("session:1"), "{}", stdout(&listed));
}

#[test]
fn a_session_closed_elsewhere_is_created_again() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    fs::remove_file(fixture.state.join("sessions/1")).unwrap();

    let output = fixture.session(&["create", "acme/core"]);
    assert_eq!(stdout(&output), "Created session core\n");
    assert_eq!(fixture.creates().len(), 2);
}

#[test]
fn list_shows_the_checkouts_with_a_live_session() {
    let fixture = Fixture::new();
    let spike = fixture.linked("dev_spike-one");
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    assert!(
        fixture
            .session(&["create", "acme/core", "dev_spike-one"])
            .status
            .success()
    );

    let output = fixture.session(&["list"]);
    assert_eq!(stderr(&output), "");
    let primary = fixture.primary.display().to_string();
    let spike_text = spike.display().to_string();
    let width = spike_text.len();
    assert_eq!(
        stdout(&output),
        format!(
            "{:20}  {primary:width$}  session:1\ncore · Dev spike one  {spike_text}  session:2\n",
            "core"
        )
    );

    let output = fixture.session(&["list", "--json"]);
    let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(document["schema_version"], 1);
    let sessions = document["sessions"].as_array().unwrap();
    assert_eq!(
        sessions
            .iter()
            .map(|session| (
                session["path"].as_str().unwrap(),
                session["id"].as_str().unwrap(),
                session["label"].as_str().unwrap(),
                session["tabs"][0]["panes"]["Agent I"].as_str().unwrap(),
            ))
            .collect::<Vec<_>>(),
        [
            (primary.as_str(), "session:1", "core", "block:1-1-1"),
            (
                spike_text.as_str(),
                "session:2",
                "core · Dev spike one",
                "block:2-1-1"
            ),
        ]
    );

    fs::remove_file(fixture.state.join("sessions/1")).unwrap();
    let output = fixture.session(&["list"]);
    assert_eq!(
        stdout(&output),
        format!("core · Dev spike one  {spike_text}  session:2\n")
    );
}

#[test]
fn a_worktree_is_named_by_directory_or_reached_by_path() {
    let fixture = Fixture::new();
    let spike = fixture.linked("spike");
    let nested = spike.join("nested");
    fs::create_dir_all(&nested).unwrap();

    let output = fixture.run_in(&nested, &["session", "create", "."]);
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), "Created session core · Spike\n");
    assert_eq!(fixture.creates(), [expected_create("core · Spike", &spike)]);

    let output = fixture.session(&["create", "acme/core", "spike"]);
    assert_eq!(stdout(&output), "Session core · Spike already exists\n");

    let output = fixture.session(&["create", "acme/core", "other"]);
    assert_eq!(
        stderr(&output),
        format!(
            "error: no linked worktree of {} is named other\n  {}\n",
            fixture.primary.display(),
            spike.display()
        )
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn several_worktrees_with_one_name_are_listed() {
    let fixture = Fixture::new();
    let first = fixture.linked("spike");
    let second = fixture.home.join("elsewhere/spike");
    git(
        &fixture.primary,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            second.to_str().unwrap(),
        ],
    );

    let output = fixture.session(&["open", "acme/core", "spike"]);
    assert_eq!(
        stderr(&output),
        format!(
            "error: several linked worktrees are named spike\n  {}\n  {}\n",
            second.display(),
            first.display()
        )
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.calls(), Vec::<String>::new());
}

#[test]
fn a_missing_checkout_is_named() {
    let fixture = Fixture::new();
    let missing = fixture.base.join("nowhere");
    fs::create_dir_all(&missing).unwrap();

    let output = fixture.session(&["open", missing.to_str().unwrap()]);
    assert_eq!(
        stderr(&output),
        format!("error: not a Git checkout: {}\n", missing.display())
    );
    assert_eq!(output.status.code(), Some(1));

    let output = fixture.session(&["open", "acme/absent"]);
    assert_eq!(
        stderr(&output),
        format!(
            "error: not a Git checkout: {}\n",
            fixture.home.join("repos/acme/absent").display()
        )
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn without_a_session_section_no_rex_call_happens() {
    let fixture = Fixture::with_config("root = \"repos\"\n");
    for args in [
        &["open", "acme/core"][..],
        &["create", "acme/core", "--json"],
        &["close", "acme/core"],
        &["list"],
    ] {
        let output = fixture.session(args);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: sessions are not configured; add [session] to {}\n",
                fixture.config.display()
            )
        );
        assert_eq!(output.status.code(), Some(1));
    }
    fs::remove_file(&fixture.config).unwrap();
    let output = fixture.session(&["list"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.calls(), Vec::<String>::new());
}

#[test]
fn an_unreachable_host_is_unavailable() {
    let fixture = Fixture::new();
    fixture.mode("offline");
    let line = format!(
        "error: session host unavailable: no Rex server is running at {}, and autostart is disabled\n",
        fixture.state.join("server.sock").display()
    );
    for args in [
        &["open", "acme/core"][..],
        &["create", "acme/core"],
        &["close", "acme/core"],
        &["list"],
    ] {
        let output = fixture.session(args);
        assert_eq!(stdout(&output), "");
        assert_eq!(stderr(&output), line);
        assert_eq!(output.status.code(), Some(1));
    }
}

#[test]
fn an_unavailable_host_keeps_the_record() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    fixture.mode("offline");
    assert_eq!(fixture.session(&["list"]).status.code(), Some(1));
    fs::remove_file(fixture.state.join("offline")).unwrap();

    let output = fixture.session(&["create", "acme/core"]);
    assert_eq!(stdout(&output), "Session core already exists\n");
    assert_eq!(fixture.creates().len(), 1);
}

#[test]
fn a_malformed_session_list_keeps_the_record() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    fixture.mode("bad-list");

    let output = fixture.session(&["list"]);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "error: session host failed: rex listed no session ids\n"
    );
    assert_eq!(output.status.code(), Some(1));

    fs::remove_file(fixture.state.join("bad-list")).unwrap();
    let output = fixture.session(&["create", "acme/core"]);
    assert_eq!(stdout(&output), "Session core already exists\n");
    assert_eq!(fixture.creates().len(), 1);
}

#[test]
fn a_session_gone_during_close_counts_as_closed() {
    let fixture = Fixture::new();
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    fixture.mode("vanished");

    let output = fixture.session(&["close", "acme/core"]);
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), "Closed session core\n");
    assert!(output.status.success());

    fs::remove_file(fixture.state.join("vanished")).unwrap();
    assert_eq!(stdout(&fixture.session(&["list"])), "");
}

#[test]
fn concurrent_creates_make_one_session() {
    let fixture = Fixture::new();
    fixture.mode("slow-create");
    let children: Vec<_> = (0..2)
        .map(|_| {
            fixture
                .command(&fixture.base)
                .args(["session", "create", "acme/core"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut lines: Vec<_> = children
        .into_iter()
        .map(|child| stdout(&child.wait_with_output().unwrap()))
        .collect();
    lines.sort();
    assert_eq!(
        lines,
        ["Created session core\n", "Session core already exists\n"]
    );
    assert_eq!(fixture.creates().len(), 1);
}

#[test]
fn a_layout_without_focus_sends_none() {
    let fixture = Fixture::with_config(&format!(
        "root = \"repos\"\n{}",
        SESSION.replace("focus = \"Shells\"\n", "")
    ));
    assert!(fixture.session(&["create", "acme/core"]).status.success());
    assert_eq!(fixture.creates()[0]["focus"], Value::Null);
}

#[test]
fn a_removed_worktree_cannot_be_opened_but_its_session_closes() {
    let fixture = Fixture::new();
    let spike = fixture.linked("spike");
    assert!(
        fixture
            .session(&["create", "acme/core", "spike"])
            .status
            .success()
    );
    fs::remove_dir_all(&spike).unwrap();
    let calls = fixture.calls().len();

    for verb in ["open", "create"] {
        let output = fixture.session(&[verb, "acme/core", "spike"]);
        assert_eq!(
            stderr(&output),
            format!("error: not a Git checkout: {}\n", spike.display())
        );
        assert_eq!(output.status.code(), Some(1));
    }
    assert_eq!(fixture.calls().len(), calls);

    let output = fixture.session(&["close", "acme/core", "spike"]);
    assert_eq!(stdout(&output), "Closed session core · Spike\n");
    assert!(output.status.success());
}

#[test]
fn a_worktree_named_or_reached_by_path_has_one_session() {
    let fixture = Fixture::new();
    let listed = fixture.linked("spike");
    let container = listed.parent().unwrap();
    let moved = fixture.home.join("repos/acme/moved");
    fs::rename(container, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, container).unwrap();

    assert!(
        fixture
            .session(&["create", "acme/core", "spike"])
            .status
            .success()
    );
    let output = fixture.session(&["create", moved.join("spike").to_str().unwrap()]);
    assert_eq!(stdout(&output), "Session core · Spike already exists\n");
    assert_eq!(fixture.creates().len(), 1);

    fs::remove_dir_all(moved.join("spike")).unwrap();
    let output = fixture.session(&["close", "acme/core", "spike"]);
    assert_eq!(stdout(&output), "Closed session core · Spike\n");
}

#[test]
fn a_name_not_found_also_names_what_git_could_not_list() {
    let fixture = Fixture::new();
    let broken = fixture.primary.join(".git/worktrees/broken");
    fs::create_dir_all(&broken).unwrap();

    let output = fixture.session(&["open", "acme/core", "spike"]);
    let stderr = stderr(&output);
    let mut lines = stderr.lines();
    assert_eq!(
        lines.next(),
        Some(
            format!(
                "error: no linked worktree of {} is named spike",
                fixture.primary.display()
            )
            .as_str()
        )
    );
    assert!(
        lines
            .next()
            .is_some_and(|line| line.starts_with(&format!("error: {}: ", broken.display()))),
        "{stderr}"
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn a_failed_create_records_nothing() {
    let fixture = Fixture::new();
    fixture.mode("create-fails");

    let output = fixture.session(&["create", "acme/core", "--json"]);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "error: session host failed: stdin:4: session.create: stream response \"control-3\" failed: initial_windows[1]: block flavor not found: \"com.example.nonexistent.flavor\" (status=404 code=not_found)\n"
    );
    assert_eq!(output.status.code(), Some(1));

    fs::remove_file(fixture.state.join("create-fails")).unwrap();
    let output = fixture.session(&["create", "acme/core"]);
    assert_eq!(stdout(&output), "Created session core\n");
}

#[test]
fn an_invalid_layout_names_the_setting_at_fault() {
    let tab = |splits: &str| {
        format!(
            "[session.layouts.default.tabs.Main]\nroot = {{ pane = \"A\" }}\nsplits = [{splits}]\n"
        )
    };
    for (section, message) in [
        (
            String::new(),
            "session.layouts.default: is missing".to_owned(),
        ),
        (
            "[session.layouts.default]\n".to_owned(),
            "session.layouts.default: has no tabs".to_owned(),
        ),
        (
            format!("[session.layouts.default]\nfocus = \"Other\"\n{}", tab("")),
            "session.layouts.default: focus names no tab \"Other\"".to_owned(),
        ),
        (
            tab(r#"{ source = "B", pane = "C", direction = "right" }"#),
            "session.layouts.default: tab Main: no pane \"B\"".to_owned(),
        ),
        (
            tab(r#"{ source = "A", pane = "A", direction = "right" }"#),
            "session.layouts.default: tab Main: pane \"A\" is named twice".to_owned(),
        ),
        (
            tab(r#"{ source = "A", pane = "B", direction = "right", ratio = 1.5 }"#),
            "session.layouts.default: ratio 1.5 is not between 0 and 1".to_owned(),
        ),
    ] {
        let fixture = Fixture::with_config(&format!(
            "root = \"repos\"\n[session]\nhost = \"rex\"\n{section}"
        ));
        for (args, prefix) in [(&["session", "list"][..], "error"), (&["status"], "lager")] {
            let output = fixture.run_in(&fixture.base, args);
            assert_eq!(
                stderr(&output),
                format!("{prefix}: invalid config: {message}\n"),
                "{args:?}"
            );
            assert_eq!(output.status.code(), Some(1));
        }
    }
}

#[test]
fn an_unknown_host_or_direction_is_refused() {
    for (section, message) in [
        (
            "host = \"tmux\"\n",
            "unknown variant `tmux`, expected `rex`",
        ),
        (
            "host = \"rex\"\n[session.layouts.default.tabs.Main]\nroot = { pane = \"A\" }\nsplits = [{ source = \"A\", pane = \"B\", direction = \"diagonal\" }]\n",
            "unknown variant `diagonal`, expected one of `right`, `left`, `below`, `down`, `above`, `up`",
        ),
    ] {
        let fixture = Fixture::with_config(&format!("root = \"repos\"\n[session]\n{section}"));
        let output = fixture.session(&["list"]);
        assert_eq!(
            stderr(&output),
            format!(
                "error: could not parse config {}: session: {message}\n",
                fixture.config.display()
            )
        );
        assert_eq!(output.status.code(), Some(1));
    }
}
