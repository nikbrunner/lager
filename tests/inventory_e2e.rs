mod support;

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::time::{Duration, Instant};
use std::{fs, process::Command};

#[cfg(unix)]
#[test]
fn inventory_inspect_shows_current_declaration_and_action_reason_after_resize_and_scroll() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let padding = (0..3)
        .map(|index| format!("\"padding-{index}-{}\"", "a ".repeat(70)))
        .chain(std::iter::once("\"escape-\\n\\t\\u001b\"".to_owned()))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [{padding}]\n\n[inventory.keys.inspection]\npage_down = [\"x\"]\n"
        ),
    )
    .unwrap();

    let selected = "selected: github.com/org/*";
    let mut scrolled_frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["selected: github.com/org/alpha"], b"j\r".to_vec()),
            (
                vec![
                    "INSPECT / github.com/org/*",
                    "Declaration: github.com/org/*",
                    "Mark: unavailable",
                    "declaration patterns cannot be marked",
                ],
                vec![b'x'; 32],
            ),
            (
                vec!["INSPECT", "excludes escape-\\n\\t\\x1b"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", selected], b"q".to_vec()),
        ],
        24,
        80,
        0,
        |action, writer, frame| match action {
            1 => resize_pty(writer, 16, 80),
            2 => scrolled_frames.push(frame.to_owned()),
            _ => {}
        },
    );

    let [scrolled] = scrolled_frames.as_slice() else {
        panic!("expected one resized, scrolled inspection frame: {scrolled_frames:?}");
    };
    let screen = screen_text(scrolled, 16, 80);
    for text in ["INSPECT", "excludes escape-\\n\\t\\x1b"] {
        assert!(screen.contains(text), "missing {text}:\n{screen}");
    }
}

#[cfg(unix)]
#[test]
fn inventory_root_menu_uses_effective_hints_and_preserves_search_and_selection() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/beta\"\n\n[inventory.keys.normal]\nmenu = [\"M\"]\nsearch = [\"s\"]\ninspect = [\"i\"]\n",
    )
    .unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["local scan complete", "selected: github.com/org/alpha"],
                b"jsbeta\rM".to_vec(),
            ),
            (
                vec![
                    "MENU / actions",
                    "i inspect",
                    "s search",
                    "? help",
                    "q quit",
                    "NORMAL / beta",
                ],
                b"i".to_vec(),
            ),
            (vec!["INSPECT / github.com/org/beta"], b"\x1b".to_vec()),
            (
                vec!["NORMAL / beta", "selected: github.com/org/beta"],
                b"M".to_vec(),
            ),
            (vec!["MENU / actions", "s search"], b"s".to_vec()),
            (
                vec!["SEARCH / beta", "github.com/org/beta"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL / beta", "selected: github.com/org/beta"],
                b"M".to_vec(),
            ),
            (vec!["MENU / actions"], b"\x1b".to_vec()),
            (
                vec!["NORMAL / beta", "selected: github.com/org/beta"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_search_matches_full_origins_and_keeps_text_and_filters_safe() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"alpha\"]\n",
    )
    .unwrap();
    init_repo(
        &root.join("hidden/zephyr-target"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(&root.join("other"), "git@github.com:org/beta.git", true);
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["github.com/org/alpha", "github.com/org/beta"],
                b"/gto/rgap".to_vec(),
            ),
            (
                vec!["SEARCH / gto/rgap", "github.com/org/alpha", "excluded"],
                b"raRq".to_vec(),
            ),
            (
                vec!["SEARCH / gto/rgapraRq", "No matching repositories"],
                b"\x7f\x7f\x7f\x7f".to_vec(),
            ),
            (
                vec!["SEARCH / gto/rgap", "github.com/org/alpha"],
                b"\r".to_vec(),
            ),
            (
                vec!["NORMAL / gto/rgap", "github.com/org/alpha"],
                b"/".to_vec(),
            ),
            (
                vec!["SEARCH / gto/rgap", "github.com/org/alpha"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL / gto/rgap"], b"\x7f".to_vec()),
            (
                vec!["NORMAL /  · 3 rows", "github.com/org/beta"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_search_matches_checkout_state_without_matching_contrasting_rows() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/aid/alpha\"\n\n[[repositories]]\nurl = \"github.com/aid/beta\"\n",
    )
    .unwrap();
    init_repo(&root.join("aid/beta"), "git@github.com:aid/beta.git", true);

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["local scan complete"], b"/missing\r".to_vec()),
            (
                vec![
                    "NORMAL / missing · 1 rows",
                    "github.com/aid/alpha",
                    "missing",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        220,
    );
    let screen = screen_text(&transcript, 24, 220);
    assert!(screen.contains("github.com/aid/alpha"), "{screen}");
    assert!(
        !screen.contains("github.com/aid/beta"),
        "checkout-state query retained contrasting row:\n{screen}"
    );
}

#[test]
fn inventory_aliases_reject_invalid_non_tty_entry_without_provider_requests() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();

    for command_name in ["inventory", "inv"] {
        support::clear_outbound_requests();
        let archived =
            output(support::lager(&home, &config).args([command_name, "--include-archived"]));
        assert_eq!(
            archived.status.code(),
            Some(2),
            "{command_name}: {archived:?}"
        );
        assert!(String::from_utf8_lossy(&archived.stderr).contains("--remote"));
        assert!(archived.stdout.is_empty());
        assert!(
            support::outbound_requests().is_empty(),
            "{command_name} made provider requests"
        );

        support::clear_outbound_requests();
        let non_tty = output(support::lager(&home, &config).arg(command_name));
        assert_eq!(
            non_tty.status.code(),
            Some(2),
            "{command_name}: {non_tty:?}"
        );
        assert!(String::from_utf8_lossy(&non_tty.stderr).contains("requires a TTY"));
        assert!(non_tty.stdout.is_empty());
        assert!(
            support::outbound_requests().is_empty(),
            "{command_name} made provider requests"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_an_origin_query_failure_when_a_later_probe_is_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("later-probe-pids");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("checkout"),
        "git@github.com:org/checkout.git",
        true,
    );
    let tools = temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), format!(
        "#!/bin/sh\nif [ \"$4\" = get-url ]; then echo 'origin query failed' >&2; exit 128; fi\n/bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait\n",
        shell_word(&marker),
    )).unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let processes = FixtureProcesses(marker);
    let mut cancelled_at = None;
    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inv"),
        &[(vec!["checkout", "pending"], b"q".to_vec())],
        24,
        200,
        1,
        |_| {
            processes.assert_running();
            cancelled_at = Some(Instant::now());
        },
    );
    processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn inventory_keeps_real_failures_when_cancelling_a_changed_target_or_config() {
    for change_config in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("repos");
        let config = temp.path().join("config.toml");
        let checkout = root.join("checkout");
        let marker = temp.path().join("status-pids");
        fs::create_dir_all(&root).unwrap();
        fs::write(&config, "root = \"repos\"\n").unwrap();
        init_repo(&checkout, "git@github.com:org/checkout.git", true);
        let tools = temp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("git"), format!(
            "#!/bin/sh\nif [ \"$3\" = symbolic-ref ]; then echo cancelled >&2; exit 1; fi\nif [ \"$3\" = status ]; then /bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait; fi\nexec {} \"$@\"\n",
            shell_word(&marker), shell_word(&support::real_tool("git")),
        )).unwrap();
        fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let processes = FixtureProcesses(marker);
        let mut cancelled_at = None;
        run_inventory_pty_with_exit(
            support::lager_with_path(&home, &config, Some(&tools)).arg("inv"),
            &[(vec!["checkout", "pending"], b"q".to_vec())],
            24,
            200,
            1,
            |_| {
                processes.assert_running();
                if change_config {
                    fs::write(&config, "root = \"repos\"\n# edited\n").unwrap();
                } else {
                    run_git(
                        &checkout,
                        &[
                            "remote",
                            "set-url",
                            "origin",
                            "git@github.com:org/changed.git",
                        ],
                    );
                }
                cancelled_at = Some(Instant::now());
            },
        );
        processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
    }
}

#[cfg(unix)]
#[test]
fn inventory_pending_checkout_does_not_assert_a_conflicting_origin() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/checkout\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/checkout"),
        "git@github.com:org/checkout.git",
        true,
    );
    let tools = temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), "#!/bin/sh\n/bin/sleep 30\n").unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let transcript = run_inventory_pty_with_size(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inv"),
        &["2 rows", "pending"],
        b"q",
        24,
        300,
    );
    let rows = inventory_rows(&screen_text(&transcript, 24, 300));
    let declared = rows
        .iter()
        .find(|row| row[0] == "github.com/org/checkout")
        .unwrap();
    assert_eq!(declared[3], "unknown", "{rows:?}");
}

