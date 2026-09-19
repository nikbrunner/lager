mod support;

use std::fs;

#[test]
fn invalid_later_reference_leaves_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "# untouched\nroot = \"repos\"\nfuture_key = \"keep\"\n";
    fs::write(&config, original).unwrap();
    let output = support::lager(temp.path(), &config)
        .args(["register", "github.com/org/first", "invalid"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(config).unwrap(), original);
}

#[test]
fn invalid_final_configuration_rolls_back_all_edits() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "# keep exact formatting\nroot = 'repos'\n";
    fs::write(&config, original).unwrap();
    let output = support::lager(temp.path(), &config)
        .args(["register", "org/first", "unknown.example/org/*"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_to_string(config).unwrap(), original);
}

#[test]
fn atomic_store_rejects_invalid_later_and_conflicting_per_request_hooks() {
    use lager::application::ports::{AtomicRegistrationStore, RegistrationRequest};
    use lager::infrastructure::config::FileConfigStore;

    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    for reference in ["invalid", "git@github.com:org/first.git"] {
        let requests = [
            RegistrationRequest {
                reference: "org/first".into(),
                post_clone: Some("first".into()),
            },
            RegistrationRequest {
                reference: reference.into(),
                post_clone: Some("second".into()),
            },
        ];
        assert!(FileConfigStore.register_batch(&config, &requests).is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
    }
}

#[test]
fn conflicting_later_hook_rolls_back_and_equivalent_duplicates_are_ordered() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\nfuture_key = \"keep\"\n\n[[repositories]]\nurl = \"org/existing\"\npost_clone = \"old\"\nfuture_repo_key = 42\n";
    fs::write(&config, original).unwrap();
    let output = support::lager(temp.path(), &config)
        .args(["register", "org/new", "org/existing", "--post-clone", "new"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_to_string(&config).unwrap(), original);

    let args = [
        "register",
        "org/new",
        "git@github.com:org/new.git",
        "github.com/org/other",
    ];
    let output = support::lager(temp.path(), &config)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"added\nalready managed\nadded\n");
    let saved = fs::read(&config).unwrap();
    let text = String::from_utf8(saved.clone()).unwrap();
    assert!(text.contains("future_key = \"keep\""));
    assert!(text.contains("future_repo_key = 42"));
    let output = support::lager(temp.path(), &config)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"already managed\nalready managed\nalready managed\n"
    );
    assert_eq!(fs::read(&config).unwrap(), saved);
}

