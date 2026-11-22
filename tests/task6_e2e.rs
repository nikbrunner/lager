mod support;

use std::fs;

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn list_with_alias_parity(
    config: &std::path::Path,
    home: &std::path::Path,
    flags: &[&str],
) -> std::process::Output {
    let listed = run(config, home, &[&["list"], flags].concat());
    for name in ["ls", "status"] {
        let alias = run(config, home, &[&[name], flags].concat());
        assert_eq!(alias.status.code(), listed.status.code());
        assert_eq!(alias.stdout, listed.stdout);
        // Clap may use the invoked name in usage text; all other diagnostics match.
        assert_eq!(
            String::from_utf8_lossy(&alias.stderr).replace(&format!("lager {name}"), "lager list"),
            String::from_utf8_lossy(&listed.stderr)
        );
    }
    listed
}

#[test]
fn ls_matches_empty_list_in_human_and_json_formats() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos"))?;
    fs::write(&config, "root = \"repos\"\n")?;

    for flags in [&[][..], &["--json"][..]] {
        let listed = list_with_alias_parity(&config, &home, flags);
        assert!(listed.status.success());
        assert!(listed.stderr.is_empty());
        if flags.is_empty() {
            assert_eq!(
                String::from_utf8(listed.stdout)?,
                "repository  local  branch  status  registration\n"
            );
        } else {
            let document: Value = serde_json::from_slice(&listed.stdout)?;
            assert_eq!(
                document,
                serde_json::json!({
                    "schema_version": 2,
                    "complete": true,
                    "rows": [],
                    "problems": [],
                })
            );
        }
    }
    Ok(())
}

fn run(config: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    support::lager(home, config)
        .args(args)
        .output()
        .expect("lager process")
}

#[test]
fn offline_json_list_is_single_versioned_document_with_wildcard_metadata() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos"))?;
    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/repo\"\npost_clone = \"echo hook\"\n\n[[repositories]]\nurl = \"github.com/team/*\"\nexclude = [\"skip\"]\n",
    )?;
    let destination = |relative: &str| {
        home.join("repos")
            .join(relative)
            .to_str()
            .unwrap()
            .to_owned()
    };
    let not_applicable = serde_json::json!({"state": "not_applicable", "value": null});

    let output = list_with_alias_parity(&config, &home, &["--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout.clone())?.lines().count(), 1);
    let document: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        document,
        serde_json::json!({
            "schema_version": 2,
            "complete": true,
            "rows": [
                {
                    "kind": "declaration",
                    "identity": "github.com/org/repo",
                    "declaration": "github.com/org/repo",
                    "hook": "echo hook",
                    "registration": "explicit",
                    "pattern": null,
                    "exclusions": null,
                    "checkout": "missing",
                    "found_path": null,
                    "configured_path": destination("org/repo"),
                    "origin": {"state": "declared", "value": "git@github.com:org/repo.git"},
                    "branch": not_applicable,
                    "changes": not_applicable,
                    "archived": null,
                    "remote": null,
                    "warnings": [],
                },
                {
                    "kind": "pattern",
                    "identity": "github.com/team/*",
                    "declaration": "github.com/team/*",
                    "hook": null,
                    "registration": "pattern",
                    "pattern": null,
                    "exclusions": ["skip"],
                    "checkout": "pattern",
                    "found_path": null,
                    "configured_path": destination("team"),
                    "origin": not_applicable,
                    "branch": not_applicable,
                    "changes": not_applicable,
                    "archived": null,
                    "remote": null,
                    "warnings": ["excludes skip"],
                },
            ],
            "problems": [],
        })
    );
    let human = list_with_alias_parity(&config, &home, &[]);
    assert!(human.status.success());
    assert_eq!(
        String::from_utf8(human.stdout)?,
        "\
repository  local    branch  status  registration
repo        missing  —       —       explicit
"
    );
    Ok(())
}

#[test]
fn ls_matches_list_configuration_failures() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    for contents in [
        None,
        Some("not valid TOML = ["),
        Some("root = \"/absolute\"\n"),
    ] {
        if let Some(contents) = contents {
            fs::write(&config, contents)?;
        }
        for flags in [&[][..], &["--json"][..]] {
            let output = list_with_alias_parity(&config, &home, flags);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn ls_matches_offline_list_for_every_local_state() -> TestResult {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let root = home.join("repos/org");
    fs::create_dir_all(root.join("plain"))?;
    for (directory, origin) in [("cloned", "cloned"), ("conflict", "other")] {
        let checkout = root.join(directory);
        let origin = format!("git@github.com:org/{origin}.git");
        for args in [
            vec!["init", checkout.to_str().unwrap()],
            vec![
                "-C",
                checkout.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                &origin,
            ],
        ] {
            let output = support::external(&home, "git").args(&args).output()?;
            assert!(output.status.success(), "{args:?}: {output:?}");
        }
    }
    // An ancestor loop makes metadata inspection fail, even when run as root.
    symlink("blocked", home.join("repos/blocked"))?;
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/missing\"\n\n[[repositories]]\nurl = \"github.com/org/cloned\"\n\n[[repositories]]\nurl = \"github.com/org/conflict\"\n\n[[repositories]]\nurl = \"github.com/org/plain\"\n\n[[repositories]]\nurl = \"github.com/blocked/unreadable\"\n",
    )?;
    let output = list_with_alias_parity(&config, &home, &["--json"]);
    assert!(output.status.success(), "{output:?}");
    let document: Value = serde_json::from_slice(&output.stdout)?;
    let states: Vec<_> = document["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["kind"].as_str().unwrap(),
                row["identity"].as_str().unwrap(),
                row["checkout"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            ("declaration", "github.com/blocked/unreadable", "unreadable"),
            ("declaration", "github.com/org/cloned", "cloned"),
            ("declaration", "github.com/org/conflict", "conflict"),
            ("declaration", "github.com/org/missing", "missing"),
            ("declaration", "github.com/org/plain", "unknown"),
            ("checkout", "github.com/org/other", "cloned"),
        ]
    );
    let human = list_with_alias_parity(&config, &home, &[]);
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout)?;
    let locals: Vec<_> = human
        .lines()
        .skip(1)
        .map(|line| {
            line.split("  ")
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .nth(1)
                .unwrap()
        })
        .collect();
    assert_eq!(
        locals,
        [
            "~/repos/blocked/unreadable · unreadable",
            "~/repos/org/cloned",
            "~/repos/org/conflict · conflict",
            "missing",
            "~/repos/org/plain · unknown",
            "~/repos/org/conflict"
        ],
        "{human}"
    );
    Ok(())
}

#[test]
fn archived_filter_requires_remote_and_legacy_commands_are_rejected() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;

    let invalid = list_with_alias_parity(&config, &home, &["--include-archived"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--remote"));
    let legacy = run(&config, &home, &["clone", "github.com/org/repo"]);
    assert_eq!(legacy.status.code(), Some(2));
    let old_add_flag = run(&config, &home, &["add", "github.com/org/repo", "--add"]);
    assert_eq!(old_add_flag.status.code(), Some(2));
    let old_remove_flag = run(
        &config,
        &home,
        &["remove", "github.com/org/repo", "--remove"],
    );
    assert_eq!(old_remove_flag.status.code(), Some(2));
    Ok(())
}