#[cfg(unix)]
#[test]
fn inventory_refresh_reconciles_deleted_checkouts_without_turning_incomplete_into_missing() {
    for incomplete in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("repos");
        let config = temp.path().join("config.toml");
        let checkout = root.join("org/checkout");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &config,
            "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/checkout\"\n",
        )
        .unwrap();
        init_repo(&checkout, "git@github.com:org/checkout.git", true);
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("inv"),
            &[
                (
                    vec!["main", "known clean", "local scan complete"],
                    b"R".to_vec(),
                ),
                (
                    vec![
                        "generation 2",
                        if incomplete {
                            "partial scan:"
                        } else {
                            "local scan complete"
                        },
                    ],
                    b"q".to_vec(),
                ),
            ],
            24,
            300,
            i32::from(incomplete),
            |action| {
                if action == 0 {
                    if incomplete {
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
                    } else {
                        fs::remove_dir_all(&checkout).unwrap();
                    }
                }
            },
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let rows = inventory_rows(&screen_text(&transcript, 24, 300));
        let row = rows
            .iter()
            .find(|row| row[0] == "github.com/org/checkout")
            .unwrap();
        assert_eq!(
            row[3],
            if incomplete { "unreadable" } else { "missing" },
            "{rows:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_bare_probe_failure_is_partial_even_when_stderr_says_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    run_git(
        temp.path(),
        &["init", "--bare", root.join("bare.git").to_str().unwrap()],
    );
    let tools = temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), "#!/bin/sh\necho cancelled >&2\nexit 1\n").unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inv"),
        &[(
            vec![
                "partial scan:",
                "could not inspect bare repository",
                "cancelled",
            ],
            b"q".to_vec(),
        )],
        24,
        300,
        1,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_detached_head_is_a_successful_observation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let checkout = root.join("detached");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&checkout, "git@github.com:org/detached.git", true);
    run_git(&checkout, &["checkout", "--detach", "-q"]);
    run_inventory_pty_with_size(
        support::lager(&home, &config).arg("inv"),
        &["detached HEAD", "known clean", "local scan complete"],
        b"q",
        24,
        220,
    );
}

#[cfg(unix)]
#[test]
fn inventory_cancels_an_active_originless_fallback_without_failure() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("fallback-pids");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo_without_origin(&root.join("originless"));
    let tools = temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(&wrapper, format!(
        "#!/bin/sh\nif [ \"$3\" = remote ] && [ $# = 3 ]; then /bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait; fi\nexec {} \"$@\"\n",
        shell_word(&marker), shell_word(&support::real_tool("git")),
    )).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let processes = FixtureProcesses(marker);
    let mut cancelled_at = None;
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[(vec!["originless", "pending"], b"q".to_vec())],
        24,
        180,
        |_| {
            processes.assert_running();
            cancelled_at = Some(Instant::now());
        },
    );
    processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn inventory_validates_both_terminal_streams_and_startup_before_raw_mode() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(&config, "invalid = [").unwrap();
    for name in ["inventory", "inv"] {
        for tty_stdin in [false, true] {
            let pty = rustix_openpty::openpty(None, None).unwrap();
            let before = rustix_openpty::rustix::termios::tcgetattr(&pty.user).unwrap();
            let mut command = support::lager(&home, &config);
            command
                .arg(name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            if tty_stdin {
                command.stdin(Stdio::from(pty.user.try_clone().unwrap()));
            } else {
                command.stdout(Stdio::from(pty.user.try_clone().unwrap()));
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(String::from_utf8_lossy(&output.stderr).contains("requires a TTY"));
            let after = rustix_openpty::rustix::termios::tcgetattr(&pty.user).unwrap();
            assert_eq!(termios_fingerprint(&before), termios_fingerprint(&after));
        }
        for flags in [vec!["--include-archived"], vec!["--invalid"]] {
            let transcript = run_inventory_pty_with_exit(
                support::lager(&home, &config).arg(name).args(flags),
                &[],
                24,
                132,
                2,
                |_| {},
            );
            assert!(!transcript.contains("\u{1b}[?1049h"));
        }
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg(name),
            &[],
            24,
            132,
            1,
            |_| {},
        );
        assert!(!transcript.contains("\u{1b}[?1049h"));
        assert!(transcript.contains("lager:"));
    }
}

