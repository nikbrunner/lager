mod support;

use std::fs;

#[cfg(unix)]
fn guarded_lager(home: &std::path::Path, config: &std::path::Path) -> std::process::Command {
    use std::os::unix::fs::PermissionsExt;
    let tools = home.join("guard-tools");
    fs::create_dir_all(&tools).unwrap();
    for tool in ["git", "gh", "fzf"] {
        let path = tools.join(tool);
        fs::write(
            &path,
            "#!/bin/sh\nprintf attempted >> \"$ATTEMPT\"\nexit 99\n",
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut command = support::lager_with_path(home, config, Some(&tools));
    command.env("ATTEMPT", home.join("attempt"));
    command
}

#[cfg(unix)]
#[test]
fn unsafe_later_arguments_and_loaded_config_fail_before_interactive_prompts() {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    for loaded in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let unsafe_reference = "https://SENTINEL@github.com/org/second";
        let original = if loaded {
            format!("root = 'repos'\n[[repositories]]\nurl = '{unsafe_reference}'\n")
        } else {
            "root = 'repos'\n".to_owned()
        };
        fs::write(&config, &original).unwrap();
        let pty = rustix_openpty::openpty(None, None).unwrap();
        let mut reader = fs::File::from(pty.controller);
        let terminal = fs::File::from(pty.user);
        let mut command = guarded_lager(temp.path(), &config);
        command
            .args([
                "register",
                "org/first",
                if loaded {
                    "org/second"
                } else {
                    unsafe_reference
                },
            ])
            .env("TERM", "xterm")
            .stdin(Stdio::from(terminal.try_clone().unwrap()))
            .stdout(Stdio::from(terminal.try_clone().unwrap()))
            .stderr(Stdio::from(terminal));
        let mut child = command.spawn().unwrap();
        drop(command);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("credential preflight waited for interactive input");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let bytes = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let transcript = String::from_utf8_lossy(&bytes);
        assert_eq!(status.code(), Some(1), "{transcript}");
        assert!(!transcript.contains("SENTINEL"), "{transcript}");
        assert!(
            !transcript.contains("Configure a post-clone"),
            "{transcript}"
        );
        assert!(!temp.path().join("attempt").exists());
        assert_eq!(fs::read_to_string(config).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn provider_credentials_are_rejected_instead_of_silently_materialized() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "root = 'repos'\n[providers.'github.com']\npreset = 'github'\n[[repositories]]\nurl = 'github.com/org/*'\n";
    fs::write(&config, original).unwrap();
    let tools = temp.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let gh = tools.join("gh");
    fs::write(&gh, "#!/bin/sh\nprintf '%s' '[{\"data\":{\"organization\":{\"repositories\":{\"nodes\":[{\"name\":\"repo\",\"owner\":{\"login\":\"org\"},\"isArchived\":false,\"sshUrl\":\"ssh://git:SENTINEL@github.com/org/repo\"}],\"pageInfo\":{\"hasNextPage\":false}}}}}]'\n").unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    for archived in [false, true] {
        if archived {
            let response = fs::read_to_string(&gh)
                .unwrap()
                .replace("isArchived\":false", "isArchived\":true");
            fs::write(&gh, response).unwrap();
        }
        let output = support::lager_with_path(temp.path(), &config, Some(&tools))
            .args(["list", "--remote", "--json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if archived { 0 } else { 1 }),
            "{output:?}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SENTINEL"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SENTINEL"));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["repositories"].as_array().unwrap().len(), 0);
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn clone_argv_and_full_pipeline_preserve_transport_and_equivalent_identity() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    ] {
        let output = support::external(temp.path(), "git")
            .current_dir(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let tools = temp.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(&wrapper, format!(
        "#!/bin/sh\nif [ \"$1\" = clone ]; then\n printf '%s\\n' \"$@\" >> \"$CAPTURE\"\n '{}' clone '{}' \"$3\" || exit $?\n '{}' remote set-url origin \"$2\"\nelse\n exec '{}' \"$@\"\nfi\n",
        support::real_tool("git").display(), source.display(),
        support::real_tool("git").display(), support::real_tool("git").display()
    )).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let file_url = format!("file://{}", source.display());
    for (index, (input, clone_url, equivalent)) in [
        (
            "https://github.com/org/repo",
            "https://github.com/org/repo",
            "org/repo",
        ),
        (
            "https://github.com/org/repo.git",
            "https://github.com/org/repo.git",
            "org/repo",
        ),
        (
            "http://bitbucket.org/org/repo",
            "http://bitbucket.org/org/repo",
            "git@bitbucket.org:org/repo.git",
        ),
        (
            "http://bitbucket.org/org/repo.git",
            "http://bitbucket.org/org/repo.git",
            "git@bitbucket.org:org/repo",
        ),
        (
            "ssh://alice@git.example:2222/org/repo",
            "ssh://alice@git.example:2222/org/repo",
            "git@git.example:org/repo.git",
        ),
        (
            "alice@git.example:org/repo.git",
            "alice@git.example:org/repo.git",
            "ssh://git@git.example/org/repo",
        ),
        (&file_url, &file_url, &file_url),
        (
            "org/repo",
            "git@github.com:org/repo.git",
            "https://github.com/org/repo",
        ),
        (
            "github.com/org/repo",
            "git@github.com:org/repo.git",
            "https://github.com/org/repo.git",
        ),
        (
            "https://git.example/projects/P/repos/R/browse",
            "ssh://git@git.example:7999/P/R.git",
            "git@git.example:P/R.git",
        ),
        (
            "http://git.example/projects/P/repos/R/browse/",
            "ssh://git@git.example:7999/P/R.git",
            "git@git.example:P/R",
        ),
        (
            "https://git.example/projects/P/repos/R/archive",
            "https://git.example/projects/P/repos/R/archive",
            "git@git.example:projects/P/repos/R/archive.git",
        ),
        (
            "https://git.example/projects/P/repos/R/browse/src",
            "https://git.example/projects/P/repos/R/browse/src",
            "git@git.example:projects/P/repos/R/browse/src.git",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let home = temp.path().join(index.to_string());
        fs::create_dir(&home).unwrap();
        let config = home.join("config.toml");
        let capture = home.join("argv");
        fs::write(&config, "root = 'repos'\n").unwrap();
        let run = |args: &[&str]| {
            let output = support::lager_with_path(&home, &config, Some(&tools))
                .env("CAPTURE", &capture)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{input}, {args:?}: {output:?}");
            output
        };
        run(&["register", input, "--post-clone", "printf HOOK"]);
        let saved = fs::read(&config).unwrap();
        assert!(String::from_utf8_lossy(&saved).contains(input));
        for command in ["list", "ls"] {
            let output = run(&[command, "--json"]);
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["repositories"][0]["clone_url"], clone_url);
        }
        run(&["register", equivalent, "--post-clone", "printf HOOK"]);
        assert_eq!(fs::read(&config).unwrap(), saved);
        run(&["add", input, "--no-register"]);
        assert_eq!(
            fs::read_to_string(&capture).unwrap(),
            format!("clone\n{clone_url}\n.\n")
        );
        run(&["ensure"]);
        assert_eq!(
            fs::read_to_string(&capture).unwrap(),
            format!("clone\n{clone_url}\n.\n")
        );
        assert!(String::from_utf8_lossy(&run(&["hook", equivalent]).stdout).contains("HOOK"));
        run(&[
            "remove",
            equivalent,
            "--yes",
            "--force",
            "--keep-registered",
        ]);
        assert_eq!(fs::read(&config).unwrap(), saved);
        assert!(String::from_utf8_lossy(&run(&["ensure"]).stdout).contains("HOOK"));
        assert_eq!(
            fs::read_to_string(&capture).unwrap(),
            format!("clone\n{clone_url}\n.\nclone\n{clone_url}\n.\n")
        );
        run(&["remove", equivalent, "--yes", "--force", "--unregister"]);
        let output = run(&["list", "--json"]);
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["repositories"].as_array().unwrap().len(), 0);
    }
}

#[cfg(unix)]
#[test]
fn loaded_credentials_and_malformed_toml_never_escape_diagnostics() {
    for reference in [
        "https:///SENTINEL@github.com/org/repo",
        "https:////SENTINEL@github.com/org/repo",
        "ftp://SENTINEL@example.invalid/org/repo",
        "https://SENTINEL@github.com/org/repo",
        "https://user:SENTINEL@github.com/org/*/bad",
        "ssh://user:SENTINEL@git.example/org/repo",
    ] {
        for malformed in [false, true] {
            for command in [
                "register",
                "unregister",
                "add",
                "ensure",
                "hook",
                "remove",
                "list",
                "ls",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let config = temp.path().join("config.toml");
                let original = format!(
                    "root = 'repos'\n[[repositories]]\nurl = 'org/first'\n[[repositories]]\nurl = '{reference}{}\n",
                    if malformed { "" } else { "'" }
                );
                fs::write(&config, &original).unwrap();
                let mut cli = guarded_lager(temp.path(), &config);
                cli.arg(command);
                if !matches!(command, "ensure" | "list" | "ls") {
                    cli.arg("org/first");
                }
                if command == "add" {
                    cli.arg("--register");
                }
                if command == "remove" {
                    cli.args(["--yes", "--unregister"]);
                }
                let output = cli.output().unwrap();
                assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
                assert!(output.stdout.is_empty(), "{command}: {output:?}");
                assert!(
                    !String::from_utf8_lossy(&output.stderr).contains("SENTINEL"),
                    "{command}: {output:?}"
                );
                assert_eq!(fs::read_to_string(config).unwrap(), original);
                assert!(!temp.path().join("repos").exists());
                assert!(
                    !temp.path().join("attempt").exists(),
                    "{command}: {output:?}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn rejected_references_are_redacted_before_batch_effects() {
    for reference in [
        "https:///SENTINEL@github.com/org/repo",
        "https:////SENTINEL@github.com/org/repo",
        "ftp://SENTINEL@example.invalid/org/repo",
        "https://SENTINEL@github.com/org/repo",
        "https://user:SENTINEL@github.com/org/repo",
        "ssh://user:SENTINEL@git.example/org/repo",
        "user:SENTINEL@git.example:org/repo",
        "file://SENTINEL@localhost/tmp/repo",
        "https://github.com/org/repo?SENTINEL",
        "https://github.com/org/repo#SENTINEL",
        "http://SENTINEL@github.com/org/repo",
        "ssh://SENTINEL:@git.example/org/repo",
        "file://user:SENTINEL@localhost/tmp/repo",
        "ssh://git@git.example/org/repo?SENTINEL",
        "file:///tmp/repo#SENTINEL",
        "git@git.example:org/repo#SENTINEL",
    ] {
        for command in ["register", "unregister", "add", "hook", "remove"] {
            let temp = tempfile::tempdir().unwrap();
            let config = temp.path().join("config.toml");
            let original = "root = 'repos'\n";
            fs::write(&config, original).unwrap();
            let mut cli = guarded_lager(temp.path(), &config);
            cli.args([command, "org/first", reference]);
            if command == "add" {
                cli.arg("--register");
            }
            if command == "remove" {
                cli.args(["--yes", "--unregister"]);
            }
            let output = cli.output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
            assert!(output.stdout.is_empty(), "{command}: {output:?}");
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("SENTINEL"),
                "{command}: {output:?}"
            );
            assert_eq!(fs::read_to_string(config).unwrap(), original);
            assert!(!temp.path().join("repos").exists());
            assert!(
                !temp.path().join("attempt").exists(),
                "{command}: {output:?}"
            );
        }
    }
}

#[test]
fn explicit_transport_survives_registration_and_listing() {
    for reference in [
        "https://github.com/org/repo",
        "http://bitbucket.org/org/repo",
        "https://github.com/org/repo.git",
        "ssh://alice@git.example:2222/org/repo",
        "alice@git.example:org/repo.git",
        "alice+ci@git.example:org/repo.git",
        "ssh://alice+ci@git.example/org/repo.git",
        "file:///tmp/repo.git",
        "https://git.example/projects/P/repos/R/archive",
        "https://git.example/projects/P/repos/R/browse/src",
        "https://git.example/projects/P/repos/R/browse//",
        "https://git.example/projects/P/repos/R/archive/../browse",
        "https://git.example/projects//repos/R/browse",
        "ssh://git@git.example/projects/P/repos/R/browse",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        fs::write(&config, "root = 'repos'\n").unwrap();
        let output = support::lager(temp.path(), &config)
            .args(["register", reference])
            .output()
            .unwrap();
        assert!(output.status.success(), "{reference}: {output:?}");
        assert!(fs::read_to_string(&config).unwrap().contains(reference));
        for command in ["list", "ls"] {
            let output = support::lager(temp.path(), &config)
                .args([command, "--json"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["repositories"][0]["clone_url"], reference);
        }
    }
}

#[cfg(unix)]
#[test]
fn invalid_provider_host_is_redacted_before_any_effect() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "root = 'repos'\n[providers.'SENTINEL@example.invalid']\npreset = 'github'\nunknown = true\n";
    fs::write(&config, original).unwrap();
    let output = guarded_lager(temp.path(), &config)
        .args(["register", "org/repo"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SENTINEL"));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_to_string(config).unwrap(), original);
    assert!(!temp.path().join("attempt").exists());
}

#[test]
fn malformed_scp_hosts_and_unsupported_schemes_are_not_registered() {
    for reference in [
        "alice@git example:org/repo",
        "alice@git/example:org/repo",
        "alice@git\\example:org/repo",
        "alice@:org/repo",
        "alice@git.example:",
        "ftp://example.invalid/org/repo",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        fs::write(&config, "root = 'repos'\n").unwrap();
        let output = support::lager(temp.path(), &config)
            .args(["register", reference])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{reference}: {output:?}");
        assert_eq!(fs::read_to_string(config).unwrap(), "root = 'repos'\n");
    }
}

#[test]
fn ordinary_malformed_targets_do_not_block_independent_operations() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let output = support::external(temp.path(), "git")
        .current_dir(&source)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let reference = format!("file://{}", source.display());
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = 'repos'\n").unwrap();
    let register = support::lager(temp.path(), &config)
        .args(["register", &reference, "--post-clone", "printf INDEPENDENT"])
        .output()
        .unwrap();
    assert!(register.status.success(), "{register:?}");
    for command in ["add", "hook", "remove"] {
        let mut cli = support::lager(temp.path(), &config);
        cli.args([command, "malformed", &reference]);
        if command == "add" {
            cli.arg("--no-register");
        }
        if command == "remove" {
            cli.args(["--yes", "--force", "--keep-registered"]);
        }
        let output = cli.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        if command == "remove" {
            assert!(!temp.path().join("repos/source").exists(), "{output:?}");
        } else {
            assert!(
                temp.path().join("repos/source/.git").exists(),
                "{command}: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("INDEPENDENT"),
                "{command}: {output:?}"
            );
        }
    }
}

#[test]
fn malformed_hook_and_remove_references_are_redacted_without_blocking_later_targets() {
    for (rejected, allow_later) in [
        ("ssh://SENTINEL@example.invalid", true),
        ("SENTINEL@host:", true),
        // Ambiguous userinfo remains a sensitive whole-batch preflight failure.
        ("SENTINEL@@host:org/repo", false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let checkout = temp.path().join("repos/source");
        fs::create_dir_all(&checkout).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["remote", "add", "origin", "file:///source"],
        ] {
            let output = support::external(&checkout, "git")
                .current_dir(&checkout)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        fs::write(
            &config,
            "root = 'repos'\n[[repositories]]\nurl = 'file:///source'\npost_clone = 'printf SAFE_LATER'\n",
        )
        .unwrap();
        for operation in ["hook", "remove"] {
            let mut cli = support::lager(temp.path(), &config);
            cli.args([operation, rejected, "file:///source"]);
            if operation == "remove" {
                cli.args(["--yes", "--force", "--keep-registered"]);
            }
            let output = cli.output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("SENTINEL"),
                "{output:?}"
            );
            if operation == "hook" {
                let expected: &[u8] = if allow_later { b"SAFE_LATER" } else { b"" };
                assert_eq!(output.stdout, expected, "{rejected}: {output:?}");
            } else {
                assert_eq!(checkout.exists(), !allow_later, "{rejected}: {output:?}");
            }
        }
    }
}
