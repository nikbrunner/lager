mod support;

use std::fs;

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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

    let output = run(&config, &home, &["list", "--json"]);
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
    Ok(())
}

#[test]
fn archived_filter_requires_remote_and_legacy_commands_are_rejected() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;

    let invalid = run(&config, &home, &["list", "--include-archived"]);
    assert_eq!(invalid.status.code(), Some(2));
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