#[cfg(unix)]
#[test]
fn inventory_observation_failures_set_exit_status_even_after_successful_refresh() {
    for operation in ["remote", "symbolic-ref", "status"] {
        for refresh in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let root = home.join("repos");
            let config = temp.path().join("config.toml");
            fs::create_dir_all(&root).unwrap();
            fs::write(&config, "root = \"repos\"\n").unwrap();
            init_repo(
                &root.join("checkout"),
                "git@github.com:org/checkout.git",
                true,
            );
            let tools = temp.path().join("tools");
            fs::create_dir_all(&tools).unwrap();
            let failing = temp.path().join("fail");
            fs::write(&failing, "fail").unwrap();
            let wrapper = tools.join("git");
            fs::write(&wrapper, format!(
                "#!/bin/sh\nif [ \"$3\" = '{operation}' ] && [ -e {} ]; then echo 'injected probe failure' >&2; exit 1; fi\nexec {} \"$@\"\n",
                shell_word(&failing), shell_word(&support::real_tool("git")),
            )).unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            let actions = if refresh {
                vec![
                    (vec!["injected probe failure"], b"R".to_vec()),
                    (
                        vec!["generation 2", "local scan complete", "known clean", "main"],
                        b"q".to_vec(),
                    ),
                ]
            } else {
                vec![(vec!["injected probe failure"], b"q".to_vec())]
            };
            let transcript = run_inventory_pty_with_exit(
                support::lager_with_path(&home, &config, Some(&tools)).arg("inv"),
                &actions,
                24,
                300,
                1,
                |action| {
                    if refresh && action == 0 {
                        fs::remove_file(&failing).unwrap();
                    }
                },
            );
            if refresh {
                assert!(!screen_text(&transcript, 24, 300).contains("injected probe failure"));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_discovery_failure_sets_exit_status_even_after_successful_refresh() {
    for refresh in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(&home).unwrap();
        fs::write(&config, "root = \"repos\"\n").unwrap();
        let actions = if refresh {
            vec![
                (vec!["partial scan:", "scan is incomplete"], b"R".to_vec()),
                (vec!["generation 2", "local scan complete"], b"q".to_vec()),
            ]
        } else {
            vec![(vec!["partial scan:", "scan is incomplete"], b"q".to_vec())]
        };
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("inventory"),
            &actions,
            24,
            180,
            1,
            |action| {
                if refresh && action == 0 {
                    fs::create_dir_all(home.join("repos")).unwrap();
                }
            },
        );
        if refresh {
            assert!(!screen_text(&transcript, 24, 180).contains("partial scan:"));
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_advertises_only_offline_capabilities_and_rejects_remote_before_raw_mode() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    for name in ["inventory", "inv"] {
        let help = output(support::lager(&home, &config).args([name, "--help"]));
        assert!(String::from_utf8_lossy(&help.stdout).contains("Remote discovery is unavailable"));
        support::clear_outbound_requests();
        let transcript = run_inventory_pty_with_size(
            support::lager(&home, &config).arg(name),
            &["local scan complete", "No repositories found"],
            b"q",
            24,
            132,
        );
        let plain = screen_text(&transcript, 24, 132);
        assert!(plain.contains("remote unavailable"), "{plain}");
        assert!(
            plain.contains("q/Esc quit") && plain.contains("R refresh"),
            "{plain}"
        );
        assert!(plain.contains("/ search"), "{plain}");
        for unavailable in ["all actions", "Add to", "Navigation is ready"] {
            assert!(!plain.contains(unavailable), "{plain}");
        }
        assert!(support::outbound_requests().is_empty());
        for flags in [vec!["--remote"], vec!["--remote", "--include-archived"]] {
            let transcript = run_inventory_pty_with_exit(
                support::lager(&home, &config).arg(name).args(flags),
                &[],
                24,
                132,
                2,
                |_| {},
            );
            assert!(
                transcript.contains("remote discovery is unavailable"),
                "{transcript}"
            );
            assert!(!transcript.contains("\u{1b}[?1049h"));
            assert!(support::outbound_requests().is_empty());
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_explicit_registration_wildcard_unions_destination_uncertainty_and_origin_facts()
{
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    fs::create_dir_all(&root).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        r#"root = "repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/group/managed"

[[repositories]]
url = "github.com/group/missing"

[[repositories]]
url = "github.com/group/*"

[[repositories]]
url = "github.com/group/sub/*"
exclude = ["repo"]
"#,
    )
    .unwrap();

    // This directory is not a checkout, so treating it as cloneable/missing would be unsafe.
    fs::create_dir_all(root.join("group/missing")).unwrap();
    init_repo(
        &root.join("elsewhere/managed"),
        "git@github.com:group/managed.git",
        true,
    );
    init_repo(
        &root.join("group/sub/repo"),
        "git@github.com:group/sub/repo.git",
        true,
    );
    init_repo(&root.join("local/relative"), "../relative-origin", true);

    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config).arg("inventory"),
        &["7 rows", "local scan complete"],
        b"q",
        80,
        220,
    );
    let plain = screen_text(&transcript, 80, 220);

    for expected in [
        "github.com/group/managed",
        "explicit",
        "elsewhere/managed",
        "misplaced checkout; found:",
        "github.com/group/missing",
        "unknown",
        "github.com/group/sub/repo",
        "wildcard",
        "../relative-origin",
    ] {
        assert!(plain.contains(expected), "missing {expected}:\n{plain}");
    }
    assert!(
        !plain.contains("github.com/group/missing | missing"),
        "a present non-checkout destination was marked missing:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_unions_nested_wildcards_with_exclusions_in_each_declaration_order() {
    for declarations in [
        [
            "[[repositories]]\nurl = \"github.com/group/*\"\n",
            "[[repositories]]\nurl = \"github.com/group/sub/*\"\nexclude = [\"repo\"]\n",
        ],
        [
            "[[repositories]]\nurl = \"github.com/group/sub/*\"\nexclude = [\"repo\"]\n",
            "[[repositories]]\nurl = \"github.com/group/*\"\n",
        ],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("repos");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &config,
            format!(
                "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n{}\n{}",
                declarations[0], declarations[1]
            ),
        )
        .unwrap();
        init_repo(
            &root.join("group/sub/repo"),
            "git@github.com:group/sub/repo.git",
            true,
        );

        let transcript = run_inventory_pty_with_size(
            support::lager(&home, &config).arg("inventory"),
            &[
                "github.com/group/sub/repo",
                "covered by github.com/group/*",
                "excluded by github.com/group/sub/*",
            ],
            b"q",
            24,
            220,
        );
        let rows = inventory_rows(&screen_text(&transcript, 24, 220));
        let row = rows
            .iter()
            .find(|row| {
                row[0] == "github.com/group/sub/repo" && row[1].ends_with("/group/sub/repo")
            })
            .expect("nested wildcard checkout row");
        assert_eq!(row[2], "wildcard");
        assert_eq!(row[3], "cloned");
        assert_eq!(row[5], "main");
        assert_eq!(row[6], "known clean");
    }
}

#[cfg(unix)]
#[test]
fn inventory_shows_a_discovered_checkout_pending_before_a_delayed_probe_completes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("arrives-early"),
        "git@github.com:org/arrives.git",
        true,
    );
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n/bin/sleep 2\nexec {} \"$@\"\n",
            shell_word(support::real_tool("git").as_path())
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let transcript = run_inventory_pty_with_size(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &["arrives-early", "pending"],
        b"q",
        80,
        220,
    );
    assert!(screen_text(&transcript, 80, 220).contains("pending"));
}

#[cfg(unix)]
#[test]
fn inventory_refresh_cancels_delayed_probes_and_quit_remains_responsive() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/delayed\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/delayed"),
        "git@github.com:org/delayed.git",
        true,
    );

    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let real_git = support::real_tool("git");
    let wrapper = tools.join("git");
    let marker = temp.path().join("refresh-pids");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ ! -e {} ]; then /bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait; fi\nexec {} \"$@\"\n",
            shell_word(&marker), shell_word(&marker), shell_word(&real_git)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let processes = FixtureProcesses(marker);
    let mut cancelled_at = None;
    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["pending"], b"R".to_vec()),
            (vec!["generation 2", "github.com/org/fresh"], b"q".to_vec()),
        ],
        32,
        132,
        |action_index| {
            if action_index == 0 {
                processes.assert_running();
                fs::create_dir_all(home.join("fresh-repos")).unwrap();
                fs::write(
                    &config,
                    "root = \"fresh-repos\"\n\n[[repositories]]\nurl = \"github.com/org/fresh\"\n",
                )
                .unwrap();
                cancelled_at = Some(Instant::now());
            } else {
                processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
            }
        },
    );
    let plain = screen_text(&transcript, 32, 132);
    assert!(
        plain.contains("generation 2"),
        "refresh did not start a new generation:\n{plain}"
    );
    assert!(
        plain.contains("github.com/org/fresh") && !plain.contains("github.com/org/delayed"),
        "stale result from the prior config generation was accepted:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_refresh_keeps_completed_observations_stale_until_reprobed() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let slow_marker = temp.path().join("slow-refresh");
    let target = root.join("org/stale");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/stale\"\n",
    )
    .unwrap();
    init_repo(&target, "git@github.com:org/stale.git", true);
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    let real_git = support::real_tool("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e {} ]; then /bin/sleep 2; fi\nexec {} \"$@\"\n",
            shell_word(&slow_marker),
            shell_word(&real_git)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["github.com/org/stale", "main", "known clean"],
                b"R".to_vec(),
            ),
            (
                vec!["generation 2", "stale: main", "stale: known clean"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 0 {
                fs::write(&slow_marker, "slow").unwrap();
            }
        },
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(plain.contains("stale: main") && plain.contains("stale: known clean"));
}

#[cfg(unix)]
#[test]
fn inventory_quit_reaps_a_probe_descendant_that_is_actually_running() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("probe-started");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("active-probe"),
        "git@github.com:org/active-probe.git",
        true,
    );
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n/bin/sleep 30 &\nprintf '%s %s' \"$$\" \"$!\" > {}\nwait\n",
            shell_word(&marker)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let processes = FixtureProcesses(marker);
    let mut cancelled_at = None;
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[(vec!["active-probe", "pending"], b"q".to_vec())],
        24,
        132,
        |_| {
            processes.assert_running();
            cancelled_at = Some(Instant::now());
        },
    );
    processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn inventory_quit_reaps_a_traversal_child_while_traversal_is_active() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("traversal-started");
    fs::create_dir_all(root.join("slow-bare/objects")).unwrap();
    fs::create_dir_all(root.join("slow-bare/refs")).unwrap();
    fs::write(root.join("slow-bare/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    let real_git = support::real_tool("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--git-dir\" ]; then /bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait; fi\nexec {} \"$@\"\n",
            shell_word(&marker),
            shell_word(&real_git)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let processes = FixtureProcesses(marker);
    let mut cancelled_at = None;
    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[(
            vec!["generation 1", "loading local discovery"],
            b"q".to_vec(),
        )],
        24,
        132,
        |_| {
            processes.assert_running();
            cancelled_at = Some(Instant::now());
        },
    );
    processes.assert_reaped_by(cancelled_at.unwrap() + Duration::from_secs(2));
    let first_draw = transcript.split("\u{1b}[?25l").next().unwrap();
    assert!(screen_text(first_draw, 24, 132).contains("Scanning repository roots"));
}

