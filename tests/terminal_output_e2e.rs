mod support;

use std::fs;

#[cfg(unix)]
fn script(path: &std::path::Path, source: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn provider_and_config_diagnostics_are_one_safe_line_but_json_errors_are_raw() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let error = format!("provider{}end", controls().replace('\0', ""));
    script(
        &bin.join("gh"),
        "#!/bin/sh\nprintf '%s' \"$ERROR\" >&2\nexit 1\n",
    );
    fs::write(&config, "root = 'repos'\n[providers.'github.com']\npreset = 'github'\n[[repositories]]\nurl = 'org/*'\n").unwrap();
    for command in ["list", "ensure", "register"] {
        let mut process = support::lager_with_path(temp.path(), &config, Some(&bin));
        process.arg(command).env("ERROR", &error);
        if command == "list" {
            process.args(["--remote", "--json"]);
        }
        let output = process.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_safe_fields(&output.stderr);
        assert!(String::from_utf8_lossy(&output.stderr).contains("\\x1b"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("\nend"));
        if command == "list" {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["provider_errors"][0]["error"], error);
        }
    }
    let missing = temp.path().join("missing\n\u{1b}\u{85}.toml");
    let output = support::lager(temp.path(), &missing)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_safe_fields(&output.stderr);
    assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
}

fn controls() -> String {
    (0u8..=159)
        .filter(|byte| *byte < 32 || *byte >= 127)
        .map(char::from)
        .collect()
}

#[cfg(unix)]
#[test]
fn picker_escapes_labels_and_maps_duplicate_labels_to_untouched_candidates() {
    use lager::application::ports::{RepositorySelector, SelectionCandidate};
    use lager::domain::repository::RepositoryRef;
    use lager::infrastructure::fzf::Fzf;

    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("fzf");
    let captured = temp.path().join("input");
    script(
        &executable,
        &format!(
            "#!/bin/sh\ncat > '{}'\ncat '{}'\n",
            captured.display(),
            captured.display()
        ),
    );
    let candidates: Vec<_> = ["org/one", "org/two"]
        .into_iter()
        .map(|reference| SelectionCandidate {
            reference: RepositoryRef::parse(reference).unwrap(),
            display: format!("label{}end", controls()),
            archived: false,
            exact_path: None,
        })
        .collect();
    assert_eq!(
        Fzf::new(executable).select(&candidates).unwrap(),
        candidates
    );
    let labels = fs::read(&captured).unwrap();
    assert_safe_fields(&labels);
    let labels = String::from_utf8(labels).unwrap();
    assert_eq!(labels.lines().count(), 2);
    for (index, row) in labels.lines().enumerate() {
        let (id, label) = row.split_once('\t').unwrap();
        assert_eq!(id, index.to_string());
        assert!(!label.contains('\t'));
    }
    assert!(labels.contains("\\n"));
}

#[cfg(unix)]
#[test]
fn literal_escape_notation_cannot_collide_with_an_exact_path_control() {
    use lager::application::ports::{RepositorySelector, SelectionCandidate};
    use lager::domain::repository::RepositoryRef;
    use lager::infrastructure::fzf::Fzf;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("fzf");
    let captured = temp.path().join("labels");
    script(
        &executable,
        &format!(
            "#!/bin/sh\ncat > '{}'\nsed -n '2p' '{}'\n",
            captured.display(),
            captured.display()
        ),
    );
    let candidates: Vec<_> = ["path\u{1b}end", "path\\x1bend"]
        .into_iter()
        .map(|path| SelectionCandidate {
            reference: RepositoryRef::parse("org/repo").unwrap(),
            display: "untrusted\nlabel".into(),
            archived: false,
            exact_path: Some(temp.path().join(path)),
        })
        .collect();
    assert_eq!(
        Fzf::new(executable).select(&candidates).unwrap(),
        [candidates[1].clone()]
    );
    let labels = fs::read_to_string(captured).unwrap();
    let lines: Vec<_> = labels.lines().collect();
    assert_ne!(lines[0], lines[1]);
    assert!(lines[1].contains("path\\\\x1bend"));
}

