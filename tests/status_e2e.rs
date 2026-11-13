mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use serde_json::{Value, json};

const CONFIG: &str = r#"root = "repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/org/clean"

[[repositories]]
url = "github.com/org/dirty"

[[repositories]]
url = "github.com/org/missing"

[[repositories]]
url = "github.com/org/moved"

[[repositories]]
url = "github.com/team/*"
exclude = ["skip"]
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn root(&self) -> PathBuf {
        self.home.join("repos")
    }

    fn path(&self, relative: &str) -> String {
        self.root().join(relative).to_str().unwrap().to_owned()
    }

    fn run(&self, args: &[&str]) -> Output {
        support::lager(&self.home, &self.config)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("lager process")
    }
}

fn empty_fixture(config: &str) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("repos")).unwrap();
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, config).unwrap();
    Fixture {
        _temp: temp,
        home,
        config: config_path,
    }
}

/// Every row kind: clean, dirty and missing declarations, a misplaced declared checkout, a
/// duplicate checkout of a declared origin, a wildcard member, an excluded member, an
/// unregistered checkout, an originless checkout and the wildcard pattern itself.
fn fixture() -> Fixture {
    let fixture = empty_fixture(CONFIG);
    let root = fixture.root();
    init_repo(
        &root.join("org/clean"),
        Some("git@github.com:org/clean.git"),
    );
    init_repo(
        &root.join("org/dirty"),
        Some("git@github.com:org/dirty.git"),
    );
    fs::write(root.join("org/dirty/README"), "changed\n").unwrap();
    init_repo(
        &root.join("elsewhere/moved"),
        Some("git@github.com:org/moved.git"),
    );
    init_repo(
        &root.join("copies/clean"),
        Some("git@github.com:org/clean.git"),
    );
    init_repo(
        &root.join("team/member"),
        Some("git@github.com:team/member.git"),
    );
    init_repo(
        &root.join("other/stray"),
        Some("git@github.com:other/stray.git"),
    );
    init_repo(
        &root.join("team/skip"),
        Some("git@github.com:team/skip.git"),
    );
    init_repo(&root.join("local/scratch"), None);
    fixture
}

fn run_git(directory: &Path, args: &[&str]) {
    // Fixture commits otherwise spawn a detached `git maintenance run` that outlives this call.
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(path: &Path, origin: Option<&str>) {
    fs::create_dir_all(path).unwrap();
    run_git(path, &["init", "-q"]);
    run_git(path, &["checkout", "-qb", "main"]);
    run_git(path, &["config", "user.email", "lager@example.invalid"]);
    run_git(path, &["config", "user.name", "lager"]);
    if let Some(origin) = origin {
        run_git(path, &["remote", "add", "origin", origin]);
    }
    fs::write(path.join("README"), "initial\n").unwrap();
    run_git(path, &["add", "README"]);
    run_git(path, &["commit", "-qm", "initial"]);
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}",
            String::from_utf8_lossy(&output.stdout).into_owned()
        )
    })
}