#[cfg(unix)]
#[test]
fn inventory_rejects_a_probe_result_after_same_path_checkout_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("probe-started");
    let target = root.join("changed-target");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&target, "git@github.com:org/old-origin.git", true);
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    let real_git = support::real_tool("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf started > {}\n/bin/sleep 1\nexec {} \"$@\"\n",
            shell_word(&marker),
            shell_word(&real_git)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["changed-target", "pending"], b" ".to_vec()),
            (
                vec!["changed-target", "stale: target changed during observation"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 0 {
                wait_for_file(&marker);
                let old = temp.path().join("old-checkout");
                fs::rename(&target, &old).unwrap();
                init_repo(&target, "git@github.com:org/new-origin.git", true);
            }
        },
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(
        plain.contains("stale: target changed during observation"),
        "changed target remained forever pending instead of becoming stale:\n{plain}"
    );
    assert!(
        !plain.contains("github.com/org/old-origin"),
        "stale probe result was accepted after target replacement:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_rejects_a_probe_result_after_in_place_origin_config_change() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("probe-started");
    let target = root.join("origin-changed");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&target, "git@github.com:org/old-origin.git", true);
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    let real_git = support::real_tool("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf started > {}\n/bin/sleep 1\nexec {} \"$@\"\n",
            shell_word(&marker),
            shell_word(&real_git)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["origin-changed", "pending"], b" ".to_vec()),
            (
                vec!["origin-changed", "stale: target changed during observation"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 0 {
                wait_for_file(&marker);
                run_git(
                    &target,
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "git@github.com:org/new-origin.git",
                    ],
                );
            }
        },
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(
        !plain.contains("github.com/org/old-origin"),
        "stale probe result was accepted after origin config changed:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_declared_rows_keep_wrong_corrupt_and_unreadable_facts_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/wrong\"\n\n[[repositories]]\nurl = \"github.com/org/corrupt\"\n\n[[repositories]]\nurl = \"github.com/org/unreadable\"\n",
    )
    .unwrap();
    let wrong = root.join("org/wrong");
    init_repo(&wrong, "git@github.com:other/wrong.git", true);
    let corrupt = root.join("org/corrupt");
    init_repo(&corrupt, "git@github.com:org/corrupt.git", true);
    fs::write(corrupt.join(".git/config"), "[remote \"origin\"\n").unwrap();
    let destination = root.join("org/unreadable");
    fs::create_dir_all(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o000)).unwrap();

    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[(
            vec!["github.com/other/wrong", "origin error:", "unreadable"],
            b"q".to_vec(),
        )],
        24,
        500,
        1,
        |_| {},
    );
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    let rows = inventory_rows(&screen_text(&transcript, 24, 500));
    let wrong_row = rows
        .iter()
        .find(|row| row[0] == "github.com/other/wrong")
        .expect("wrong-origin checkout row");
    assert!(wrong_row[1].ends_with("/org/wrong"));
    assert_eq!(wrong_row[2], "unregistered");
    assert_eq!(wrong_row[4], "git@github.com:other/wrong.git");
    let declared_wrong = rows
        .iter()
        .find(|row| row[0] == "github.com/org/wrong")
        .expect("declared wrong-origin row");
    assert_eq!(declared_wrong[3], "conflict");
    let corrupt_row = rows
        .iter()
        .find(|row| row[1].ends_with("/org/corrupt") && row[0].starts_with("origin error:"))
        .unwrap_or_else(|| panic!("corrupt-origin checkout row; parsed rows: {rows:?}"));
    assert!(corrupt_row[4].starts_with("origin error:"));
    let unreadable_row = rows
        .iter()
        .find(|row| row[0] == "github.com/org/unreadable")
        .expect("declared unreadable row");
    assert_eq!(unreadable_row[3], "unreadable");
}

#[cfg(unix)]
#[test]
fn inventory_keeps_partial_summary_visible_when_rows_overflow_the_viewport() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    for index in 0..30 {
        init_repo(
            &root.join(format!("overflow-{index:02}")),
            &format!("git@github.com:org/overflow-{index:02}.git"),
            true,
        );
    }
    let unreadable = root.join("definitely-unreadable");
    fs::create_dir_all(&unreadable).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[(vec!["partial scan:"], b"q".to_vec())],
        12,
        220,
        1,
        |_| {},
    );
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();
    let plain = screen_text(&transcript, 12, 220);
    assert!(
        plain.contains("partial scan:"),
        "summary was clipped:\n{plain}"
    );
    assert!(
        !plain.contains("overflow-29"),
        "viewport did not overflow:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_informational_git_file_exclusions_do_not_make_scan_partial() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let primary = root.join("primary");
    init_repo(&primary, "git@github.com:org/primary.git", true);
    run_git(
        &primary,
        &[
            "worktree",
            "add",
            "-q",
            root.join("linked").to_str().unwrap(),
        ],
    );
    run_git(
        temp.path(),
        &["init", "--bare", root.join("bare.git").to_str().unwrap()],
    );
    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config).arg("inventory"),
        &[
            "ignored repositories:",
            "bare repository ignored",
            "linked worktree or submodule ignored",
        ],
        b"q",
        24,
        132,
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(plain.contains("ignored repositories:"));
    assert!(
        !plain.contains("partial scan:"),
        "exclusions became partial:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_excludes_git_file_entries_reports_unreadable_paths_escapes_controls_and_restores_terminal()
 {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();

    let primary = root.join("primary");
    init_repo(&primary, "git@github.com:org/primary.git", true);
    let linked = root.join("linked-worktree");
    run_git(
        &primary,
        &["worktree", "add", "-q", linked.to_str().unwrap()],
    );
    let submodule_source = temp.path().join("submodule-source");
    init_repo(
        &submodule_source,
        "git@github.com:org/submodule-source.git",
        true,
    );
    // The superproject deliberately lives outside the scan root: the reachable submodule below
    // must be excluded for its own .git-file marker, not because parent-root pruning hid it.
    let submodule_parent = temp.path().join("submodule-parent");
    init_repo(
        &submodule_parent,
        "git@github.com:org/submodule-parent.git",
        true,
    );
    run_git(
        &submodule_parent,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            submodule_source.to_str().unwrap(),
            "nested-unique",
        ],
    );
    let original_git_dir =
        fs::canonicalize(submodule_parent.join(".git/modules/nested-unique")).unwrap();
    let reachable_submodule = root.join("reachable-submodule");
    fs::rename(submodule_parent.join("nested-unique"), &reachable_submodule).unwrap();
    fs::write(
        reachable_submodule.join(".git"),
        format!("gitdir: {}\n", original_git_dir.display()),
    )
    .unwrap();
    run_git(
        temp.path(),
        &[
            "--git-dir",
            original_git_dir.to_str().unwrap(),
            "--work-tree",
            reachable_submodule.to_str().unwrap(),
            "config",
            "core.worktree",
            reachable_submodule.to_str().unwrap(),
        ],
    );
    let inside = Command::new("git")
        .args([
            "-C",
            reachable_submodule.to_str().unwrap(),
            "rev-parse",
            "--is-inside-work-tree",
        ])
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&inside.stdout).trim(), "true");
    let git_dir = Command::new("git")
        .args([
            "-C",
            reachable_submodule.to_str().unwrap(),
            "rev-parse",
            "--git-dir",
        ])
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(git_dir.status.success());
    assert_eq!(
        fs::canonicalize(String::from_utf8_lossy(&git_dir.stdout).trim()).unwrap(),
        original_git_dir
    );
    init_repo(
        &root.join("control\n\t\u{1b}[31mred"),
        "git@github.com:org/control.git",
        true,
    );
    let unreadable = root.join("unreadable");
    fs::create_dir_all(&unreadable).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[(
            vec![
                "ignored repositories:",
                "reachable-submodule",
                "linked-worktree",
                "partial scan:",
                "\\n",
                "\\t",
                "\\x1b",
            ],
            b"q".to_vec(),
        )],
        80,
        220,
        1,
        |_| {},
    );
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();

    let plain = screen_text(&transcript, 80, 220);
    assert!(
        plain.contains("ignored repositories:") && plain.contains("reachable-submodule"),
        "reachable submodule was not independently excluded:\n{plain}"
    );
    assert!(
        plain.contains("linked-worktree"),
        "linked worktree was not independently excluded:\n{plain}"
    );
    assert!(
        plain.contains("partial scan:"),
        "unreadable scan diagnostic was not counted in the persistent summary:\n{plain}"
    );
    assert!(
        plain.contains("\\n") && plain.contains("\\t") && plain.contains("\\x1b"),
        "control path injected terminal text:\n{plain}"
    );
    assert!(
        !plain.contains("nested-unique"),
        "scanner descended into the real submodule:\n{plain}"
    );
    assert!(
        transcript.contains("\u{1b}[?1049h"),
        "alternate screen was not entered"
    );
    assert!(
        transcript.contains("\u{1b}[?1049l"),
        "alternate screen was not restored"
    );
    assert!(
        transcript.contains("\u{1b}[?25h"),
        "cursor was not restored"
    );
}