#[cfg(unix)]
#[test]
fn removal_picker_uses_safe_fields_and_deletes_only_the_exact_selected_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let root = temp.path().join("repos");
    let checkout = root.join("custom\n\t\r\u{1b}\u{7f}\u{85}end");
    let unselected = root.join("untouched");
    for (path, origin) in [
        (&checkout, "file:///primary"),
        (&unselected, "file:///other"),
    ] {
        fs::create_dir_all(path).unwrap();
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", origin]] {
            let output = support::external(temp.path(), "git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
    }
    fs::write(
        &config,
        "root = 'repos'\n[[repositories]]\nurl = 'file:///primary'\n",
    )
    .unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let captured = temp.path().join("labels");
    script(
        &bin.join("fzf"),
        "#!/bin/sh\ncat > \"$CAPTURE\"\nsed -n '2p' \"$CAPTURE\"\n",
    );
    let output = support::lager_with_path(temp.path(), &config, Some(&bin))
        .args(["remove", "--yes", "--force", "--keep-registered"])
        .env("CAPTURE", &captured)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!checkout.exists(), "{output:?}");
    assert!(unselected.join(".git").exists(), "{output:?}");
    let labels = fs::read(captured).unwrap();
    assert_safe_fields(&labels);
    let labels = String::from_utf8(labels).unwrap();
    assert_eq!(labels.lines().count(), 2);
    assert_eq!(labels.matches('\t').count(), 4);
    assert!(
        labels
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("1\tfile:///primary\t")
    );
    assert!(labels.contains("custom\\n\\t\\r\\x1b\\x7f\\x85"));
    assert!(
        fs::read_to_string(config)
            .unwrap()
            .contains("file:///primary")
    );
}

fn assert_safe_fields(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).unwrap();
    assert!(
        !text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "{text:?}"
    );
}

#[cfg(unix)]
fn prompt_session(mut command: std::process::Command, ready: &str, answer: &[u8]) -> String {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let pty = rustix_openpty::openpty(None, None).unwrap();
    let mut writer = fs::File::from(pty.controller);
    let mut reader = writer.try_clone().unwrap();
    let terminal = fs::File::from(pty.user);
    command
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
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut transcript = String::new();
    let mut answered = false;
    loop {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
            transcript.push_str(&String::from_utf8_lossy(&bytes));
        }
        if !answered && transcript.contains(ready) {
            writer.write_all(answer).unwrap();
            writer.flush().unwrap();
            answered = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            while let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
                transcript.push_str(&String::from_utf8_lossy(&bytes));
            }
            assert!(status.success(), "{transcript:?}");
            assert!(answered, "{transcript:?}");
            return transcript;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("prompt timeout: {transcript:?}");
        }
    }
}

