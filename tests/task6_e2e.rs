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
    let alias = run(config, home, &[&["ls"], flags].concat());
    assert_eq!(alias.status.code(), listed.status.code());
    assert_eq!(alias.stdout, listed.stdout);
    // Clap may use the invoked name in usage text; all other diagnostics match.
    assert_eq!(
        String::from_utf8_lossy(&alias.stderr).replace("lager ls", "lager list"),
        String::from_utf8_lossy(&listed.stderr)
    );
    listed
}

#[test]
fn ls_matches_empty_list_in_human_and_json_formats() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;

    for flags in [&[][..], &["--json"][..]] {
        let listed = list_with_alias_parity(&config, &home, flags);
        assert!(listed.status.success());
        assert!(listed.stderr.is_empty());
        if flags.is_empty() {
            assert!(listed.stdout.is_empty());
        } else {
            let document: Value = serde_json::from_slice(&listed.stdout)?;
            assert_eq!(document["schema_version"], 1);
            assert_eq!(document["repositories"], serde_json::json!([]));
            assert_eq!(document["provider_errors"], serde_json::json!([]));
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
    fs::create_dir_all(&home)?;
    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/repo\"\npost_clone = \"echo hook\"\n\n[[repositories]]\nurl = \"github.com/team/*\"\nexclude = [\"skip\"]\n",
    )?;

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
    let document: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["repositories"].as_array().unwrap().len(), 2);
    assert_eq!(document["repositories"][0]["state"], "missing");
    assert!(document["repositories"][0]["hook"].as_bool().unwrap());
    assert_eq!(document["repositories"][1]["pattern"], "github.com/team/*");
    assert_eq!(document["repositories"][1]["exclusions"][0], "skip");
    assert!(document["provider_errors"].as_array().unwrap().is_empty());
    let human = list_with_alias_parity(&config, &home, &[]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("github.com/team/*"));
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
    fs::create_dir_all(root.join("conflict"))?;
    let cloned = root.join("cloned");
    for args in [
        vec!["init", cloned.to_str().unwrap()],
        vec![
            "-C",
            cloned.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@github.com:org/cloned.git",
        ],
    ] {
        let output = support::external(&home, "git").args(&args).output()?;
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    // An ancestor loop makes metadata inspection fail, even when run as root.
    symlink("blocked", home.join("repos/blocked"))?;
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/missing\"\n\n[[repositories]]\nurl = \"github.com/org/cloned\"\n\n[[repositories]]\nurl = \"github.com/org/conflict\"\n\n[[repositories]]\nurl = \"github.com/blocked/unreadable\"\n",
    )?;
    let output = list_with_alias_parity(&config, &home, &["--json"]);
    assert!(output.status.success());
    let document: Value = serde_json::from_slice(&output.stdout)?;
    let states: Vec<_> = document["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["missing", "cloned", "conflict", "unreadable"]);
    let human = list_with_alias_parity(&config, &home, &[]);
    assert!(human.status.success());
    for state in states {
        assert!(String::from_utf8_lossy(&human.stdout).contains(state));
    }
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