#[cfg(unix)]
#[test]
fn offline_inventory_renders_discovery_identity_and_status_rows_in_a_pty() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    fs::create_dir_all(&root).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        r#"root = "repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/org/managed"

[[repositories]]
url = "github.com/org/missing"

[[repositories]]
url = "github.com/org/*"
exclude = ["skip"]
"#,
    )
    .unwrap();

    init_repo(
        &root.join("org/managed"),
        "git@github.com:org/managed.git",
        true,
    );
    init_repo(
        &root.join("elsewhere/managed"),
        "git@github.com:org/managed.git",
        true,
    );
    init_repo(
        &root.join("org/extra"),
        "git@github.com:org/extra.git",
        false,
    );
    init_repo(&root.join("org/skip"), "git@github.com:org/skip.git", true);
    init_repo_without_origin(&root.join("local/no-origin"));
    init_repo(
        &root.join("deep/outer"),
        "git@github.com:org/outer.git",
        true,
    );
    init_repo(
        &root.join("deep/outer/ignored-nested"),
        "git@github.com:org/ignored.git",
        true,
    );
    run_git(
        temp.path(),
        &["init", "--bare", root.join("bare.git").to_str().unwrap()],
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("org"), root.join("linked-org")).unwrap();

    support::clear_outbound_requests();
    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config).arg("inv"),
        &[
            "8 rows",
            "ignored repositories: bare.git: bare repository ignored",
        ],
        b"q",
        80,
        220,
    );
    let plain = screen_text(&transcript, 80, 220);

    for header in [
        "repository",
        "path",
        "registration",
        "checkout",
        "origin",
        "branch",
        "changes",
    ] {
        assert!(plain.contains(header), "missing header {header}:\n{plain}");
    }
    for expected in [
        "github.com/org/*",
        "pattern",
        "configured:",
        "ignored repositories: bare.git: bare repository ignored",
    ] {
        assert!(plain.contains(expected), "missing {expected}:\n{plain}");
    }
    let rows = inventory_rows(&plain);
    let assert_row = |label: &str, expected: [String; 7]| {
        assert!(
            rows.iter().any(|row| row == &expected),
            "missing exact {label} row {expected:?}; parsed rows: {rows:?}"
        );
    };
    let root = root.to_string_lossy();
    assert_row(
        "pattern",
        [
            "github.com/org/*".to_owned(),
            format!("{root}/org"),
            "pattern".to_owned(),
            "pattern".to_owned(),
            "git@github.com:org/*.git".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        ],
    );
    assert_row(
        "missing declaration",
        [
            "github.com/org/missing".to_owned(),
            format!("{root}/org/missing"),
            "explicit".to_owned(),
            "missing".to_owned(),
            "git@github.com:org/missing.git".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        ],
    );
    assert_row(
        "exact managed checkout",
        [
            "github.com/org/managed".to_owned(),
            format!("{root}/org/managed"),
            "explicit".to_owned(),
            "cloned".to_owned(),
            "git@github.com:org/managed.git".to_owned(),
            "main".to_owned(),
            "known clean".to_owned(),
        ],
    );
    assert_row(
        "misplaced duplicate managed checkout",
        [
            "github.com/org/managed".to_owned(),
            format!("{root}/elsewhere/managed"),
            "explicit".to_owned(),
            "cloned".to_owned(),
            "git@github.com:org/managed.git".to_owned(),
            "main".to_owned(),
            "known clean".to_owned(),
        ],
    );
    assert_row(
        "wildcard checkout",
        [
            "github.com/org/extra".to_owned(),
            format!("{root}/org/extra"),
            "wildcard".to_owned(),
            "cloned".to_owned(),
            "git@github.com:org/extra.git".to_owned(),
            "main".to_owned(),
            "1 modified".to_owned(),
        ],
    );
    assert_row(
        "excluded checkout",
        [
            "github.com/org/skip".to_owned(),
            format!("{root}/org/skip"),
            "excluded".to_owned(),
            "cloned".to_owned(),
            "git@github.com:org/skip.git".to_owned(),
            "main".to_owned(),
            "known clean".to_owned(),
        ],
    );
    assert_row(
        "originless checkout",
        [
            "No origin".to_owned(),
            format!("{root}/local/no-origin"),
            "unregistered".to_owned(),
            "cloned".to_owned(),
            "No origin".to_owned(),
            "main".to_owned(),
            "known clean".to_owned(),
        ],
    );
    assert!(
        !plain.contains("ignored-nested"),
        "descended into a discovered Git root:\n{plain}"
    );
    assert!(
        !plain.contains("linked-org"),
        "followed a directory symlink:\n{plain}"
    );
    assert!(
        support::outbound_requests().is_empty(),
        "offline inventory made provider requests"
    );
}

#[cfg(unix)]
#[test]
fn inventory_navigates_every_row_without_shifting_columns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let mut content = String::from("root = \"repos\"\n");
    for index in 0..30 {
        content.push_str(&format!(
            "\n[[repositories]]\nurl = \"github.com/org/repo-{index:02}\"\n"
        ));
    }
    fs::write(&config, content).unwrap();
    for alias in ["inventory", "inv"] {
        let transcript = run_inventory_pty_actions(
            support::lager(&home, &config)
                .arg(alias)
                .env("NO_COLOR", "1"),
            &[
                (
                    vec!["selected: github.com/org/repo-00", "local scan complete"],
                    vec![b'j'; 29],
                ),
                (vec!["selected: github.com/org/repo-29"], b"\x1b[A".to_vec()),
                (vec!["selected: github.com/org/repo-28"], b"k".to_vec()),
                (vec!["selected: github.com/org/repo-27"], b"\x1b[B".to_vec()),
                (vec!["selected: github.com/org/repo-28"], b"q".to_vec()),
            ],
            24,
            80,
        );
        let screen = screen_text(&transcript, 24, 80);
        for text in [
            "registration",
            "checkout",
            "branch",
            "changes",
            "explicit",
            "missing",
        ] {
            assert!(screen.contains(text), "missing {text}:\n{screen}");
        }
        assert!(
            !screen.contains("repo-00"),
            "viewport failed to scroll:\n{screen}"
        );
        let selected =
            highlighted_inventory_row(&transcript, 24, 80, "github.com/org/repo-28").unwrap();
        let adjacent = screen
            .lines()
            .find(|line| line.contains("github.com/org/repo-27"))
            .unwrap();
        assert_eq!(selected.find("explicit"), adjacent.find("explicit"));
        assert!(transcript.contains("\x1b[?1049l"));
        assert!(transcript.contains("\x1b[?25h"));
    }
}