#[cfg(unix)]
#[test]
fn prompt_display_is_safe_and_empty_input_preserves_raw_default() {
    use lager::application::ports::Interaction;
    use lager::cli::interaction::TerminalInteraction;
    let raw = "value\n\r\t\u{1b}]2;INJECT\u{7}\u{85}end";
    if let Some(result) = std::env::var_os("LAGER_PROMPT_TEST_RESULT") {
        let answer = TerminalInteraction.input(raw, raw, Some(raw)).unwrap();
        fs::write(result, answer).unwrap();
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let result = temp.path().join("answer");
    let mut command = support::external(temp.path(), std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "prompt_display_is_safe_and_empty_input_preserves_raw_default",
            "--nocapture",
        ])
        .env("LAGER_PROMPT_TEST_RESULT", &result);
    let transcript = prompt_session(command, "end", b"\r");
    assert_eq!(fs::read_to_string(result).unwrap(), raw);
    assert!(!transcript.contains("\u{1b}]2;INJECT"), "{transcript:?}");
    assert!(!transcript.contains('\u{85}'), "{transcript:?}");
    assert!(
        transcript.contains("value\\n\\r\\t\\x1b]2;INJECT\\x07\\x85end"),
        "{transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn confirmation_display_is_safe_without_changing_default_answer() {
    use lager::application::ports::Interaction;
    use lager::cli::interaction::TerminalInteraction;
    let raw = "confirm\n\r\t\u{1b}]2;INJECT\u{7}\u{85}end";
    if std::env::var_os("LAGER_CONFIRM_TEST_CHILD").is_some() {
        assert!(!TerminalInteraction.confirm(raw, false).unwrap());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut command = support::external(temp.path(), std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "confirmation_display_is_safe_without_changing_default_answer",
            "--nocapture",
        ])
        .env("LAGER_CONFIRM_TEST_CHILD", "1");
    let transcript = prompt_session(command, "end", b"\r");
    assert!(!transcript.contains("\u{1b}]2;INJECT"), "{transcript:?}");
    assert!(!transcript.contains('\u{85}'), "{transcript:?}");
    assert!(
        transcript.contains("confirm\\n\\r\\t\\x1b]2;INJECT\\x07\\x85end"),
        "{transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn native_git_and_hook_streams_remain_raw_while_ensure_reporter_escapes_fields() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    script(
        &bin.join("git"),
        "#!/bin/sh\nprintf 'native\\033[31m\\tgit\\n' >&2\nexit 1\n",
    );
    let mut document = toml_edit::DocumentMut::new();
    document["root"] = toml_edit::value("repos\n\u{85}end");
    let mut repository = toml_edit::Table::new();
    repository["url"] = toml_edit::value("org/repo");
    let mut repositories = toml_edit::ArrayOfTables::new();
    repositories.push(repository);
    document["repositories"] = toml_edit::Item::ArrayOfTables(repositories);
    fs::write(&config, document.to_string()).unwrap();
    let output = support::lager_with_path(temp.path(), &config, Some(&bin))
        .arg("ensure")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("native\u{1b}[31m\tgit\n"), "{stderr:?}");
    assert!(stderr.contains("repos\\n\\x85end"), "{stderr:?}");
    assert!(!stderr.contains("repos\n"), "{stderr:?}");
    let checkout = temp.path().join("repos/primary");
    fs::create_dir_all(&checkout).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["remote", "add", "origin", "file:///primary"],
    ] {
        assert!(
            support::external(temp.path(), "git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fs::write(&config, "root = 'repos'\n[[repositories]]\nurl = 'file:///primary'\npost_clone = '''printf 'hook\\033[32m\\tstdout\\n'; printf 'hook\\033[33m\\tstderr\\n' >&2'''\n").unwrap();
    let output = support::lager(temp.path(), &config)
        .args(["hook", "file:///primary"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"hook\x1b[32m\tstdout\n");
    assert_eq!(output.stderr, b"hook\x1b[33m\tstderr\n");
}

#[test]
fn human_rows_and_unknown_keys_escape_controls_while_json_preserves_values() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    // Portable roots permit control characters; repository reference validation
    // stays intact. Quoted TOML keys reach the warning adapter directly.
    let root = format!("repos{}", controls());
    let key = format!("future{}", controls());
    let mut document = toml_edit::DocumentMut::new();
    document["root"] = toml_edit::value(&root);
    document[&key] = toml_edit::value(true);
    document["providers"]["github.com"]["preset"] = toml_edit::value("github");
    document["providers"]["github.com"][&key] = toml_edit::value(true);
    let mut repository = toml_edit::Table::new();
    repository["url"] = toml_edit::value("org/repo");
    repository[&key] = toml_edit::value(true);
    let mut repositories = toml_edit::ArrayOfTables::new();
    repositories.push(repository);
    document["repositories"] = toml_edit::Item::ArrayOfTables(repositories);
    fs::write(&config, document.to_string()).unwrap();

    let human = support::lager(temp.path(), &config)
        .arg("list")
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    assert_safe_fields(&human.stdout);
    assert_safe_fields(&human.stderr);
    assert_eq!(String::from_utf8_lossy(&human.stdout).lines().count(), 1);
    assert_eq!(
        String::from_utf8_lossy(&human.stdout).matches('\t').count(),
        2
    );
    assert_eq!(String::from_utf8_lossy(&human.stderr).lines().count(), 3);
    assert!(String::from_utf8_lossy(&human.stdout).contains("\\x1b"));
    let json = support::lager(temp.path(), &config)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(json.status.success(), "{json:?}");
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        value["repositories"][0]["destination"].as_str().unwrap(),
        temp.path().join(root).join("org/repo").to_str().unwrap()
    );
}