#[cfg(unix)]
#[test]
fn atomic_batches_preserve_symlink_and_target_mode_on_success_and_abort() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.toml");
    let config = temp.path().join("config.toml");
    let original =
        "root = \"repos\"\n[[repositories]]\nurl = \"org/existing\"\npost_clone = \"old\"\n";
    fs::write(&target, original).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("target.toml", &config).unwrap();
    for args in [
        vec!["register", "org/new", "invalid"],
        vec![
            "register",
            "org/new",
            "org/existing",
            "--post-clone",
            "conflict",
        ],
    ] {
        let output = support::lager(temp.path(), &config)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        assert_eq!(
            fs::read_link(&config).unwrap(),
            std::path::Path::new("target.toml")
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    let output = support::lager(temp.path(), &config)
        .args(["register", "org/new", "org/other"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_link(config).unwrap(),
        std::path::Path::new("target.toml")
    );
    assert_eq!(
        fs::metadata(target).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn overlapping_wildcards_restore_all_exclusions_and_keep_explicit_hook() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n[providers.\"example.com\"]\npreset = \"bitbucket-data-center\"\napi_url = \"https://example.com\"\n[[repositories]]\nurl = \"example.com/org/*\"\n[[repositories]]\nurl = \"example.com/org/sub/*\"\n").unwrap();
    let run = |args: &[&str]| {
        let output = support::lager(temp.path(), &config)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    };
    for _ in 0..2 {
        run(&["unregister", "example.com/org/sub/repo"]);
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("exclude = [\"sub/repo\"]"));
        assert!(text.contains("exclude = [\"repo\"]"));
        run(&[
            "register",
            "example.com/org/sub/repo",
            "--post-clone",
            "make setup",
        ]);
        let saved = fs::read(&config).unwrap();
        let parsed: toml_edit::DocumentMut =
            String::from_utf8(saved.clone()).unwrap().parse().unwrap();
        let declarations = parsed["repositories"].as_array_of_tables().unwrap();
        assert_eq!(declarations.len(), 3);
        assert!(
            declarations
                .iter()
                .all(|item| !item.contains_key("exclude"))
        );
        assert_eq!(
            declarations
                .iter()
                .filter(|item| item.get("post_clone").is_some())
                .count(),
            1
        );
        assert_eq!(
            declarations.iter().nth(2).unwrap()["post_clone"].as_str(),
            Some("make setup")
        );
        let output = run(&["register", "example.com/org/sub/repo"]);
        assert_eq!(output.stdout, b"already managed\n");
        assert_eq!(fs::read(&config).unwrap(), saved);
    }
}

#[cfg(unix)]
fn interactive_batch(cancel: bool) {
    use std::io::{Read, Write};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.toml");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\nfuture_key = \"keep\"\n";
    fs::write(&target, original).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("target.toml", &config).unwrap();
    let pty = rustix_openpty::openpty(None, None).unwrap();
    let mut writer = fs::File::from(pty.controller);
    let mut reader = writer.try_clone().unwrap();
    let terminal = fs::File::from(pty.user);
    let mut command = support::lager(temp.path(), &config);
    command
        .args(["register", "org/first", "org/second"])
        .env("TERM", "xterm")
        .stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.try_clone().unwrap()))
        .stderr(Stdio::from(terminal));
    let mut child = command.spawn().unwrap();
    drop(command);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(length) = reader.read(&mut buffer) {
            if length == 0 || sender.send(buffer[..length].to_vec()).is_err() {
                break;
            }
        }
    });
    let steps: Vec<(&str, &[u8])> = if cancel {
        vec![
            ("Configure a post-clone hook for org/first?", b"y\r"),
            ("Post-clone command for org/first?", b"echo first\r"),
            ("Configure a post-clone hook for org/second?", b"\x03"),
        ]
    } else {
        vec![
            ("Configure a post-clone hook for org/first?", b"y\r"),
            ("Post-clone command for org/first?", b"echo first\r"),
            ("Configure a post-clone hook for org/second?", b"y\r"),
            ("Post-clone command for org/second?", b"echo second\r"),
        ]
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut transcript = String::new();
    let mut next = 0;
    let mut latest = original.to_owned();
    let status = loop {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("PTY timed out at step {next}: {transcript}");
        }
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
            transcript.push_str(&String::from_utf8_lossy(&bytes));
        }
        if next < steps.len() && transcript.contains(steps[next].0) {
            // The entire batch must remain uncommitted while any prompt is open.
            assert_eq!(fs::read_to_string(&target).unwrap(), latest);
            if !cancel && next == 2 {
                // Another process can commit while the prompt is open. The
                // staged batch must subsequently edit this latest document.
                let mut concurrent = support::lager(temp.path(), &config)
                    .args(["register", "org/concurrent"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                loop {
                    if let Some(status) = concurrent.try_wait().unwrap() {
                        assert!(status.success());
                        break;
                    }
                    if Instant::now() >= deadline {
                        concurrent.kill().unwrap();
                        concurrent.wait().unwrap();
                        child.kill().unwrap();
                        child.wait().unwrap();
                        panic!("registration held the config lock during prompts");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                latest = fs::read_to_string(&target).unwrap();
                assert!(latest.contains("org/concurrent"));
            }
            writer.write_all(steps[next].1).unwrap();
            writer.flush().unwrap();
            next += 1;
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
    };
    assert_eq!(next, steps.len(), "{transcript}");
    assert_eq!(
        status.code(),
        Some(if cancel { 130 } else { 0 }),
        "{transcript}"
    );
    assert_eq!(
        fs::read_link(config).unwrap(),
        std::path::Path::new("target.toml")
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let text = fs::read_to_string(target).unwrap();
    if cancel {
        assert_eq!(text, original);
    } else {
        let parsed: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(parsed["future_key"].as_str(), Some("keep"));
        let declarations = parsed["repositories"].as_array_of_tables().unwrap();
        assert_eq!(declarations.len(), 3);
        assert_eq!(
            declarations.iter().next().unwrap()["url"].as_str(),
            Some("org/concurrent")
        );
        assert_eq!(
            declarations.iter().nth(1).unwrap()["post_clone"].as_str(),
            Some("echo first")
        );
        assert_eq!(
            declarations.iter().nth(2).unwrap()["post_clone"].as_str(),
            Some("echo second")
        );
    }
}

#[cfg(unix)]
#[test]
fn interactive_hooks_are_staged_and_saved_together() {
    interactive_batch(false);
}

#[cfg(unix)]
#[test]
fn later_prompt_cancellation_preserves_original_bytes_link_and_mode() {
    interactive_batch(true);
}