#[cfg(unix)]
#[test]
fn inventory_resizes_from_wide_to_compact_without_losing_whole_row_selection() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/beta\"\n",
    )
    .unwrap();

    let mut frames = Vec::new();
    let transcript = run_inventory_pty_with_exit_observing(
        support::lager(&home, &config)
            .arg("inventory")
            .env("NO_COLOR", "1"),
        &[
            (
                vec![
                    "repository",
                    "path",
                    "registration",
                    "checkout",
                    "origin",
                    "branch",
                    "changes",
                    "selected: github.com/org/alpha",
                    "local scan complete",
                ],
                b"j".to_vec(),
            ),
            (
                vec!["selected: github.com/org/beta", "registration", "changes"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        0,
        |action, writer, frame| {
            if action == 0 {
                frames.push(frame.to_owned());
                resize_pty(writer, 24, 80);
            } else {
                frames.push(frame.to_owned());
            }
        },
    );

    let [wide_frame, compact_frame] = frames.as_slice() else {
        panic!("expected a wide and compact frame: {frames:?}");
    };
    let wide = screen_text(wide_frame, 24, 132);
    let compact = screen_text(compact_frame, 24, 80);
    assert!(wide.contains("path") && wide.contains("origin"), "{wide}");
    assert!(
        highlighted_inventory_row(wide_frame, 24, 132, "github.com/org/alpha").is_some(),
        "{wide}"
    );
    assert!(
        highlighted_inventory_row(compact_frame, 24, 80, "github.com/org/beta").is_some(),
        "{compact}"
    );
    for column in [
        "repository",
        "registration",
        "checkout",
        "branch",
        "changes",
    ] {
        assert!(compact.contains(column), "missing {column}:\n{compact}");
    }
    assert!(
        !compact.contains("path") && !compact.contains("origin"),
        "{compact}"
    );
    let selected = highlighted_inventory_row(compact_frame, 24, 80, "github.com/org/beta").unwrap();
    let adjacent = compact
        .lines()
        .find(|line| line.contains("github.com/org/alpha"))
        .unwrap();
    assert_eq!(
        selected.find("explicit"),
        adjacent.find("explicit"),
        "selection shifted a compact table column:\n{compact}"
    );
    assert!(transcript.contains("\x1b[?1049l") && transcript.contains("\x1b[?25h"));
}

#[cfg(unix)]
#[test]
fn inventory_navigation_remains_responsive_while_status_is_pending() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let marker = temp.path().join("status-pending");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/beta\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let tools = temp.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\nif [ \"$3\" = status ]; then printf started > {}; while :; do /bin/sleep 30; done; fi\nexec {} \"$@\"\n",
            shell_word(&marker),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();

    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["github.com/org/alpha", "github.com/org/beta", "pending"],
                b"j".to_vec(),
            ),
            (
                vec!["selected: github.com/org/beta", "pending"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 0 {
                wait_for_file(&marker);
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_shows_unknown_config_warnings_without_corrupting_the_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\nfuture_key = \"keep\"\n").unwrap();
    let transcript = run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec![
                    "warning: unknown config key `future_key`",
                    "local scan complete",
                ],
                b"R".to_vec(),
            ),
            (
                vec!["generation 2", "warning: unknown config key `future_key`"],
                b"q".to_vec(),
            ),
        ],
        24,
        120,
        |index| {
            if index == 0 {
                fs::write(&config, "root = \"repos\"\nfuture_key = \"changed\"\n").unwrap();
            }
        },
    );
    assert!(!transcript.contains("lager: warning:"));
}

#[cfg(unix)]
#[test]
fn inventory_bindings_refresh_atomically_and_keep_failures_sticky() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let base = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[[repositories]]\nurl = \"github.com/org/beta\"\n";
    fs::write(
        &config,
        format!("{base}\n[inventory.keys.normal]\ndown = [\"n\"]\nrefresh = [\"x\"]\n"),
    )
    .unwrap();
    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["selected: github.com/org/alpha", "n down", "x refresh"],
                b"j?".to_vec(),
            ),
            (vec!["HELP", "n down", "x refresh"], b"\x1b".to_vec()),
            (
                vec!["NORMAL", "selected: github.com/org/alpha"],
                b"n".to_vec(),
            ),
            (vec!["selected: github.com/org/beta"], b"Rx".to_vec()),
            (
                vec!["generation 2", "y refresh", "selected: github.com/org/beta"],
                b"y".to_vec(),
            ),
            (
                vec!["refresh failed:", "collision", "y refresh"],
                b"y".to_vec(),
            ),
            (
                vec!["generation 3", "z refresh", "selected: github.com/org/beta"],
                b"q".to_vec(),
            ),
        ],
        24,
        100,
        1,
        |index| {
            let keys = match index {
                3 => "down = [\"n\"]\nrefresh = [\"y\"]",
                4 => "refresh = [\"j\"]",
                5 => "down = [\"n\"]\nrefresh = [\"z\"]",
                _ => return,
            };
            fs::write(
                &config,
                format!("{base}\n[inventory.keys.normal]\n{keys}\n"),
            )
            .unwrap();
        },
    );
    assert!(screen_text(&transcript, 24, 100).contains("generation 3"));
}

#[cfg(unix)]
#[test]
fn inventory_search_bindings_override_accept_and_retain_the_previous_map_on_refresh_error() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let base = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(
        &config,
        format!(
            "{base}\n[inventory.keys.normal]\nrefresh = [\"x\"]\n\
             [inventory.keys.search]\naccept = [\"Ctrl+a\"]\ncancel = [\"Ctrl+e\"]\n"
        ),
    )
    .unwrap();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL", "x refresh", "github.com/org/alpha"],
                b"/alpha".to_vec(),
            ),
            (
                vec!["SEARCH / alpha", "Ctrl+a accept", "Ctrl+e cancel"],
                b"\r".to_vec(),
            ),
            (
                vec!["SEARCH / alpha", "github.com/org/alpha"],
                b"\x01".to_vec(),
            ),
            (vec!["NORMAL / alpha", "x refresh"], b"x".to_vec()),
            (
                vec![
                    "refresh failed:",
                    "preserve ordinary input",
                    "NORMAL / alpha",
                    "x refresh",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        100,
        1,
        |index| {
            if index == 3 {
                fs::write(
                    &config,
                    format!(
                        "{base}\n[inventory.keys.normal]\nrefresh = [\"x\"]\n\
                         [inventory.keys.search]\naccept = [\"a\"]\n"
                    ),
                )
                .unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_help_paging_overrides_scroll_and_resize_clamps_the_offset() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[inventory.keys.inspection]\npage_up = [\"z\"]\npage_down = [\"x\"]\n",
    )
    .unwrap();

    let mut frames = Vec::new();
    let transcript = run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["local scan complete", "? help"], b"?".to_vec()),
            (
                vec!["HELP / inventory", "k/Up up", "z page_up", "x page_down"],
                b"x".to_vec(),
            ),
            (
                vec![
                    "HELP / inventory",
                    "Backspace clear_search",
                    "Enter inspect",
                    "m menu",
                ],
                b"z".to_vec(),
            ),
            (
                vec!["HELP / inventory", "k/Up up", "z page_up", "x page_down"],
                b"x".to_vec(),
            ),
            (
                vec![
                    "HELP / inventory",
                    "Backspace clear_search",
                    "Enter inspect",
                    "m menu",
                ],
                Vec::new(),
            ),
            (vec!["HELP / inventory", "k/Up up"], b"q".to_vec()),
        ],
        8,
        100,
        0,
        |action, writer, frame| match action {
            4 => {
                frames.push(frame.to_owned());
                resize_pty(writer, 20, 100);
            }
            5 => frames.push(frame.to_owned()),
            _ => {}
        },
    );

    let [scrolled_frame, resized_frame] = frames.as_slice() else {
        panic!("expected a scrolled and resized help frame: {frames:?}");
    };
    let scrolled = screen_text(scrolled_frame, 8, 100);
    assert!(scrolled.contains("Enter inspect"), "{scrolled}");
    assert!(
        !scrolled.contains("k/Up up"),
        "custom page-down did not scroll help: {scrolled}"
    );

    let resized = screen_text(resized_frame, 20, 100);
    for text in ["HELP / inventory", "k/Up up", "x page_down"] {
        assert!(resized.contains(text), "missing {text}:\n{resized}");
    }
    assert!(
        transcript.contains("\x1b[?1049l") && transcript.contains("\x1b[?25h"),
        "terminal was not restored: {transcript}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_selection_follows_a_pending_path_when_its_identity_arrives() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&root.join("a-ready"), "git@github.com:org/zzz.git", true);
    let pending = root.join("z-pending");
    init_repo(&pending, "git@github.com:org/aaa.git", true);
    let tools = temp.path().join("tools");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), format!(
        "#!/bin/sh\ncase \"$*\" in *z-pending*) while [ ! -f {} ]; do /bin/sleep 0.02; done;; esac\nexec {} \"$@\"\n",
        shell_word(&release), shell_word(&support::real_tool("git"))
    )).unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let selected_pending = format!("selected: {}", pending.display());
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["selected: github.com/org/zzz", "pending", "z-pending"],
                b"j".to_vec(),
            ),
            (vec![&selected_pending], b" ".to_vec()),
            (
                vec!["selected: github.com/org/aaa", "local scan complete"],
                b"q".to_vec(),
            ),
        ],
        24,
        500,
        |index| {
            if index == 1 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_validates_all_binding_modes_before_entering_raw_mode() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    for (settings, expected) in [
        (
            "[inventory.keys.typo]\nquit = [\"x\"]",
            "unknown inventory key mode",
        ),
        (
            "[inventory.keys.normal]\ntypo = [\"x\"]",
            "unknown inventory action",
        ),
        ("[inventory.keys.normal]\nrefresh = [\"j\"]", "collision"),
        (
            "[inventory.keys.normal]\nquit = []",
            "must remain reachable",
        ),
        (
            "[inventory.keys.normal]\nhelp = []",
            "must remain reachable",
        ),
        (
            "[inventory.keys.menu]\naccept = []",
            "must remain reachable",
        ),
        (
            "[inventory.keys.inspection]\ncancel = []",
            "must remain reachable",
        ),
        (
            "[inventory.keys.confirmation]\nquit = []",
            "must remain reachable",
        ),
        (
            "[inventory.keys.search]\nhelp = [\"q\"]",
            "preserve ordinary input",
        ),
        (
            "[inventory.keys.input]\naccept = [\"Backspace\"]",
            "preserve ordinary input",
        ),
        (
            "[inventory.keys.normal]\nrefresh = [\"Ctrl+c\"]",
            "native terminal controls",
        ),
        (
            "[inventory.keys.normal]\nrefresh = [\"Banana\"]",
            "invalid inventory key",
        ),
    ] {
        fs::write(&config, format!("root = \"repos\"\n{settings}\n")).unwrap();
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("inventory"),
            &[],
            24,
            80,
            1,
            |_| {},
        );
        assert!(transcript.contains(expected), "{settings}: {transcript}");
        assert!(
            !transcript.contains("\x1b[?1049h"),
            "raw terminal entered for invalid bindings"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_bindings_survive_symlinked_registration_and_hide_planned_actions() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let target = temp.path().join("settings.toml");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let settings = "[inventory.keys.normal]\nrefresh = [\"x\"]\nregister = [\"g\"]\n[inventory.keys.inspection]\ncancel = [\"Backspace\"]\n";
    fs::write(&target, format!("root = \"repos\"\n{settings}")).unwrap();
    std::os::unix::fs::symlink(&target, &config).unwrap();
    for action in ["register", "unregister"] {
        let output = output(support::lager(&home, &config).args([action, "github.com/org/repo"]));
        assert!(output.status.success(), "{output:?}");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("unknown config key `inventory`")
        );
        assert!(config.is_symlink());
        assert!(fs::read_to_string(&target).unwrap().contains(settings));
    }
    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["No repositories found", "x refresh"], b"g?".to_vec()),
            (vec!["HELP", "Backspace cancel"], vec![127]),
            (vec!["NORMAL", "x refresh"], b"q".to_vec()),
        ],
        24,
        100,
    );
    assert!(!screen_text(&transcript, 24, 100).contains("g register"));
    assert!(
        !fs::read_to_string(&target)
            .unwrap()
            .contains("[[repositories]]")
    );
}