fn fact(state: &str, value: Option<&str>) -> Value {
    json!({"state": state, "value": value})
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn expected_rows(fixture: &Fixture) -> Vec<Value> {
    let path = |relative: &str| fixture.path(relative);
    let checked = |kind: &str,
                   identity: &str,
                   declaration: Option<&str>,
                   registration: &str,
                   pattern: Option<&str>,
                   found: &str,
                   configured: Option<String>,
                   origin: &str,
                   changes: Value,
                   warnings: Vec<String>| {
        json!({
            "kind": kind,
            "identity": identity,
            "declaration": declaration,
            "hook": null,
            "registration": registration,
            "pattern": pattern,
            "exclusions": null,
            "checkout": "cloned",
            "found_path": path(found),
            "configured_path": configured,
            "origin": fact("supported", Some(origin)),
            "branch": fact("known", Some("main")),
            "changes": changes,
            "archived": null,
            "warnings": warnings,
        })
    };
    let misplaced = |found: &str, configured: &str| {
        format!(
            "misplaced checkout; found: {}; configured: {}",
            path(found),
            path(configured)
        )
    };
    let missing = |identity: &str, relative: &str, origin: &str| {
        json!({
            "kind": "declaration",
            "identity": identity,
            "declaration": identity,
            "hook": null,
            "registration": "explicit",
            "pattern": null,
            "exclusions": null,
            "checkout": "missing",
            "found_path": null,
            "configured_path": path(relative),
            "origin": fact("declared", Some(origin)),
            "branch": fact("not_applicable", None),
            "changes": fact("not_applicable", None),
            "archived": null,
            "warnings": [],
        })
    };
    vec![
        checked(
            "declaration",
            "github.com/org/clean",
            Some("github.com/org/clean"),
            "explicit",
            None,
            "org/clean",
            Some(path("org/clean")),
            "git@github.com:org/clean.git",
            fact("clean", None),
            vec![],
        ),
        checked(
            "declaration",
            "github.com/org/dirty",
            Some("github.com/org/dirty"),
            "explicit",
            None,
            "org/dirty",
            Some(path("org/dirty")),
            "git@github.com:org/dirty.git",
            fact("dirty", Some("1 modified")),
            vec![],
        ),
        missing(
            "github.com/org/missing",
            "org/missing",
            "git@github.com:org/missing.git",
        ),
        missing(
            "github.com/org/moved",
            "org/moved",
            "git@github.com:org/moved.git",
        ),
        checked(
            "checkout",
            "github.com/org/clean",
            Some("github.com/org/clean"),
            "explicit",
            None,
            "copies/clean",
            Some(path("org/clean")),
            "git@github.com:org/clean.git",
            fact("clean", None),
            vec![misplaced("copies/clean", "org/clean")],
        ),
        checked(
            "checkout",
            "github.com/org/moved",
            Some("github.com/org/moved"),
            "explicit",
            None,
            "elsewhere/moved",
            Some(path("org/moved")),
            "git@github.com:org/moved.git",
            fact("clean", None),
            vec![misplaced("elsewhere/moved", "org/moved")],
        ),
        checked(
            "checkout",
            "github.com/other/stray",
            None,
            "unregistered",
            None,
            "other/stray",
            None,
            "git@github.com:other/stray.git",
            fact("clean", None),
            vec![],
        ),
        checked(
            "checkout",
            "github.com/team/member",
            None,
            "wildcard",
            Some("github.com/team/*"),
            "team/member",
            Some(path("team/member")),
            "git@github.com:team/member.git",
            fact("clean", None),
            vec!["covered by github.com/team/*".to_owned()],
        ),
        checked(
            "checkout",
            "github.com/team/skip",
            None,
            "excluded",
            Some("github.com/team/*"),
            "team/skip",
            None,
            "git@github.com:team/skip.git",
            fact("clean", None),
            vec!["excluded by github.com/team/*".to_owned()],
        ),
        json!({
            "kind": "path",
            "identity": null,
            "declaration": null,
            "hook": null,
            "registration": "unregistered",
            "pattern": null,
            "exclusions": null,
            "checkout": "cloned",
            "found_path": path("local/scratch"),
            "configured_path": null,
            "origin": fact("absent", None),
            "branch": fact("known", Some("main")),
            "changes": fact("clean", None),
            "archived": null,
            "warnings": ["Identity is the exact path"],
        }),
        json!({
            "kind": "pattern",
            "identity": "github.com/team/*",
            "declaration": "github.com/team/*",
            "hook": null,
            "registration": "pattern",
            "pattern": null,
            "exclusions": ["skip"],
            "checkout": "pattern",
            "found_path": null,
            "configured_path": path("team"),
            "origin": fact("not_applicable", None),
            "branch": fact("not_applicable", None),
            "changes": fact("not_applicable", None),
            "archived": null,
            "warnings": ["excludes skip"],
        }),
    ]
}

const EXPECTED_TABLE: &str = "\
repository    path                     registration  checkout  origin                  branch  changes
clean         ~/repos/org/clean        explicit      cloned    github.com/org/clean    main    clean
dirty         ~/repos/org/dirty        explicit      cloned    github.com/org/dirty    main    1 modified
missing       ~/repos/org/missing      explicit      missing   github.com/org/missing  —       —
moved         ~/repos/org/moved        explicit      missing   github.com/org/moved    —       —
clean (copy)  ~/repos/copies/clean     explicit      cloned    github.com/org/clean    main    clean
moved         ~/repos/elsewhere/moved  explicit      cloned    github.com/org/moved    main    clean
stray         ~/repos/other/stray      unregistered  cloned    github.com/other/stray  main    clean
member        ~/repos/team/member      wildcard      cloned    github.com/team/member  main    clean
skip          ~/repos/team/skip        excluded      cloned    github.com/team/skip    main    clean
scratch       ~/repos/local/scratch    unregistered  cloned    No origin               main    clean
team/*        —                        wildcard      —         github.com/team/*       —       —
";

#[test]
fn status_table_shows_every_row_kind_with_the_inventory_cells() {
    let fixture = fixture();
    let output = fixture.run(&["status"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output), EXPECTED_TABLE);
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn status_json_carries_every_fact_of_every_row() {
    let fixture = fixture();
    let output = fixture.run(&["status", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        json(&output),
        json!({
            "schema_version": 2,
            "complete": true,
            "rows": expected_rows(&fixture),
            "problems": [],
        })
    );
    let text = stdout(&output);
    let keys = [
        "\"kind\"",
        "\"identity\"",
        "\"declaration\"",
        "\"hook\"",
        "\"registration\"",
        "\"pattern\"",
        "\"exclusions\"",
        "\"checkout\"",
        "\"found_path\"",
        "\"configured_path\"",
        "\"origin\"",
        "\"branch\"",
        "\"changes\"",
        "\"archived\"",
        "\"warnings\"",
    ];
    let first_row = &text[text.find("\"rows\":[").unwrap()..];
    let positions = keys
        .iter()
        .map(|key| first_row.find(&format!("{key}:")).unwrap())
        .collect::<Vec<_>>();
    assert!(positions.is_sorted(), "key order: {text}");
    assert!(text.starts_with("{\"schema_version\":2,\"complete\":true,\"rows\":["));
    assert!(text.ends_with("],\"problems\":[]}\n"));
}

#[test]
fn status_list_and_ls_print_identical_bytes_and_exit_codes() {
    let fixture = fixture();
    for flags in [
        &[][..],
        &["--json"][..],
        &["--registered"][..],
        &["--json", "--registered"][..],
    ] {
        let status = fixture.run(&[&["status"], flags].concat());
        assert_eq!(status.status.code(), Some(0), "{flags:?}: {status:?}");
        for alias in ["list", "ls"] {
            let other = fixture.run(&[&[alias], flags].concat());
            assert_eq!(
                other.status.code(),
                status.status.code(),
                "{alias} {flags:?}"
            );
            assert_eq!(other.stdout, status.stdout, "{alias} {flags:?}");
            assert_eq!(other.stderr, status.stderr, "{alias} {flags:?}");
        }
    }
}

#[test]
fn registered_keeps_only_declared_rows_and_patterns() {
    let fixture = fixture();
    let table = fixture.run(&["status", "--registered"]);
    assert_eq!(table.status.code(), Some(0), "{table:?}");
    let expected = EXPECTED_TABLE
        .lines()
        .filter(|line| !line.contains("unregistered") && !line.contains("excluded"))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    assert_eq!(stdout(&table), expected);

    let document = json(&fixture.run(&["status", "--json", "--registered"]));
    let rows = expected_rows(&fixture)
        .into_iter()
        .filter(|row| {
            !["unregistered", "excluded"].contains(&row["registration"].as_str().unwrap())
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 8);
    assert_eq!(document["rows"], Value::Array(rows));
}

#[test]
fn empty_config_and_empty_root_print_the_header_and_an_empty_document() {
    let fixture = empty_fixture("root = \"repos\"\n");
    let table = fixture.run(&["status"]);
    assert_eq!(table.status.code(), Some(0), "{table:?}");
    assert_eq!(
        stdout(&table),
        "repository  path  registration  checkout  origin  branch  changes\n"
    );
    assert!(table.stderr.is_empty(), "{table:?}");
    let document = fixture.run(&["status", "--json"]);
    assert_eq!(document.status.code(), Some(0), "{document:?}");
    assert_eq!(
        stdout(&document),
        "{\"schema_version\":2,\"complete\":true,\"rows\":[],\"problems\":[]}\n"
    );
    assert!(document.stderr.is_empty(), "{document:?}");
}

#[cfg(unix)]
#[test]
fn partial_scan_prints_the_rows_reports_the_problem_and_exits_one() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = fixture();
    let blocked = fixture.root().join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let table = fixture.run(&["status"]);
    let document = fixture.run(&["status", "--json"]);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
    if fs::read_dir(&blocked).is_ok() && table.status.code() == Some(0) {
        // Privileged users read through mode 000; the scan really is complete.
        return;
    }

    assert_eq!(table.status.code(), Some(1), "{table:?}");
    assert_eq!(stdout(&table), EXPECTED_TABLE);
    assert_eq!(
        String::from_utf8_lossy(&table.stderr),
        "lager: ~/repos/blocked: unreadable path: Permission denied (os error 13)\n"
    );

    assert_eq!(document.status.code(), Some(1), "{document:?}");
    assert_eq!(document.stderr, table.stderr);
    assert_eq!(
        json(&document),
        json!({
            "schema_version": 2,
            "complete": false,
            "rows": expected_rows(&fixture),
            "problems": [{
                "source": "scan",
                "subject": fixture.path("blocked"),
                "message": "unreadable path: Permission denied (os error 13)",
            }],
        })
    );
}

#[test]
fn json_stays_one_document_on_stdout_when_the_config_warns() {
    let fixture = empty_fixture("root = \"repos\"\nfuture_key = true\n");
    let output = fixture.run(&["status", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        json(&output),
        json!({"schema_version": 2, "complete": true, "rows": [], "problems": []})
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("unknown config key `future_key`")
            .count(),
        1,
        "{output:?}"
    );
}

#[test]
fn control_characters_are_escaped_in_the_table_and_original_in_json() {
    let fixture = empty_fixture("root = \"repos\"\n");
    let name = "tab\there\u{1b}[31mred";
    let checkout = fixture.root().join(name);
    init_repo(&checkout, None);
    let table = fixture.run(&["status"]);
    assert_eq!(table.status.code(), Some(0), "{table:?}");
    let text = stdout(&table);
    assert!(!text.contains('\u{1b}') && !text.contains('\t'), "{text:?}");
    assert_eq!(
        text.lines().nth(1).unwrap(),
        "tab\\there\\x1b[31mred  ~/repos/tab\\there\\x1b[31mred  unregistered  cloned    No origin  main    clean"
    );
    let document = fixture.run(&["status", "--json"]);
    assert!(!document.stdout.contains(&0x1b), "{document:?}");
    let value = json(&document);
    assert_eq!(value["rows"][0]["found_path"], checkout.to_str().unwrap());
}

#[test]
fn status_leaves_a_stat_dirty_index_and_the_config_untouched() {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    let fixture = fixture();
    let checkout = fixture.root().join("org/clean");
    // A README whose mtime no longer matches its index entry makes a plain `git status` rewrite the index.
    fs::File::options()
        .write(true)
        .open(checkout.join("README"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
        .unwrap();
    let index = checkout.join(".git/index");
    let before = fs::metadata(&index).unwrap();
    let config = fs::read(&fixture.config).unwrap();

    for args in [&["status"][..], &["status", "--json"][..]] {
        let output = fixture.run(args);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    }

    let after = fs::metadata(&index).unwrap();
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    #[cfg(unix)]
    assert_eq!(
        (after.ino(), after.mtime_nsec()),
        (before.ino(), before.mtime_nsec())
    );
    assert!(!checkout.join(".git/index.lock").exists());
    assert_eq!(fs::read(&fixture.config).unwrap(), config);
}

#[test]
fn status_runs_without_a_terminal_and_never_draws_a_screen() {
    let fixture = fixture();
    for args in [&["status"][..], &["status", "--json"][..]] {
        let output = fixture.run(args);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(!output.stdout.contains(&0x1b), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn include_archived_requires_remote_before_reading_the_config() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.toml");
    for command in ["status", "list", "ls"] {
        let output = support::lager(temp.path(), &missing)
            .args([command, "--include-archived"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "lager: --include-archived requires --remote\n"
        );
    }
}

#[test]
fn a_missing_root_lists_as_empty_with_a_note_and_exits_zero() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/repo\"\n",
    )
    .unwrap();
    let note = "lager: ~/repos: root does not exist yet\n";
    for command in ["status", "list", "ls"] {
        let output = support::lager(&home, &config)
            .arg(command)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stderr), note);
        assert_eq!(
            stdout(&output),
            "repository  path              registration  checkout  origin               branch  changes\n\
             repo        ~/repos/org/repo  explicit      missing   github.com/org/repo  —       —\n"
        );
    }
    let output = support::lager(&home, &config)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), note);
    let document = json(&output);
    assert_eq!(document["complete"], true);
    assert_eq!(document["rows"].as_array().unwrap().len(), 1);
    assert_eq!(document["rows"][0]["checkout"], "missing");
    assert_eq!(document["problems"], json!([]));
    assert_eq!(
        document["notes"],
        json!([{
            "source": "scan",
            "subject": home.join("repos").to_str().unwrap(),
            "message": "root does not exist yet",
        }])
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_root_symlink_lists_as_missing_and_a_link_to_a_file_is_a_problem() {
    let fixture = empty_fixture("root = \"repos\"\n");
    let root = fixture.root();
    fs::remove_dir(&root).unwrap();
    std::os::unix::fs::symlink(fixture.home.join("nowhere"), &root).unwrap();
    let output = fixture.run(&["status", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "lager: ~/repos: root does not exist yet\n"
    );
    assert_eq!(
        json(&output)["notes"][0]["message"],
        "root does not exist yet"
    );

    fs::write(fixture.home.join("nowhere"), "a file\n").unwrap();
    let output = fixture.run(&["status"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "lager: ~/repos: root is not a directory\n"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_root_is_a_problem_and_exits_one() {
    use std::os::unix::fs::PermissionsExt;

    let fixture =
        empty_fixture("root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/repo\"\n");
    let root = fixture.root();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let output = fixture.run(&["status", "--json"]);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    if output.status.code() == Some(0) {
        // Privileged users read through mode 000; the scan really is complete.
        return;
    }
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let message = "unreadable path: Permission denied (os error 13)";
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("lager: ~/repos: {message}\n")
    );
    let document = json(&output);
    assert_eq!(document["complete"], false);
    assert_eq!(document["rows"][0]["checkout"], "unreadable");
    assert_eq!(
        document["problems"],
        json!([{"source": "scan", "subject": root.to_str().unwrap(), "message": message}])
    );
    assert!(document.get("notes").is_none(), "{document}");
}

#[test]
fn a_root_that_is_a_file_is_a_problem_and_exits_one() {
    let fixture = empty_fixture("root = \"repos\"\n");
    let root = fixture.root();
    fs::remove_dir(&root).unwrap();
    fs::write(&root, "not a directory\n").unwrap();
    let message = "root is not a directory";
    for command in ["status", "ls"] {
        let output = fixture.run(&[command]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("lager: ~/repos: {message}\n")
        );
    }
    let document = json(&fixture.run(&["status", "--json"]));
    assert_eq!(document["complete"], false);
    assert_eq!(
        document["problems"],
        json!([{"source": "scan", "subject": root.to_str().unwrap(), "message": message}])
    );
}

#[cfg(unix)]
#[test]
fn a_failed_git_observation_is_a_problem_and_exits_one() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = empty_fixture("root = \"repos\"\n");
    let checkout = fixture.root().join("org/repo");
    init_repo(&checkout, Some("git@github.com:org/repo.git"));
    let tools = fixture._temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$3\" = status ]; then echo 'injected probe failure' >&2; exit 1; fi\nexec '{}' \"$@\"\n",
            support::real_tool("git").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&tools))
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = "git command failed (exit Some(1)): injected probe failure";
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("lager: ~/repos/org/repo: status: {error}\n")
    );
    let document = json(&output);
    assert_eq!(document["complete"], true);
    assert_eq!(document["rows"][0]["changes"], fact("error", Some(error)));
    assert_eq!(document["rows"][0]["branch"], fact("known", Some("main")));
    assert_eq!(
        document["problems"],
        json!([{
            "source": "probe",
            "subject": checkout.to_str().unwrap(),
            "message": format!("status: {error}"),
        }])
    );
}

/// A `gh` stub lists five members of `github.com/org/*`: one excluded, one archived, one also
/// declared explicitly, one cloned locally and one only on the provider. A second provider
/// refuses every connection.
#[cfg(unix)]
fn remote_fixture() -> (Fixture, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let fixture = empty_fixture(
        r#"root = "repos"

[providers."github.com"]
preset = "github"

[providers."dc.example"]
preset = "bitbucket-data-center"
api_url = "http://127.0.0.1:9"

[[repositories]]
url = "github.com/org/explicit"

[[repositories]]
url = "github.com/org/*"
exclude = ["skip"]

[[repositories]]
url = "dc.example/project/*"
"#,
    );
    let bin = fixture._temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let node = |name: &str, archived: bool| {
        format!(
            r#"{{"name":"{name}","isArchived":{archived},"sshUrl":"git@github.com:org/{name}.git","owner":{{"login":"org"}}}}"#
        )
    };
    let nodes = [
        node("active", false),
        node("old", true),
        node("skip", false),
        node("explicit", false),
        node("local", false),
    ]
    .join(",");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n[{{\"data\":{{\"organization\":{{\"repositories\":{{\"nodes\":[{nodes}],\"pageInfo\":{{\"hasNextPage\":false,\"endCursor\":null}}}}}}}}}}]\nJSON\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    init_repo(
        &fixture.root().join("org/local"),
        Some("git@github.com:org/local.git"),
    );
    (fixture, bin)
}

#[cfg(unix)]
fn run_remote(fixture: &Fixture, bin: &Path, args: &[&str]) -> Output {
    support::lager_with_path(&fixture.home, &fixture.config, Some(bin))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn remote_status_merges_members_applies_exclusions_and_keeps_rows_when_a_provider_fails() {
    let (fixture, bin) = remote_fixture();
    let remote_only = |name: &str, archived: bool| {
        json!({
            "kind": "declaration",
            "identity": format!("github.com/org/{name}"),
            "declaration": null,
            "hook": null,
            "registration": "wildcard",
            "pattern": "github.com/org/*",
            "exclusions": null,
            "checkout": "remote_only",
            "found_path": null,
            "configured_path": fixture.path(&format!("org/{name}")),
            "origin": fact("declared", Some(&format!("git@github.com:org/{name}.git"))),
            "branch": fact("not_applicable", None),
            "changes": fact("not_applicable", None),
            "archived": archived,
            "warnings": ["covered by github.com/org/*"],
        })
    };
    let explicit = json!({
        "kind": "declaration",
        "identity": "github.com/org/explicit",
        "declaration": "github.com/org/explicit",
        "hook": null,
        "registration": "explicit",
        "pattern": null,
        "exclusions": null,
        "checkout": "missing",
        "found_path": null,
        "configured_path": fixture.path("org/explicit"),
        "origin": fact("declared", Some("git@github.com:org/explicit.git")),
        "branch": fact("not_applicable", None),
        "changes": fact("not_applicable", None),
        "archived": false,
        "warnings": ["covered by github.com/org/*"],
    });
    let local = json!({
        "kind": "checkout",
        "identity": "github.com/org/local",
        "declaration": null,
        "hook": null,
        "registration": "wildcard",
        "pattern": "github.com/org/*",
        "exclusions": null,
        "checkout": "cloned",
        "found_path": fixture.path("org/local"),
        "configured_path": fixture.path("org/local"),
        "origin": fact("supported", Some("git@github.com:org/local.git")),
        "branch": fact("known", Some("main")),
        "changes": fact("clean", None),
        "archived": false,
        "warnings": ["covered by github.com/org/*"],
    });
    let pattern = |identity: &str, destination: &str, exclusions: Value, warnings: Value| {
        json!({
            "kind": "pattern",
            "identity": identity,
            "declaration": identity,
            "hook": null,
            "registration": "pattern",
            "pattern": null,
            "exclusions": exclusions,
            "checkout": "pattern",
            "found_path": null,
            "configured_path": fixture.path(destination),
            "origin": fact("not_applicable", None),
            "branch": fact("not_applicable", None),
            "changes": fact("not_applicable", None),
            "archived": null,
            "warnings": warnings,
        })
    };
    let patterns = [
        pattern("dc.example/project/*", "project", json!([]), json!([])),
        pattern(
            "github.com/org/*",
            "org",
            json!(["skip"]),
            json!(["excludes skip"]),
        ),
    ];

    let output = run_remote(&fixture, &bin, &["status", "--remote", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let document = json(&output);
    assert_eq!(
        document["rows"],
        json!([
            remote_only("active", false),
            explicit.clone(),
            local.clone(),
            patterns[0].clone(),
            patterns[1].clone(),
        ])
    );
    assert_eq!(document["complete"], false);
    let problems = document["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{document}");
    assert_eq!(problems[0]["source"], "provider");
    assert_eq!(problems[0]["subject"], "dc.example");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("lager: dc.example: "), "{stderr}");

    let archived = run_remote(
        &fixture,
        &bin,
        &["status", "--remote", "--include-archived", "--json"],
    );
    assert_eq!(archived.status.code(), Some(1), "{archived:?}");
    assert_eq!(
        json(&archived)["rows"],
        json!([
            remote_only("active", false),
            explicit,
            remote_only("old", true),
            local,
            patterns[0].clone(),
            patterns[1].clone(),
        ])
    );

    let table = run_remote(&fixture, &bin, &["status", "--remote"]);
    assert_eq!(table.status.code(), Some(1), "{table:?}");
    assert_eq!(table.stderr, output.stderr);
    assert_eq!(
        stdout(&table),
        "\
repository  path                  registration  checkout     origin                   branch  changes
active      ~/repos/org/active    wildcard      remote only  github.com/org/active    —       —
explicit    ~/repos/org/explicit  explicit      missing      github.com/org/explicit  —       —
local       ~/repos/org/local     wildcard      cloned       github.com/org/local     main    clean
project/*   —                     wildcard      —            dc.example/project/*     —       —
org/*       —                     wildcard      —            github.com/org/*         —       —
"
    );
    let archived_table = run_remote(
        &fixture,
        &bin,
        &["status", "--remote", "--include-archived"],
    );
    assert_eq!(archived_table.status.code(), Some(1), "{archived_table:?}");
    assert_eq!(
        stdout(&archived_table),
        "\
repository      path                  registration  checkout     origin                   branch  changes
active          ~/repos/org/active    wildcard      remote only  github.com/org/active    —       —
explicit        ~/repos/org/explicit  explicit      missing      github.com/org/explicit  —       —
old [archived]  ~/repos/org/old       wildcard      remote only  github.com/org/old       —       —
local           ~/repos/org/local     wildcard      cloned       github.com/org/local     main    clean
project/*       —                     wildcard      —            dc.example/project/*     —       —
org/*           —                     wildcard      —            github.com/org/*         —       —
"
    );
    for alias in ["list", "ls"] {
        for flags in [&["--remote"][..], &["--remote", "--json"][..]] {
            let reference = run_remote(&fixture, &bin, &[&["status"], flags].concat());
            let other = run_remote(&fixture, &bin, &[&[alias], flags].concat());
            assert_eq!(other.status.code(), reference.status.code());
            assert_eq!(other.stdout, reference.stdout, "{alias} {flags:?}");
            assert_eq!(other.stderr, reference.stderr, "{alias} {flags:?}");
        }
    }
}