#[cfg(unix)]
#[test]
fn inventory_refresh_preserves_selection_across_checkout_and_declaration_rows() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let configured = root.join("org/same");
    let duplicate = root.join("other");
    init_repo(&root.join("alpha"), "git@github.com:org/alpha.git", true);
    init_repo(&configured, "git@github.com:org/same.git", true);
    init_repo(&duplicate, "git@github.com:org/same.git", true);
    let mut frames = Vec::new();

    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["local scan complete", "selected: github.com/org/alpha"],
                b"j/cloned\r".to_vec(),
            ),
            (vec!["NORMAL / cloned · 3 rows"], b"R".to_vec()),
            (
                vec!["generation 2", "explicit", "selected: github.com/org/same"],
                b"R".to_vec(),
            ),
            (
                vec![
                    "generation 3",
                    "unregistered",
                    "selected: github.com/org/same",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        500,
        0,
        |index, _, frame| match index {
            1 => fs::write(
                &config,
                "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/same\"\n",
            )
            .unwrap(),
            2 => {
                frames.push(frame.to_owned());
                fs::write(&config, "root = \"repos\"\n").unwrap();
            }
            3 => frames.push(frame.to_owned()),
            _ => {}
        },
    );

    let [declaration_frame, checkout_frame] = frames.as_slice() else {
        panic!("expected declaration and checkout frames: {frames:?}");
    };
    for (representation, frame) in [
        ("declaration", declaration_frame),
        ("checkout", checkout_frame),
    ] {
        let screen = screen_text(frame, 24, 500);
        assert!(
            screen.contains("NORMAL / cloned · 3 rows"),
            "filter did not survive refresh: {screen}"
        );
        let highlighted = highlighted_inventory_row(frame, 24, 500, "github.com/org/same")
            .unwrap_or_else(|| panic!("{representation} row was not selected:\n{screen}"));
        assert!(
            highlighted.contains(configured.to_string_lossy().as_ref()),
            "{highlighted}"
        );
        assert!(
            !highlighted.contains(duplicate.to_string_lossy().as_ref()),
            "duplicate path took selection: {highlighted}"
        );
    }

    let declared_rows = inventory_rows(&screen_text(declaration_frame, 24, 500));
    let checkout_rows = inventory_rows(&screen_text(checkout_frame, 24, 500));
    let row_position = |rows: &[Vec<String>], path: &std::path::Path| {
        rows.iter()
            .position(|row| row[1] == path.to_string_lossy())
            .unwrap_or_else(|| panic!("missing {} in {rows:?}", path.display()))
    };
    assert!(
        row_position(&declared_rows, &configured)
            < row_position(&declared_rows, &root.join("alpha")),
        "declaration form did not move ahead in sort order: {declared_rows:?}"
    );
    assert!(
        row_position(&checkout_rows, &configured)
            > row_position(&checkout_rows, &root.join("alpha")),
        "checkout form did not return to checkout sort order: {checkout_rows:?}"
    );
}

fn output(command: &mut Command) -> std::process::Output {
    command.output().unwrap()
}

#[cfg(unix)]
static TRANSCRIPT_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(unix)]
fn run_inventory_pty_with_size(
    command: &mut Command,
    waits: &[&str],
    input: &[u8],
    rows: u16,
    cols: u16,
) -> String {
    run_inventory_pty_actions(command, &[(waits.to_vec(), input.to_vec())], rows, cols)
}

#[cfg(unix)]
fn run_inventory_pty_actions(
    command: &mut Command,
    actions: &[(Vec<&str>, Vec<u8>)],
    rows: u16,
    cols: u16,
) -> String {
    run_inventory_pty_actions_with_hook(command, actions, rows, cols, |_| {})
}

#[cfg(unix)]
fn run_inventory_pty_actions_with_hook(
    command: &mut Command,
    actions: &[(Vec<&str>, Vec<u8>)],
    rows: u16,
    cols: u16,
    before_action: impl FnMut(usize),
) -> String {
    run_inventory_pty_with_exit(command, actions, rows, cols, 0, before_action)
}

#[cfg(unix)]
fn run_inventory_pty_with_exit(
    command: &mut Command,
    actions: &[(Vec<&str>, Vec<u8>)],
    rows: u16,
    cols: u16,
    expected_exit: i32,
    mut before_action: impl FnMut(usize),
) -> String {
    run_inventory_pty_with_exit_observing(
        command,
        actions,
        rows,
        cols,
        expected_exit,
        |action, _, _| before_action(action),
    )
}

#[cfg(unix)]
fn run_inventory_pty_with_exit_observing(
    command: &mut Command,
    actions: &[(Vec<&str>, Vec<u8>)],
    rows: u16,
    cols: u16,
    expected_exit: i32,
    mut before_action: impl FnMut(usize, &mut fs::File, &str),
) -> String {
    let winsize = rustix_openpty::rustix::termios::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = rustix_openpty::openpty(None, Some(&winsize)).unwrap();
    let mut writer = fs::File::from(pty.controller);
    let before = rustix_openpty::rustix::termios::tcgetattr(&writer).unwrap();
    let flags = rustix::fs::fcntl_getfl(&writer).unwrap();
    rustix::fs::fcntl_setfl(&writer, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
    let terminal = fs::File::from(pty.user);
    command
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.try_clone().unwrap()))
        .stderr(Stdio::from(terminal));
    let mut child = PtyChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut transcript = String::new();
    let mut action_index = 0;
    loop {
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            let _ = child.0.wait();
            retain_transcript(&transcript, rows, cols);
            panic!("inventory PTY timed out; transcript retained:\n{transcript}");
        }
        read_pty(&mut writer, &mut bytes);
        transcript = String::from_utf8_lossy(&bytes).into_owned();
        let completed_frame = transcript
            .rfind("\x1b[0m")
            .map(|end| &transcript[..end + "\x1b[0m".len()])
            .unwrap_or_default();
        if let Some((waits, input)) = actions.get(action_index)
            && waits.iter().all(|text| {
                if let Some(repository) = text.strip_prefix("selected: ") {
                    highlighted_inventory_row(
                        completed_frame,
                        rows as usize,
                        cols as usize,
                        repository,
                    )
                    .is_some()
                } else {
                    screen_text(completed_frame, rows as usize, cols as usize).contains(text)
                }
            })
        {
            before_action(action_index, &mut writer, completed_frame);
            writer.write_all(input).unwrap();
            writer.flush().unwrap();
            action_index += 1;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            let after = rustix_openpty::rustix::termios::tcgetattr(&writer).unwrap();
            retain_transcript(&transcript, rows, cols);
            assert_eq!(
                action_index,
                actions.len(),
                "child exited before all actions:\n{transcript}"
            );
            read_pty(&mut writer, &mut bytes);
            transcript = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(status.code(), Some(expected_exit), "{transcript}");
            assert_eq!(
                termios_fingerprint(&before),
                termios_fingerprint(&after),
                "normal quit did not restore termios:\n{transcript}"
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    transcript
}

#[cfg(unix)]
struct PtyChild(std::process::Child);

#[cfg(unix)]
impl Drop for PtyChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn resize_pty(terminal: &fs::File, rows: u16, cols: u16) {
    rustix_openpty::rustix::termios::tcsetwinsize(
        terminal,
        rustix_openpty::rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .unwrap();
}

#[cfg(unix)]
fn read_pty(reader: &mut fs::File, bytes: &mut Vec<u8>) {
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => bytes.extend_from_slice(&buffer[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(5) =>
            {
                break;
            }
            Err(error) => panic!("read inventory PTY: {error}"),
        }
    }
}

#[cfg(unix)]
fn termios_fingerprint(termios: &rustix_openpty::rustix::termios::Termios) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
        termios.input_modes,
        termios.output_modes,
        termios.control_modes,
        termios.local_modes,
        termios.special_codes,
        termios.input_speed(),
        termios.output_speed()
    )
}

#[cfg(unix)]
fn wait_for_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        path.exists(),
        "expected work marker was not created: {}",
        path.display()
    );
}

#[cfg(unix)]
struct FixtureProcesses(std::path::PathBuf);

#[cfg(unix)]
impl FixtureProcesses {
    fn pids(&self) -> Vec<rustix::process::Pid> {
        fs::read_to_string(&self.0)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok().and_then(rustix::process::Pid::from_raw))
            .collect()
    }

    fn assert_running(&self) {
        wait_for_file(&self.0);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.pids().len() != 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pids = self.pids();
        assert_eq!(pids.len(), 2, "wrapper and descendant must be recorded");
        for pid in pids {
            assert!(
                rustix::process::test_kill_process(pid).is_ok(),
                "{pid:?} is not running"
            );
        }
    }

    fn assert_reaped_by(&self, deadline: Instant) {
        let pids = self.pids();
        while pids
            .iter()
            .any(|pid| rustix::process::test_kill_process(*pid).is_ok())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            Instant::now() < deadline,
            "cancellation exceeded two seconds"
        );
        for pid in pids {
            assert_eq!(
                rustix::process::test_kill_process(pid),
                Err(rustix::io::Errno::SRCH)
            );
        }
    }
}

#[cfg(unix)]
impl Drop for FixtureProcesses {
    fn drop(&mut self) {
        for pid in self.pids() {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}

#[cfg(unix)]
fn inventory_rows(screen: &str) -> Vec<Vec<String>> {
    screen
        .lines()
        .map(|line| line.trim_matches('│').trim().trim_start_matches("> "))
        .filter(|line| line.matches(" | ").count() >= 6)
        .map(|line| {
            line.split(" | ")
                .map(str::trim)
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

#[cfg(unix)]
fn retain_transcript(transcript: &str, rows: u16, cols: u16) {
    let sequence = TRANSCRIPT_ID.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/inventory-e2e-transcripts");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{sequence}.raw")), transcript).unwrap();
    fs::write(
        directory.join(format!("{sequence}.screen")),
        screen_text(transcript, rows as usize, cols as usize),
    )
    .unwrap();
}

#[cfg(unix)]
fn shell_word(path: &std::path::Path) -> String {
    format!(
        "'{}'",
        path.display().to_string().replace('\'', "'\\\"'\\\"'")
    )
}

fn init_repo(path: &std::path::Path, origin: &str, clean: bool) {
    fs::create_dir_all(path).unwrap();
    run_git(path, &["init", "-q"]);
    run_git(path, &["checkout", "-qb", "main"]);
    run_git(path, &["config", "user.email", "lager@example.invalid"]);
    run_git(path, &["config", "user.name", "lager"]);
    run_git(path, &["remote", "add", "origin", origin]);
    fs::write(path.join("README"), "initial\n").unwrap();
    run_git(path, &["add", "README"]);
    run_git(path, &["commit", "-qm", "initial"]);
    if !clean {
        fs::write(path.join("README"), "modified\n").unwrap();
    }
}

fn init_repo_without_origin(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    run_git(path, &["init", "-q"]);
    run_git(path, &["checkout", "-qb", "main"]);
    run_git(path, &["config", "user.email", "lager@example.invalid"]);
    run_git(path, &["config", "user.name", "lager"]);
    fs::write(path.join("README"), "local\n").unwrap();
    run_git(path, &["add", "README"]);
    run_git(path, &["commit", "-qm", "initial"]);
}

fn run_git(directory: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(directory)
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

fn highlighted_inventory_row(
    input: &str,
    rows: usize,
    cols: usize,
    repository: &str,
) -> Option<String> {
    let repository = repository.chars().collect::<Vec<_>>();
    styled_screen(input, rows, cols)
        .into_iter()
        .find_map(|line| {
            let start = line.windows(repository.len()).position(|cells| {
                cells
                    .iter()
                    .map(|(character, _)| *character)
                    .eq(repository.iter().copied())
            })?;
            let selected = line[start..start + repository.len()]
                .iter()
                .all(|(_, reversed)| *reversed);
            let fixed_mark_space = start >= 2
                && line[start - 2..start]
                    .iter()
                    .all(|(character, reversed)| *character == ' ' && *reversed);
            (selected && fixed_mark_space).then(|| {
                line.into_iter()
                    .map(|(character, _)| character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
        })
}

fn screen_text(input: &str, rows: usize, cols: usize) -> String {
    styled_screen(input, rows, cols)
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|(character, _)| character)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn styled_screen(input: &str, rows: usize, cols: usize) -> Vec<Vec<(char, bool)>> {
    let mut screen = vec![vec![(' ', false); cols]; rows];
    let (mut row, mut col) = (0usize, 0usize);
    let mut reversed = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            let mut code = String::new();
            for next in chars.by_ref() {
                let done = next.is_ascii_alphabetic() || next == '~';
                code.push(next);
                if done {
                    break;
                }
            }
            let command = code.chars().last().unwrap_or('m');
            let args = &code[..code.len().saturating_sub(1)];
            match command {
                'H' | 'f' => {
                    let mut parts = args
                        .split(';')
                        .filter_map(|part| part.trim_start_matches('?').parse::<usize>().ok());
                    row = parts
                        .next()
                        .unwrap_or(1)
                        .saturating_sub(1)
                        .min(rows.saturating_sub(1));
                    col = parts
                        .next()
                        .unwrap_or(1)
                        .saturating_sub(1)
                        .min(cols.saturating_sub(1));
                }
                'J' if args.ends_with('2') || args.is_empty() => {
                    for line in &mut screen {
                        line.fill((' ', false));
                    }
                    row = 0;
                    col = 0;
                }
                'K' if row < rows => {
                    for cell in &mut screen[row][col..] {
                        *cell = (' ', false);
                    }
                }
                'm' => {
                    for code in args.split(';') {
                        match code.parse().unwrap_or(0) {
                            0 | 27 => reversed = false,
                            7 => reversed = true,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\n' {
            row = (row + 1).min(rows.saturating_sub(1));
            col = 0;
        } else if ch == '\r' {
            col = 0;
        } else if !ch.is_control() && row < rows && col < cols {
            screen[row][col] = (ch, reversed);
            col = (col + 1).min(cols.saturating_sub(1));
        }
    }
    screen
}
