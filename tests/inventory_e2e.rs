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

    let selected = "selected: org/*";
    let mut scrolled_frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["selected: alpha"], b"j\r".to_vec()),
            (
                vec![
                    "Inspect / org/*",
                    "pair: Declaration github.com/org/*",
                    "pair: Registration wildcard",
                ],
                vec![b'x'; 32],
            ),
            (
                vec![
                    "INSPECT",
                    "excludes escape-\\n\\t\\x1b",
                    "pair: Register unavailable",
                    "patterns are inspect-only",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", selected, "absent: INSPECT"], b"q".to_vec()),
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
fn inventory_frames_follow_the_canvas_regions_at_wide_and_compact_sizes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/beta\"\n\n[[repositories]]\nurl = \"github.com/team/*\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        false,
    );
    init_repo(
        &root.join("archive/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(
        &root.join("team/dots"),
        "git@github.com:team/dots.git",
        true,
    );
    init_repo_without_origin(&root.join("experiments/scratch"));

    for (rows, cols, no_color) in [
        (32, 132, true),
        (32, 132, false),
        (24, 80, true),
        (24, 80, false),
    ] {
        let mut frames = Vec::new();
        let mut command = support::lager(&home, &config);
        command.arg("inventory");
        if no_color {
            command.env("NO_COLOR", "1");
        }
        let transcript = run_inventory_pty_with_exit_observing(
            &mut command,
            &[
                (
                    vec!["absent: Scanning", "NORMAL · 6 rows", "1 modified"],
                    b"/org/alpha".to_vec(),
                ),
                (vec!["SEARCH · 2 rows", "/ org/alpha"], b"\r".to_vec()),
                (vec!["NORMAL · 2 rows"], b"\r".to_vec()),
                (vec!["INSPECT · 2 rows", "Inspect / alpha"], b"?".to_vec()),
                (
                    vec!["HELP · 2 rows", "Keyboard / effective bindings"],
                    b"\x1b".to_vec(),
                ),
                (
                    vec!["INSPECT · 2 rows", "absent: Keyboard"],
                    b"\x1b".to_vec(),
                ),
                (vec!["NORMAL · 2 rows", "absent: Inspect / "], b"q".to_vec()),
            ],
            rows,
            cols,
            0,
            |action, _, frame| {
                if matches!(action, 0 | 1 | 3 | 4) {
                    frames.push(frame.to_owned());
                }
            },
        );
        let [main, search, inspect, help] = frames.as_slice() else {
            panic!("expected four frames: {frames:?}");
        };
        let (rows, cols) = (rows as usize, cols as usize);
        let compact = cols < 132;
        // Column starts of the canvas grid: term.css g7 at 132 columns, g5 at 80.
        let (headings, starts): (&[&str], &[usize]) = if compact {
            (
                &[
                    "repository",
                    "registration",
                    "checkout",
                    "branch",
                    "changes",
                ],
                &[2, 25, 40, 53, 66],
            )
        } else {
            (
                &[
                    "repository",
                    "path",
                    "registration",
                    "checkout",
                    "origin",
                    "branch",
                    "changes",
                ],
                &[2, 27, 54, 69, 82, 106, 119],
            )
        };
        let shown = |cells: [&str; 7]| -> Vec<String> {
            let picked: &[usize] = if compact {
                &[0, 2, 3, 5, 6]
            } else {
                &[0, 1, 2, 3, 4, 5, 6]
            };
            picked
                .iter()
                .map(|index| cells[*index].to_owned())
                .collect()
        };

        let screen = screen_text(main, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(
            lines[0].starts_with("  LAGER / inventory") && lines[0].ends_with("NORMAL · 6 rows"),
            "{screen}"
        );
        assert_eq!(lines[1], "  Local + declarations · remote off", "{screen}");
        assert_eq!(
            lines[2], "  / Search repositories, paths, origins…",
            "{screen}"
        );
        assert_eq!(lines[3], "", "{screen}");
        assert_eq!(
            lines[4].split_whitespace().collect::<Vec<_>>(),
            headings,
            "{screen}"
        );
        let heading_starts = headings
            .iter()
            .map(|heading| lines[4].find(heading).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(heading_starts, starts, "{screen}");
        assert!(
            !screen.contains(" | ") && !screen.contains('│') && !screen.contains("generation"),
            "{screen}"
        );
        let table = inventory_rows(&screen);
        assert_eq!(table.len(), 6, "{table:?}\n{screen}");
        for expected in [
            [
                "alpha",
                "~/repos/org/alpha",
                "explicit",
                "cloned",
                "github.com/org/alpha",
                "main",
                "1 modified",
            ],
            [
                "alpha (copy)",
                "~/repos/archive/alpha",
                "explicit",
                "cloned",
                "github.com/org/alpha",
                "main",
                "clean",
            ],
            [
                "beta",
                "~/repos/org/beta",
                "explicit",
                "missing",
                "github.com/org/beta",
                "—",
                "—",
            ],
            [
                "dots",
                "~/repos/team/dots",
                "wildcard",
                "cloned",
                "github.com/team/dots",
                "main",
                "clean",
            ],
            [
                "scratch",
                "…epos/experiments/scratch",
                "unregistered",
                "cloned",
                "No origin",
                "main",
                "clean",
            ],
            [
                "team/*",
                "—",
                "wildcard",
                "—",
                "github.com/team/*",
                "—",
                "—",
            ],
        ] {
            assert!(
                table.contains(&shown(expected)),
                "missing {expected:?} in {table:?}\n{screen}"
            );
        }
        for line in &lines[5..11] {
            let characters = line.chars().collect::<Vec<_>>();
            let slot = characters[2..6].iter().collect::<String>();
            let pattern = line.contains("team/*");
            assert!(
                characters[..2].iter().all(|character| *character == ' ')
                    && slot == if pattern { "    " } else { "[ ] " }
                    && characters[6] != ' ',
                "repository text must start after the row padding and mark slot:\n{screen}"
            );
            for start in &starts[1..] {
                assert_eq!(
                    characters[start - 1],
                    ' ',
                    "cell padding before column {start}:\n{screen}"
                );
            }
        }
        // At 80 columns the hint line stays one line and drops its least needed hint.
        let footer: &[&str] = if compact {
            &["  j/k rows  / search  Enter inspect  R refresh  m menu  ? help  q quit"]
        } else {
            &["  j/k rows  / search  Space mark  Enter inspect  R refresh  m menu  ? help  q quit"]
        };
        let status = rows - 1 - footer.len();
        assert!(
            lines[11..status].iter().all(|line| line.is_empty()),
            "{screen}"
        );
        assert_eq!(
            lines[status],
            if compact {
                "  Path and origin: Enter inspects, menu copies the full value"
            } else {
                "  No active operation · R refreshes local/config state only"
            },
            "{screen}"
        );
        assert_eq!(&lines[status + 1..], footer, "{screen}");
        let styled = styled_screen(main, rows, cols);
        assert_eq!(
            styled[0].iter().all(|(_, reversed)| *reversed),
            !no_color,
            "the header bar is reverse video only with color:\n{screen}"
        );
        assert_eq!(
            styled[rows - 1].iter().all(|(_, reversed)| *reversed),
            !no_color,
            "the hint footer is a bar only with color:\n{screen}"
        );
        let selected = table[0][0].clone();
        assert!(
            highlighted_inventory_row(main, rows, cols, &selected).is_some(),
            "{screen}"
        );

        let screen = screen_text(search, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(lines[0].ends_with("SEARCH · 2 rows"), "{screen}");
        assert_eq!(lines[2], "  / org/alpha█", "{screen}");
        assert_eq!(inventory_rows(&screen).len(), 2, "{screen}");
        assert_eq!(
            lines[rows - 2],
            "  Enter / Esc retain filter · action letters are text",
            "{screen}"
        );
        assert_eq!(
            lines[rows - 1],
            "  Enter accept  Esc cancel  Backspace delete",
            "{screen}"
        );

        let screen = screen_text(inspect, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(lines[0].ends_with("INSPECT · 2 rows"), "{screen}");
        assert_eq!(&lines[rows - footer.len()..], footer, "{screen}");
        let title = lines
            .iter()
            .position(|line| line.contains("│  Inspect / alpha  "))
            .unwrap_or_else(|| panic!("{screen}"));
        assert!(lines[title].contains("lager  │"), "{screen}");
        assert!(lines[title - 1].contains('┌'), "{screen}");
        let body = lines[title + 2..]
            .iter()
            .map(|line| {
                let inner = line.split('│').nth(1).unwrap_or_default();
                inner.strip_prefix("  ").unwrap_or(inner).trim_end()
            })
            .collect::<Vec<_>>();
        // A long value wraps inside its value column; rejoin it to compare whole fields.
        let body = body.iter().fold(Vec::<String>::new(), |mut joined, line| {
            match joined.last_mut() {
                Some(previous) if line.starts_with(&" ".repeat(19)) && !line.trim().is_empty() => {
                    previous.push_str(line.trim())
                }
                _ => joined.push((*line).to_owned()),
            }
            joined
        });
        let body = body.iter().map(String::as_str).collect::<Vec<_>>();
        let field = |name: &str, value: &str| format!("{name:<19}{value}");
        let expected = [
            field("Repository", "alpha"),
            field("Identity", "github.com/org/alpha"),
            field("Found path", "~/repos/org/alpha"),
            field("Configured path", "~/repos/org/alpha"),
            field("Registration", "explicit"),
            field(
                "Declaration",
                "github.com/org/alpha (explicit), satisfied by ~/repos/org/alpha",
            ),
            field("Checkout", "cloned"),
            field("Origin", "git@github.com:org/alpha.git"),
            field("Branch", "main"),
            field("Changes", "1 modified"),
            String::new(),
            field("Register", "no-op · identity is already registered"),
            field("Unregister", "available"),
        ];
        let visible = expected
            .iter()
            .take_while(|line| body.contains(&line.as_str()))
            .count();
        assert!(
            visible >= 8,
            "Inspect fields missing or out of board order:\n{screen}"
        );
        assert_eq!(
            &body[..visible],
            &expected[..visible]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()[..],
            "{screen}"
        );
        assert!(
            body.contains(&"j/k ↑/↓ scroll · c copy a value… · Esc close"),
            "{screen}"
        );

        let screen = screen_text(help, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(lines[0].ends_with("HELP · 2 rows"), "{screen}");
        assert_eq!(lines[rows - 2], "  No active operation", "{screen}");
        assert_eq!(lines[rows - 1], "  j/k scroll  Esc close", "{screen}");
        let title = lines
            .iter()
            .position(|line| line.contains("Keyboard / effective bindings"))
            .unwrap();
        let body = lines[title + 2..]
            .iter()
            .map(|line| {
                let inner = line.split('│').nth(1).unwrap_or_default();
                inner.strip_prefix("  ").unwrap_or(inner).trim_end()
            })
            .collect::<Vec<_>>();
        let descriptions = body
            .iter()
            .skip(1)
            .take_while(|line| !line.is_empty() && !line.starts_with("Esc close"))
            .map(|line| {
                line.split("  ")
                    .filter(|part| !part.is_empty())
                    .last()
                    .unwrap()
                    .trim()
            })
            .collect::<Vec<_>>();
        assert_eq!(body[0], "Inspect mode", "{screen}");
        assert_eq!(
            descriptions,
            [
                "scroll",
                "page up / down",
                "copy a value…",
                "close",
                "close",
                "help / quit"
            ],
            "{screen}"
        );
        assert!(
            screen_has_pair(&screen, "j/k ↑/↓ Tab/Shift+Tab scroll")
                || body[1].starts_with("j/k ↑/↓ Tab/Shift+Tab"),
            "{screen}"
        );
        assert!(body.contains(&"Esc close"), "{screen}");

        let colored = transcript.contains("\x1b[38;5;2;");
        assert_eq!(colored, !no_color, "explicit registration tint");
        assert!(
            !transcript.contains("38;2;")
                && !transcript.contains("48;2;")
                && !transcript.contains("48;5;"),
            "palette must stay terminal-relative so light and dark themes both read"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_duplicate_and_same_named_rows_distinct_at_both_sizes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\n[[repositories]]\nurl = \"github.com/org/wrong\"\n\n[[repositories]]\nurl = \"github.com/org/lone\"\n",
    )
    .unwrap();
    for path in [
        "org/alpha",
        "archive/alpha",
        "a-very-long-backup-directory-name/alpha",
        "a-very-long-backup-directory-name/older/alpha",
    ] {
        init_repo(&root.join(path), "git@github.com:org/alpha.git", true);
    }
    init_repo(
        &root.join("org/wrong"),
        "git@github.com:other/wrong.git",
        true,
    );
    init_repo(
        &root.join("elsewhere/lone"),
        "git@github.com:org/lone.git",
        true,
    );

    for (cols, no_color) in [(80, true), (132, false)] {
        let mut command = support::lager(&home, &config);
        command.arg("inventory");
        if no_color {
            command.env("NO_COLOR", "1");
        }
        let transcript = run_inventory_pty_with_size(
            &mut command,
            &["NORMAL · 8 rows", "alpha (copy 3)", "absent: Scanning"],
            b"q",
            24,
            cols,
        );
        let screen = screen_text(&transcript, 24, cols as usize);
        let rows = inventory_rows(&screen);
        let mut names = rows.iter().map(|row| row[0].as_str()).collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "alpha",
                "alpha (copy 2)",
                "alpha (copy 3)",
                "alpha (copy)",
                "lone",
                "lone",
                "org/wrong",
                "other/wrong"
            ],
            "{screen}"
        );
        for (index, row) in rows.iter().enumerate() {
            assert!(
                !rows[..index].contains(row),
                "two rows render identically:\n{screen}"
            );
        }
        let lone = rows
            .iter()
            .filter(|row| row[0] == "lone")
            .map(|row| row[if cols < 132 { 2 } else { 3 }].as_str())
            .collect::<Vec<_>>();
        assert_eq!(lone.len(), 2, "{screen}");
        assert!(
            lone.contains(&"missing") && lone.contains(&"cloned"),
            "{screen}"
        );
        if cols >= 132 {
            let path = |name: &str| {
                rows.iter()
                    .find(|row| row[0] == name)
                    .map(|row| row[1].clone())
                    .unwrap()
            };
            let (first, second) = (path("alpha (copy)"), path("alpha (copy 2)"));
            assert!(
                first.starts_with('…') && first.ends_with("directory-name/alpha"),
                "{screen}"
            );
            assert!(
                second.starts_with('…') && second.ends_with("/older/alpha"),
                "{screen}"
            );
            assert_eq!(path("alpha (copy 3)"), "~/repos/archive/alpha", "{screen}");
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_long_named_copies_and_cross_host_rows_distinct_at_80_columns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/a-long-repository-name\"\n",
    )
    .unwrap();
    for path in [
        "org/a-long-repository-name",
        "archive/a-long-repository-name",
        "backup/a-long-repository-name",
    ] {
        init_repo(
            &root.join(path),
            "git@github.com:org/a-long-repository-name.git",
            true,
        );
    }
    init_repo(
        &root.join("hub/shared-tooling"),
        "git@github.com:org/shared-tooling.git",
        true,
    );
    init_repo(
        &root.join("lab/shared-tooling"),
        "git@gitlab.com:org/shared-tooling.git",
        true,
    );

    let mut command = support::lager(&home, &config);
    command.arg("inventory").env("NO_COLOR", "1");
    let transcript = run_inventory_pty_with_size(
        &mut command,
        &["NORMAL · 5 rows", "(copy 2)", "absent: Scanning"],
        b"q",
        24,
        80,
    );
    let screen = screen_text(&transcript, 24, 80);
    let rows = inventory_rows(&screen);
    assert_eq!(rows.len(), 5, "{screen}");
    for (index, row) in rows.iter().enumerate() {
        assert!(
            !rows[..index].contains(row),
            "two rows render identically:\n{screen}"
        );
    }
    let names = rows.iter().map(|row| row[0].as_str()).collect::<Vec<_>>();
    let copies = |suffix: &str| {
        names
            .iter()
            .filter(|name| name.starts_with("a-lo") && name.ends_with(suffix))
            .count()
    };
    assert_eq!(
        (copies("name"), copies(" (copy)"), copies(" (copy 2)")),
        (1, 1, 1),
        "{screen}"
    );
    assert!(names.contains(&"github.c…-tooling"), "{screen}");
    assert!(names.contains(&"gitlab.c…-tooling"), "{screen}");
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
                vec!["Path and origin:", "selected: alpha"],
                b"jsbeta\rM".to_vec(),
            ),
            (
                vec![
                    "All actions",
                    "pair: repository i",
                    "pair: keybindings ?",
                    "MENU ·",
                ],
                b"i".to_vec(),
            ),
            (vec!["Inspect / beta"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "/ beta", "selected: beta"], b"M".to_vec()),
            (vec!["All actions", "pair: repository i"], b"s".to_vec()),
            (
                vec!["SEARCH ·", "/ beta", "selected: beta"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "/ beta", "selected: beta"], b"M".to_vec()),
            (vec!["All actions"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "/ beta", "selected: beta"], b"q".to_vec()),
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
            (vec!["alpha", "beta"], b"/gto/rgap".to_vec()),
            (
                vec!["SEARCH ·", "/ gto/rgap", "alpha", "excluded"],
                b"raRq".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ gto/rgapraRq", "No matching repositories"],
                b"\x7f\x7f\x7f\x7f".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ gto/rgap", "selected: alpha"],
                b"\r".to_vec(),
            ),
            (
                vec!["NORMAL ·", "/ gto/rgap", "selected: alpha"],
                b"/".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ gto/rgap", "selected: alpha"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "/ gto/rgap"], b"\x7f".to_vec()),
            (
                vec!["NORMAL · 3 rows", "/ Search repositories", "beta"],
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
            (vec!["No active operation"], b"/missing\r".to_vec()),
            (
                vec![
                    "NORMAL · 1 row",
                    "/ missing",
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
        .find(|row| row[4] == "github.com/org/checkout")
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
                (vec!["main", "clean", "No active operation"], b"R".to_vec()),
                (
                    vec![
                        if incomplete { "unreadable" } else { "missing" },
                        if incomplete {
                            "partial scan:"
                        } else {
                            "No active operation"
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
        let row = rows.iter().find(|row| row[0] == "checkout").unwrap();
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
        &["detached HEAD", "clean", "No active operation"],
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
                        vec![
                            "absent: injected probe failure",
                            "No active operation",
                            "clean",
                            "main",
                        ],
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
                (
                    vec!["absent: partial scan:", "No active operation"],
                    b"q".to_vec(),
                ),
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
            &["No active operation", "No repositories found"],
            b"q",
            24,
            132,
        );
        let plain = screen_text(&transcript, 24, 132);
        assert!(plain.contains("remote off"), "{plain}");
        assert!(
            screen_has_pair(&plain, "q quit") && screen_has_pair(&plain, "R refresh"),
            "{plain}"
        );
        assert!(screen_has_pair(&plain, "/ search"), "{plain}");
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

    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["7 rows", "No active operation"],
                b"/elsewhere\r\r".to_vec(),
            ),
            (
                vec!["Inspect / managed", "misplaced checkout; found:"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
        ],
        80,
        220,
        0,
        |action, _, frame| {
            if action == 0 {
                frames.push(frame.to_owned());
            }
        },
    );
    let plain = screen_text(&frames[0], 80, 220);

    for expected in [
        "github.com/group/managed",
        "explicit",
        "elsewhere/managed",
        "github.com/group/missing",
        "unknown",
        "github.com/group/sub/repo",
        "wildcard",
        "../relative-origin",
    ] {
        assert!(plain.contains(expected), "missing {expected}:\n{plain}");
    }
    let rows = inventory_rows(&plain);
    let missing = rows
        .iter()
        .find(|row| row[4] == "github.com/group/missing")
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert_ne!(
        missing[3], "missing",
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

        let transcript = run_inventory_pty_actions(
            support::lager(&home, &config).arg("inventory"),
            &[
                (
                    vec!["No active operation", "github.com/group/sub/repo"],
                    b"kk\r".to_vec(),
                ),
                (
                    vec![
                        "Inspect / repo",
                        "covered by github.com/group/*",
                        "excluded by github.com/group/sub/*",
                    ],
                    b"\x1b".to_vec(),
                ),
                (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
            ],
            24,
            220,
        );
        let rows = inventory_rows(&screen_text(&transcript, 24, 220));
        let row = rows
            .iter()
            .find(|row| row[0] == "repo" && row[1] == "~/repos/group/sub/repo")
            .expect("nested wildcard checkout row");
        assert_eq!(row[2], "wildcard");
        assert_eq!(row[3], "cloned");
        assert_eq!(row[5], "main");
        assert_eq!(row[6], "clean");
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
            (vec!["github.com/org/fresh"], b"q".to_vec()),
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
        plain.contains("github.com/org/fresh"),
        "refresh did not install the new config:\n{plain}"
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
            (vec!["github.com/org/stale", "main", "clean"], b"R".to_vec()),
            (vec!["stale: main", "stale: known clean"], b"q".to_vec()),
        ],
        24,
        160,
        |action| {
            if action == 0 {
                fs::write(&slow_marker, "slow").unwrap();
            }
        },
    );
    let plain = screen_text(&transcript, 24, 160);
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
        &[(vec!["Scanning local repositories"], b"q".to_vec())],
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

    let mut inspected = Vec::new();
    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["changed-target", "pending"], b" ".to_vec()),
            (vec!["changed-target", "stale observation"], b"\r".to_vec()),
            (
                vec!["Inspect /", "stale: target changed during observation"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
        ],
        24,
        132,
        0,
        |action, _, frame| {
            if action == 2 {
                inspected.push(frame.to_owned());
            }
            if action == 0 {
                wait_for_file(&marker);
                let old = temp.path().join("old-checkout");
                fs::rename(&target, &old).unwrap();
                init_repo(&target, "git@github.com:org/new-origin.git", true);
            }
        },
    );
    let inspect = screen_text(&inspected[0], 24, 132);
    assert!(
        inspect.contains("stale: target changed during observation")
            && !inspect.contains("github.com/org/old-origin"),
        "changed target remained forever pending instead of becoming stale:\n{inspect}"
    );
    let plain = screen_text(&transcript, 24, 132);
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

    let mut inspected = Vec::new();
    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["origin-changed", "pending"], b" ".to_vec()),
            (vec!["origin-changed", "stale observation"], b"\r".to_vec()),
            (
                vec!["Inspect /", "stale: target changed during observation"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
        ],
        24,
        132,
        0,
        |action, _, frame| {
            if action == 2 {
                inspected.push(frame.to_owned());
            }
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
    let inspect = screen_text(&inspected[0], 24, 132);
    assert!(
        !inspect.contains("github.com/org/old-origin"),
        "stale probe result was accepted after origin config changed:\n{inspect}"
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
        .find(|row| row[4] == "github.com/other/wrong")
        .expect("wrong-origin checkout row");
    assert_eq!(wrong_row[0], "other/wrong");
    assert_eq!(wrong_row[1], "~/repos/org/wrong");
    assert_eq!(wrong_row[2], "unregistered");
    let declared_wrong = rows
        .iter()
        .find(|row| row[4] == "github.com/org/wrong")
        .expect("declared wrong-origin row");
    assert_eq!(declared_wrong[3], "conflict");
    assert_eq!(declared_wrong[0], "org/wrong");
    let corrupt_row = rows
        .iter()
        .find(|row| row[0] == "org/corrupt" && row[4].starts_with("origin error:"))
        .unwrap_or_else(|| panic!("corrupt-origin checkout row; parsed rows: {rows:?}"));
    assert!(corrupt_row[4].starts_with("origin error:"));
    let unreadable_row = rows
        .iter()
        .find(|row| row[4] == "github.com/org/unreadable")
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
            "No active operation",
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
        "org/*",
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
    assert_row(
        "pattern",
        [
            "org/*".to_owned(),
            "—".to_owned(),
            "wildcard".to_owned(),
            "—".to_owned(),
            "github.com/org/*".to_owned(),
            "—".to_owned(),
            "—".to_owned(),
        ],
    );
    assert_row(
        "missing declaration",
        [
            "missing".to_owned(),
            "~/repos/org/missing".to_owned(),
            "explicit".to_owned(),
            "missing".to_owned(),
            "github.com/org/missing".to_owned(),
            "—".to_owned(),
            "—".to_owned(),
        ],
    );
    assert_row(
        "exact managed checkout",
        [
            "managed".to_owned(),
            "~/repos/org/managed".to_owned(),
            "explicit".to_owned(),
            "cloned".to_owned(),
            "github.com/org/managed".to_owned(),
            "main".to_owned(),
            "clean".to_owned(),
        ],
    );
    assert_row(
        "misplaced duplicate managed checkout",
        [
            "managed (copy)".to_owned(),
            "~/repos/elsewhere/managed".to_owned(),
            "explicit".to_owned(),
            "cloned".to_owned(),
            "github.com/org/managed".to_owned(),
            "main".to_owned(),
            "clean".to_owned(),
        ],
    );
    assert_row(
        "wildcard checkout",
        [
            "extra".to_owned(),
            "~/repos/org/extra".to_owned(),
            "wildcard".to_owned(),
            "cloned".to_owned(),
            "github.com/org/extra".to_owned(),
            "main".to_owned(),
            "1 modified".to_owned(),
        ],
    );
    assert_row(
        "excluded checkout",
        [
            "skip".to_owned(),
            "~/repos/org/skip".to_owned(),
            "excluded".to_owned(),
            "cloned".to_owned(),
            "github.com/org/skip".to_owned(),
            "main".to_owned(),
            "clean".to_owned(),
        ],
    );
    assert_row(
        "originless checkout",
        [
            "no-origin".to_owned(),
            "~/repos/local/no-origin".to_owned(),
            "unregistered".to_owned(),
            "cloned".to_owned(),
            "No origin".to_owned(),
            "main".to_owned(),
            "clean".to_owned(),
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
                    vec!["selected: repo-00", "Path and origin:"],
                    vec![b'j'; 29],
                ),
                (vec!["selected: repo-29"], b"\x1b[A".to_vec()),
                (vec!["selected: repo-28"], b"k".to_vec()),
                (vec!["selected: repo-27"], b"\x1b[B".to_vec()),
                (vec!["selected: repo-28"], b"q".to_vec()),
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
        let selected = highlighted_inventory_row(&transcript, 24, 80, "repo-28").unwrap();
        let adjacent = screen
            .lines()
            .find(|line| line.contains("repo-27"))
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
                    "selected: alpha",
                    "No active operation",
                ],
                b"j".to_vec(),
            ),
            (
                vec!["selected: beta", "registration", "changes"],
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
    let wide_header = wide
        .lines()
        .find(|line| line.trim_start().starts_with("repository"))
        .unwrap();
    assert_eq!(
        wide_header.split_whitespace().collect::<Vec<_>>(),
        [
            "repository",
            "path",
            "registration",
            "checkout",
            "origin",
            "branch",
            "changes"
        ],
        "{wide}"
    );
    assert!(
        highlighted_inventory_row(wide_frame, 24, 132, "alpha").is_some(),
        "{wide}"
    );
    assert!(
        highlighted_inventory_row(compact_frame, 24, 80, "beta").is_some(),
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
    let compact_header = compact
        .lines()
        .find(|line| line.trim_start().starts_with("repository"))
        .unwrap();
    assert_eq!(
        compact_header.split_whitespace().collect::<Vec<_>>(),
        [
            "repository",
            "registration",
            "checkout",
            "branch",
            "changes"
        ],
        "{compact}"
    );
    let selected = highlighted_inventory_row(compact_frame, 24, 80, "beta").unwrap();
    let adjacent = compact.lines().find(|line| line.contains("alpha")).unwrap();
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
            (vec!["alpha", "beta", "pending"], b"j".to_vec()),
            (vec!["selected: org/beta", "pending"], b"q".to_vec()),
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
                    "Path and origin:",
                ],
                b"R".to_vec(),
            ),
            (
                vec!["warning: unknown config key `other_key`"],
                b"q".to_vec(),
            ),
        ],
        24,
        120,
        |index| {
            if index == 0 {
                fs::write(&config, "root = \"repos\"\nother_key = \"changed\"\n").unwrap();
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
                vec!["selected: alpha", "pair: n/k rows", "pair: x refresh"],
                b"j?".to_vec(),
            ),
            (vec!["HELP", "n/k Up", "pair: x refresh"], b"\x1b".to_vec()),
            (vec!["NORMAL", "selected: alpha"], b"n".to_vec()),
            (vec!["selected: beta"], b"Rx".to_vec()),
            (vec!["pair: y refresh", "selected: beta"], b"y".to_vec()),
            (
                vec!["refresh failed:", "collision", "pair: y refresh"],
                b"y".to_vec(),
            ),
            (vec!["pair: z refresh", "selected: beta"], b"q".to_vec()),
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
    assert!(screen_has_pair(
        &screen_text(&transcript, 24, 100),
        "z refresh"
    ));
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
                vec!["NORMAL", "pair: x refresh", "alpha"],
                b"/alpha".to_vec(),
            ),
            (
                vec![
                    "SEARCH ·",
                    "/ alpha",
                    "pair: Ctrl+a accept",
                    "pair: Ctrl+e cancel",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ alpha", "selected: alpha"],
                b"\x01".to_vec(),
            ),
            (
                vec!["NORMAL ·", "/ alpha", "pair: x refresh"],
                b"x".to_vec(),
            ),
            (
                vec![
                    "refresh failed:",
                    "preserve ordinary input",
                    "NORMAL ·",
                    "/ alpha",
                    "pair: x refresh",
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
            (vec!["Path and origin:", "pair: ? help"], b"?".to_vec()),
            (
                vec!["Keyboard / effective bindings", "j/k ↑/↓", "absent: q/Esc"],
                b"x".to_vec(),
            ),
            (
                vec![
                    "Keyboard / effective bindings",
                    "? / q/Esc",
                    "absent: j/k ↑/↓",
                ],
                b"z".to_vec(),
            ),
            (
                vec!["Keyboard / effective bindings", "j/k ↑/↓", "absent: q/Esc"],
                b"x".to_vec(),
            ),
            (
                vec![
                    "Keyboard / effective bindings",
                    "? / q/Esc",
                    "absent: j/k ↑/↓",
                ],
                Vec::new(),
            ),
            (
                vec!["Keyboard / effective bindings", "j/k ↑/↓", "? / q/Esc"],
                vec![17],
            ),
        ],
        10,
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
    let scrolled = screen_text(scrolled_frame, 10, 100);
    assert!(scrolled.contains("? / q/Esc"), "{scrolled}");
    assert!(
        !scrolled.contains("j/k ↑/↓"),
        "custom page-down did not scroll help: {scrolled}"
    );

    let resized = screen_text(resized_frame, 20, 100);
    assert!(
        resized.contains("Keyboard / effective bindings"),
        "{resized}"
    );
    assert!(resized.contains("? / q/Esc"), "{resized}");
    for pair in ["Backspace clear", "m all"] {
        assert!(
            screen_has_pair(&resized, pair),
            "missing {pair}:\n{resized}"
        );
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
    let selected_pending = "selected: z-pending".to_owned();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["selected: zzz", "pending", "z-pending"], b"j".to_vec()),
            (vec![&selected_pending], b" ".to_vec()),
            (vec!["selected: aaa", "No active operation"], b"q".to_vec()),
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
            (
                vec!["No repositories found", "pair: x refresh"],
                b"g?".to_vec(),
            ),
            (vec!["HELP", "pair: Backspace close"], vec![127]),
            (vec!["NORMAL", "pair: x refresh"], b"q".to_vec()),
        ],
        24,
        100,
    );
    assert!(!screen_has_pair(
        &screen_text(&transcript, 24, 100),
        "g register"
    ));
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
    let tools = temp.path().join("tools");
    let hold = temp.path().join("hold");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\nif [ \"$2\" != {} ]; then while [ -e {} ]; do /bin/sleep 0.02; done; fi\nexec {} \"$@\"\n",
            shell_word(&root.join("alpha")),
            shell_word(&hold),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut frames = Vec::new();

    run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["No active operation", "3 rows"],
                b"kkj/cloned\r".to_vec(),
            ),
            (
                vec!["NORMAL · 3 rows", "/ cloned", "selected: same"],
                b"R".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "/ cloned", "github.com/org/alpha"],
                Vec::new(),
            ),
            (
                vec!["No active operation", "explicit", "selected: same"],
                b"R".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "/ cloned", "github.com/org/alpha"],
                Vec::new(),
            ),
            (
                vec![
                    "No active operation",
                    "unregistered",
                    "absent: explicit",
                    "selected: same",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        500,
        0,
        |index, _, frame| match index {
            1 => {
                fs::write(&hold, "hold").unwrap();
                fs::write(
                    &config,
                    "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/same\"\n",
                )
                .unwrap();
            }
            2 | 4 => fs::remove_file(&hold).unwrap(),
            3 => {
                frames.push(frame.to_owned());
                fs::write(&hold, "hold").unwrap();
                fs::write(&config, "root = \"repos\"\n").unwrap();
            }
            5 => frames.push(frame.to_owned()),
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
            screen.contains("NORMAL · 3 rows") && screen.contains("  / cloned"),
            "filter did not survive refresh: {screen}"
        );
        let highlighted = highlighted_inventory_row(frame, 24, 500, "same")
            .unwrap_or_else(|| panic!("{representation} row was not selected:\n{screen}"));
        assert!(highlighted.contains("~/repos/org/same"), "{highlighted}");
        assert!(
            !highlighted.contains("~/repos/other"),
            "duplicate path took selection: {highlighted}"
        );
    }

    let declared_rows = inventory_rows(&screen_text(declaration_frame, 24, 500));
    let checkout_rows = inventory_rows(&screen_text(checkout_frame, 24, 500));
    let row_position = |rows: &[Vec<String>], path: &std::path::Path| {
        rows.iter()
            .position(|row| row[1] == format!("~/{}", path.strip_prefix(&home).unwrap().display()))
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

#[cfg(unix)]
#[test]
fn inventory_terminal_equivalent_keys_share_reachability_and_collision_checks() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let base = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    for (binding, input) in [
        ("Ctrl+i", b'\t'),
        ("Ctrl+M", b'\r'),
        ("Ctrl+A", 1),
        ("Ctrl+]", 29),
        ("Ctrl+@", 0),
    ] {
        fs::write(
            &config,
            format!("{base}\n[inventory.keys.search]\naccept = [\"{binding}\"]\n"),
        )
        .unwrap();
        run_inventory_pty_actions(
            support::lager(&home, &config).arg("inventory"),
            &[
                (vec!["No active operation"], b"/alpha".to_vec()),
                (vec!["SEARCH ·", "/ alpha", binding], vec![input]),
                (vec!["NORMAL ·", "/ alpha"], b"q".to_vec()),
            ],
            24,
            132,
        );
    }
    for (settings, expected) in [
        ("[inventory.keys.normal]\nquit = [\"Ctrl+m\"]", "collision"),
        (
            "[inventory.keys.search]\naccept = [\"Ctrl+0\"]",
            "unsupported Ctrl binding",
        ),
        (
            "[inventory.keys.search]\naccept = [\"Ctrl+Enter\"]",
            "unsupported Ctrl binding",
        ),
        (
            "[inventory.keys.search]\naccept = [\"Ctrl+[\"]",
            "collision",
        ),
        (
            "[inventory.keys.search]\naccept = [\"Ctrl+?\"]",
            "preserve ordinary input",
        ),
    ] {
        fs::write(&config, format!("{base}\n{settings}\n")).unwrap();
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("inventory"),
            &[],
            24,
            80,
            1,
            |_| {},
        );
        assert!(transcript.contains(expected), "{transcript}");
        assert!(!transcript.contains("\x1b[?1049h"));
    }
}

#[cfg(unix)]
#[test]
fn inventory_interruptions_restore_the_controlling_terminal_and_reap_active_probes() {
    use std::os::fd::AsRawFd;
    for signal in [Some(libc::SIGINT), Some(libc::SIGTERM), None] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("repos");
        let config = temp.path().join("config.toml");
        let tools = temp.path().join("tools");
        let marker = temp.path().join("pids");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&tools).unwrap();
        fs::write(&config, "root = \"repos\"\n").unwrap();
        init_repo(&root.join("active"), "git@github.com:org/active.git", true);
        fs::write(tools.join("git"), format!(
            "#!/bin/sh\nif [ \"$3\" = status ]; then /bin/sleep 30 & printf '%s %s' \"$$\" \"$!\" > {}; wait; fi\nexec {} \"$@\"\n",
            shell_word(&marker), shell_word(&support::real_tool("git")),
        )).unwrap();
        fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let processes = FixtureProcesses(marker);
        let mut interrupted_at = None;
        let transcript = run_inventory_pty_with_exit_observing(
            support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
            &[(
                vec!["pending"],
                if signal.is_none() {
                    vec![3]
                } else {
                    Vec::new()
                },
            )],
            24,
            132,
            128 + signal.unwrap_or(libc::SIGINT),
            |_, terminal, _| {
                processes.assert_running();
                interrupted_at = Some(Instant::now());
                if let Some(signal) = signal {
                    // SAFETY: the PTY descriptor is live; the foreground PID belongs to this fixture.
                    unsafe {
                        let foreground = libc::tcgetpgrp(terminal.as_raw_fd());
                        assert!(foreground > 0);
                        assert_eq!(libc::kill(foreground, signal), 0);
                    }
                }
            },
        );
        processes.assert_reaped_by(interrupted_at.unwrap() + Duration::from_secs(2));
        assert!(transcript.contains("\x1b[?1049l"));
        assert!(transcript.contains("\x1b[?25h"));
    }
}

#[cfg(unix)]
#[test]
fn inventory_menu_refresh_uses_effective_binding_and_preserves_filter() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let settings = |key: &str| {
        format!(
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[inventory.keys.normal]\nrefresh = [\"{key}\"]\n"
        )
    };
    fs::write(&config, settings("x")).unwrap();
    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["No active operation"], b"/alpha\rm".to_vec()),
            (
                vec!["All actions", "pair: config x", "Refresh local / config"],
                b"jjjjjjjj\r".to_vec(),
            ),
            (
                vec!["NORMAL ·", "/ alpha", "pair: y refresh", "selected: alpha"],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"y".to_vec()),
            (
                vec!["NORMAL ·", "/ alpha", "pair: z refresh"],
                b"m".to_vec(),
            ),
            (vec!["All actions", "/ alpha"], vec![127]),
            (
                vec!["NORMAL ·", "absent: All actions", "/ Search repositories"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| match action {
            1 => fs::write(&config, settings("y")).unwrap(),
            3 => fs::write(&config, settings("z")).unwrap(),
            _ => {}
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_compact_hints_keep_actions_and_popup_exit_routes_visible() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec![
                    "Path and origin:",
                    "pair: Enter inspect",
                    "pair: m menu",
                    "pair: / search",
                    "pair: ? help",
                    "pair: q quit",
                ],
                b"m".to_vec(),
            ),
            (
                vec!["All actions", "j/k ↑/↓ select · Enter choose · Esc back"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Inspect / alpha",
                    "j/k ↑/↓ scroll · c copy a value… · Esc close",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "pair: Enter inspect", "pair: m menu"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_help_uses_the_invoking_mode_and_returns_to_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[inventory.keys.search]\naccept = [\"Ctrl+a\"]\ncancel = [\"Ctrl+e\"]\n").unwrap();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"/alpha\x1bOP".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Search mode",
                    "Ctrl+a / Ctrl+e",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["SEARCH ·", "/ alpha"], vec![5]),
            (vec!["NORMAL ·", "/ alpha"], b"m?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Menu mode",
                    "j/k ↑/↓ Tab/Shift+Tab",
                    "pair: R refresh",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["All actions"], b"j\r?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Inspect mode",
                    "j/k ↑/↓ Tab/Shift+Tab",
                    "PageUp/PageDown",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["Inspect / alpha"], vec![17]),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_inspect_explains_unavailable_actions_without_changing_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let repository = home.join("repos/org/alpha");
    fs::create_dir_all(&repository).unwrap();
    let settings = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, settings).unwrap();
    init_repo(&repository, "git@github.com:org/alpha.git", true);
    let head = fs::read(repository.join(".git/HEAD")).unwrap();
    let git_config = fs::read(repository.join(".git/config")).unwrap();
    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"\r".to_vec()),
            (vec!["Inspect / alpha"], vec![b'j'; 40]),
            (
                vec![
                    "pair: Register no-op",
                    "pair: Unregister available",
                    "pair: Clone unavailable",
                    "pair: Remove unavailable",
                    "read-only inventory",
                ],
                b"ar \x1b".to_vec(),
            ),
            (vec!["NORMAL", "selected: alpha"], b"q".to_vec()),
        ],
        24,
        80,
    );
    assert!(!screen_has_pair(
        &screen_text(&transcript, 24, 80),
        "Remove available"
    ));
    assert_eq!(fs::read_to_string(&config).unwrap(), settings);
    assert_eq!(fs::read(repository.join(".git/HEAD")).unwrap(), head);
    assert_eq!(
        fs::read(repository.join(".git/config")).unwrap(),
        git_config
    );
    assert!(repository.is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_inspect_keeps_wildcard_relationships_on_exact_declarations() {
    for cloned in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        let root = home.join("repos");
        fs::create_dir_all(&root).unwrap();
        fs::write(&config, "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"team/alpha\"]\n[[repositories]]\nurl = \"github.com/org/team/*\"\n[[repositories]]\nurl = \"github.com/org/team/alpha\"\n").unwrap();
        if cloned {
            init_repo(
                &root.join("org/team/alpha"),
                "git@github.com:org/team/alpha.git",
                true,
            );
        }
        run_inventory_pty_actions(
            support::lager(&home, &config).arg("inventory"),
            &[
                (vec!["Path and origin:"], b"\r".to_vec()),
                (
                    vec!["Inspect / alpha", "pair: Registration explicit"],
                    vec![b'j'; 60],
                ),
                (
                    vec![
                        "excluded by github.com/org/*",
                        "covered by github.com/org/team/*",
                    ],
                    b"\x1b".to_vec(),
                ),
                (vec!["NORMAL", "selected: alpha"], b"q".to_vec()),
            ],
            24,
            80,
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_menu_scrolls_with_visible_title_and_hints_after_resize() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"m".to_vec()),
            (
                vec!["All actions", "Inspect repository"],
                b"\x1b[6~\x1b[6~?".to_vec(),
            ),
            (vec!["Keyboard / effective bindings"], b"\x1b".to_vec()),
            (
                vec!["All actions", "pair: Enter choose", "pair: Esc back"],
                b"\x1b[5~\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: All actions"], b"q".to_vec()),
        ],
        12,
        80,
        0,
        |action, terminal, frame| {
            if action == 3 {
                frames.push(frame.to_owned());
                resize_pty(terminal, 24, 80);
            }
        },
    );
    let small = screen_text(&frames[0], 12, 80);
    let popup = small
        .lines()
        .skip(1)
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        screen_has_pair(&popup, "Quit Ctrl+q"),
        "menu did not scroll to its final item:\n{small}"
    );
    assert!(!popup.contains("Inspect repository"), "{small}");
}

#[cfg(unix)]
#[test]
fn inventory_search_and_inspection_remain_live_during_a_controlled_probe() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let marker = temp.path().join("started");
    let release = temp.path().join("release");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("hidden/zephyr-target"),
        "git@github.com:org/alpha.git",
        true,
    );
    fs::write(tools.join("git"), format!(
        "#!/bin/sh\nif [ \"$3\" = status ]; then printf started > {}; while [ ! -f {} ]; do /bin/sleep 0.02; done; fi\nexec {} \"$@\"\n",
        shell_word(&marker), shell_word(&release), shell_word(&support::real_tool("git")),
    )).unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (vec!["pending"], b"/zphrtrgt".to_vec()),
            (
                vec!["SEARCH ·", "/ zphrtrgt", "1 row", "pending"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "/ zphrtrgt"], b"\r".to_vec()),
            (vec!["INSPECT", "pair: Origin pending"], Vec::new()),
            (vec!["Inspect / unavailable"], b"\x1b".to_vec()),
            (
                vec!["Path and origin:", "NORMAL ·", "/ zphrtrgt"],
                b"\x7f/git@github.com:org/alpha.git\r".to_vec(),
            ),
            (
                vec!["NORMAL ·", "/ git@github.com:org/alpha.git", "1 row"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
        |action| {
            if action == 0 {
                wait_for_file(&marker);
            }
            if action == 1 {
                assert!(!release.exists());
            }
            if action == 3 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_dialog_overrides_refresh_atomically_and_keep_all_exit_routes_reachable() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let settings = |accept: &str, menu_accept: &str, refresh: &str| {
        format!(
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[inventory.keys.normal]\nrefresh = [\"{refresh}\"]\n[inventory.keys.menu]\naccept = [{menu_accept}]\ncancel = [\"Ctrl+e\"]\nhelp = [\"F1\"]\nquit = [\"Ctrl+q\"]\n[inventory.keys.inspection]\naccept = [\"{accept}\"]\ncancel = [\"Ctrl+e\"]\nhelp = [\"F1\"]\nquit = [\"Ctrl+q\"]\n"
        )
    };
    fs::write(&config, settings("Ctrl+b", "\"Ctrl+a\"", "R")).unwrap();
    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"m".to_vec()),
            (
                vec!["All actions", "pair: Ctrl+a choose", "pair: Ctrl+e back"],
                vec![b'j', 1],
            ),
            (vec!["Inspect / alpha", "pair: Ctrl+e close"], vec![2]),
            (vec!["NORMAL", "absent: INSPECT"], b"R".to_vec()),
            (vec!["NORMAL ·", "pair: y refresh"], b"m\x1bOP".to_vec()),
            (
                vec!["Keyboard / effective bindings", "pair: Ctrl+e close"],
                vec![4],
            ),
            (vec!["All actions", "absent: HELP"], vec![5]),
            (vec!["NORMAL", "absent: MENU"], b"\r".to_vec()),
            (
                vec!["Inspect / alpha", "pair: Ctrl+e close"],
                b"\x1bOP".to_vec(),
            ),
            (
                vec!["Keyboard / effective bindings", "pair: Ctrl+d close"],
                vec![4],
            ),
            (vec!["INSPECT", "absent: HELP"], vec![5]),
            (vec!["NORMAL", "absent: INSPECT"], b"y".to_vec()),
            (
                vec![
                    "refresh failed:",
                    "must remain reachable",
                    "pair: y refresh",
                ],
                b"m".to_vec(),
            ),
            (vec!["All actions", "pair: Ctrl+a choose"], vec![b'j', 1]),
            (vec!["INSPECT", "pair: Ctrl+e close"], b"\x1bOP".to_vec()),
            (
                vec!["Keyboard / effective bindings", "pair: Ctrl+d close"],
                vec![17],
            ),
        ],
        24,
        100,
        1,
        |action| match action {
            3 => fs::write(&config, settings("Ctrl+d", "\"Ctrl+a\"", "y")).unwrap(),
            11 => fs::write(&config, settings("Ctrl+b", "", "z")).unwrap(),
            _ => {}
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_inspect_keeps_originless_conflict_and_unreadable_facts_distinct() {
    for (case, fact, exit) in [
        ("originless", ("Origin", "No origin"), 0),
        ("conflict", ("Checkout", "conflict"), 0),
        ("unreadable", ("Checkout", "unreadable"), 1),
    ] {
        let fact = format!("{:<19}{}", fact.0, fact.1);
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        let repository = home.join("repos/org/alpha");
        fs::create_dir_all(&repository).unwrap();
        fs::write(
            &config,
            if case == "originless" {
                "root = \"repos\"\n"
            } else {
                "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n"
            },
        )
        .unwrap();
        match case {
            "originless" => init_repo_without_origin(&repository),
            "conflict" => init_repo(&repository, "git@github.com:org/different.git", true),
            _ => fs::set_permissions(&repository, fs::Permissions::from_mode(0o000)).unwrap(),
        }
        let ready = if case == "unreadable" {
            "partial scan:"
        } else {
            "Path and origin:"
        };
        run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("inventory"),
            &[
                (vec![ready], b"\r".to_vec()),
                (vec!["Inspect /", fact.as_str()], vec![17]),
            ],
            24,
            80,
            exit,
            |action| {
                if action == 1 && case == "unreadable" {
                    fs::set_permissions(&repository, fs::Permissions::from_mode(0o755)).unwrap();
                }
            },
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_no_color_applies_to_rows_and_popups() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config)
            .arg("inventory")
            .env("NO_COLOR", "1"),
        &[
            (
                vec!["Path and origin:", "explicit", "missing"],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"j\r".to_vec()),
            (vec!["Inspect / alpha"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], vec![17]),
        ],
        24,
        80,
    );
    for sequence in transcript.split("\x1b[").skip(1) {
        if let Some((parameters, _)) = sequence.split_once('m')
            && parameters
                .chars()
                .all(|character| character.is_ascii_digit() || character == ';')
        {
            assert!(
                !parameters
                    .split(';')
                    .filter_map(|code| code.parse::<u16>().ok())
                    .any(|code| matches!(code, 30..=38 | 40..=48 | 90..=97 | 100..=107)),
                "NO_COLOR emitted color SGR {parameters}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_help_wraps_long_effective_binding_lists() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, r#"root = "repos"
[inventory.keys.normal]
down = ["Ctrl+a", "Ctrl+b", "Ctrl+d", "Ctrl+e", "Ctrl+f", "Ctrl+g", "Ctrl+h", "Ctrl+j", "Ctrl+k", "Ctrl+l", "Ctrl+n", "Ctrl+o", "Ctrl+p", "Ctrl+q", "Ctrl+r", "Ctrl+s", "Ctrl+t", "Ctrl+u", "Ctrl+v", "Ctrl+w", "Ctrl+x", "Ctrl+y"]
"#).unwrap();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings", "Ctrl+y"], vec![17]),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_event_reader_preserves_input_ready_with_resize() {
    use crossterm::{event, terminal};

    let name = "inventory_event_reader_preserves_input_ready_with_resize";
    if let Some(release) = std::env::var_os("LAGER_TEST_EVENT_RELEASE") {
        terminal::enable_raw_mode().unwrap();
        assert!(!event::poll(Duration::from_millis(1)).unwrap());
        println!("\r\nREADY\x1b[0m\r");
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal::size().unwrap() != (80, 16) {
            assert!(Instant::now() < deadline, "resize was not delivered");
            std::thread::sleep(Duration::from_millis(1));
        }
        println!("\r\nRESIZED\x1b[0m\r");
        while !std::path::Path::new(&release).exists() {
            assert!(Instant::now() < deadline, "input was not released");
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut resized = false;
        let mut escaped = false;
        for _ in 0..2 {
            if event::poll(Duration::from_millis(200)).unwrap() {
                match event::read().unwrap() {
                    event::Event::Resize(80, 16) => resized = true,
                    event::Event::Key(key) if key.code == event::KeyCode::Esc => escaped = true,
                    _ => {}
                }
            }
        }
        terminal::disable_raw_mode().unwrap();
        assert!(
            resized && escaped,
            "lost ready event: resize={resized}, escape={escaped}"
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let release = temp.path().join("release");
    run_inventory_pty_with_exit_observing(
        Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture"])
            .env("LAGER_TEST_EVENT_RELEASE", &release),
        &[(vec!["READY"], vec![]), (vec!["RESIZED"], vec![])],
        24,
        80,
        0,
        |action, writer, _| match action {
            0 => resize_pty(writer, 16, 80),
            1 => {
                writer.write_all(b"\x1b").unwrap();
                writer.flush().unwrap();
                fs::write(&release, "ready").unwrap();
            }
            _ => unreachable!(),
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_popup_q_returns_to_the_previous_view() {
    let temp = tempfile::Builder::new().prefix("beta-").tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[[repositories]]\nurl = \"github.com/org/beta\"\n").unwrap();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (vec!["Path and origin:"], b"/github.com/org/beta\r".to_vec()),
            (
                vec!["NORMAL ·", "/ github.com/org/beta", "selected: beta"],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (vec!["All actions", "absent: HELP"], b"q".to_vec()),
            (
                vec![
                    "NORMAL ·",
                    "/ github.com/org/beta",
                    "selected: beta",
                    "absent: MENU",
                ],
                b"\r".to_vec(),
            ),
            (vec!["Inspect / beta"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (vec!["Inspect / beta", "absent: HELP"], b"q".to_vec()),
            (
                vec![
                    "NORMAL ·",
                    "/ github.com/org/beta",
                    "selected: beta",
                    "absent: INSPECT",
                ],
                b"?".to_vec(),
            ),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (
                vec![
                    "NORMAL ·",
                    "/ github.com/org/beta",
                    "selected: beta",
                    "absent: HELP",
                ],
                b"/\x1bOP".to_vec(),
            ),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (
                vec!["SEARCH ·", "/ github.com/org/beta", "absent: HELP"],
                b"q".to_vec(),
            ),
            (vec!["SEARCH ·", "/ github.com/org/betaq"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "/ github.com/org/betaq"], vec![127]),
            (
                vec!["NORMAL · 2 rows", "/ Search repositories"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_popup_quit_routes_and_explicit_overrides_remain_effective() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    for (overrides, quit) in [
        ("", vec![17]),
        (
            "[inventory.keys.menu]\ncancel = [\"Esc\"]\nquit = [\"q\"]\n[inventory.keys.inspection]\ncancel = [\"Esc\"]\nquit = [\"q\"]\n",
            b"q".to_vec(),
        ),
    ] {
        fs::write(
            &config,
            format!(
                "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n{overrides}"
            ),
        )
        .unwrap();
        for (open, title) in [
            (b"m", "All actions"),
            (b"\r", "Inspect / alpha"),
            (b"?", "Keyboard / effective bindings"),
        ] {
            let transcript = run_inventory_pty_actions(
                support::lager(&home, &config).arg("inventory"),
                &[
                    (vec!["Path and origin:"], open.to_vec()),
                    (vec![title], quit.clone()),
                ],
                24,
                80,
            );
            assert!(transcript.contains("\x1b[?1049l") && transcript.contains("\x1b[?25h"));
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_marks_survive_filtering_and_refresh_and_report_vanished_entries() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[[repositories]]\nurl = \"github.com/org/beta\"\n[[repositories]]\nurl = \"github.com/team/*\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo_without_origin(&root.join("scratch/gone"));

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 4 rows", "absent: Scanning", "selected: alpha"],
                b" ".to_vec(),
            ),
            (
                vec!["[x] alpha", "[ ] beta", "1 marked · 0 hidden"],
                b"jj ".to_vec(),
            ),
            (vec!["[x] gone", "2 marked · 0 hidden"], b"j ".to_vec()),
            (
                vec![
                    "team/* cannot be marked: patterns are inspect-only",
                    "2 marked · 0 hidden",
                ],
                b"/org/alpha\r".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "[x] alpha", "2 marked · 1 hidden"],
                b"R".to_vec(),
            ),
            (
                vec![
                    "absent: Scanning",
                    "Removed 1 mark: its entry vanished",
                    "1 marked · 0 hidden",
                    "[x] alpha",
                    "/ org/alpha",
                ],
                vec![127],
            ),
            (
                vec!["NORMAL · 3 rows", "absent: gone", "[x] alpha", "[ ] beta"],
                b"mjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Selection / 1 marked",
                    "Mark visible concrete rows",
                    "Clear all marks",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Marked visible concrete rows",
                    "[x] beta",
                    "2 marked · 0 hidden",
                ],
                b"mjjjjj\rjj\r".to_vec(),
            ),
            (
                vec!["Cleared 2 marks", "absent: marked ·", "absent: [x]"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 4 {
                fs::remove_dir_all(root.join("scratch")).unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_copy_value_sends_the_full_original_value_to_the_clipboard() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let checkout = root.join("archive\tcopy/alpha");
    init_repo(&checkout, "git@github.com:org/alpha.git", true);
    let branch = "feature/an-extremely-long-branch-name-that-the-cell-cuts";
    run_git(&checkout, &["checkout", "-qb", branch]);
    let tools = temp.path().join("tools");
    let copied = temp.path().join("copied");
    desktop_stubs(
        &tools,
        &["pbcopy", "wl-copy", "xclip", "xsel", "clip"],
        &format!("cat > {}", shell_word(&copied)),
    );

    let mut values = Vec::new();
    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: alpha"],
                b"mjj\r".to_vec(),
            ),
            (
                vec![
                    "Copy value / alpha",
                    "pair: Path ~/repos/archive\\tcopy/alpha",
                    "pair: Origin git@github.com:org/alpha.git",
                    "pair: Identity github.com/org/alpha",
                ],
                b"\r".to_vec(),
            ),
            (vec!["Copied path of alpha"], b"\r".to_vec()),
            (vec!["Inspect / alpha", "c copy a value…"], b"c".to_vec()),
            (vec!["Copy value / alpha"], b"j\r".to_vec()),
            (
                vec!["Copied origin of alpha", "Inspect / alpha"],
                b"c".to_vec(),
            ),
            (vec!["Copy value / alpha"], b"jj\r".to_vec()),
            (vec!["Copied identity of alpha"], b"c".to_vec()),
            (vec!["Copy value / alpha"], b"jjj\r".to_vec()),
            (vec!["Copied branch of alpha"], b"\x1b".to_vec()),
            (vec!["NORMAL", "absent: INSPECT"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action| {
            if matches!(action, 2 | 5 | 7 | 9) {
                values.push(fs::read(&copied).unwrap());
            }
        },
    );
    let path = fs::canonicalize(&checkout).unwrap();
    assert!(
        values[0] == checkout.as_os_str().as_encoded_bytes()
            || values[0] == path.as_os_str().as_encoded_bytes(),
        "{:?}",
        String::from_utf8_lossy(&values[0])
    );
    assert_eq!(
        values[1..],
        [
            b"git@github.com:org/alpha.git".to_vec(),
            b"github.com/org/alpha".to_vec(),
            branch.as_bytes().to_vec(),
        ]
    );

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: alpha"],
                b"mjj\r\r".to_vec(),
            ),
            (
                vec![
                    "Copy unavailable: no clipboard utility found",
                    "absent: Copied",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_open_origin_opens_only_a_derived_https_address() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    let marker = temp.path().join("executed");
    init_repo(
        &root.join("odd/tool"),
        &format!("ext::sh -c touch% {}", marker.display()),
        true,
    );
    let tools = temp.path().join("tools");
    let opened = temp.path().join("opened");
    desktop_stubs(
        &tools,
        &["open", "xdg-open"],
        &format!("printf '%s\\n' \"$@\" > {}", shell_word(&opened)),
    );

    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kkjmjjj\r".to_vec(),
            ),
            (
                vec!["Opening https://github.com/org/alpha in the browser"],
                b"kmjjj\r".to_vec(),
            ),
            (
                vec!["Open origin unavailable: the repository identity has characters"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |action| {
            if action == 1 {
                wait_for_file(&opened);
                assert_eq!(
                    fs::read_to_string(&opened).unwrap(),
                    "https://github.com/org/alpha\n"
                );
                fs::remove_file(&opened).unwrap();
            }
        },
    );
    assert!(!opened.exists(), "an unsupported origin reached the opener");
    assert!(!marker.exists(), "an origin scheme was executed");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kkjmjjj\r".to_vec(),
            ),
            (
                vec!["Open origin unavailable: no URL opener found"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_menu_reaches_every_entry_through_its_groups_and_submenus() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    let repository = |down: usize| format!("m\r{}\r", "j".repeat(down)).into_bytes();
    let root_entry = |down: usize| format!("m{}\r", "j".repeat(down)).into_bytes();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: alpha"],
                b"m".to_vec(),
            ),
            (
                vec![
                    "All actions",
                    "This row · alpha",
                    "pair: actions… clone,",
                    "pair: repository Enter",
                    "Copy value…",
                    "Open origin in browser",
                    "Worktrees…",
                    "Selection · 0 marked",
                    "pair: Selection… Space",
                    "Session",
                    "Add repository",
                    "pair: discovery… off",
                    "pair: config R",
                    "pair: problems… none",
                    "pair: batch no",
                    "pair: keybindings ?",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Repository actions / alpha",
                    "Register",
                    "Unregister",
                    "Clone only",
                    "Clone + register",
                    "Remove only",
                    "Remove + unregister",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Batch / Register", "pair: NO-OP alpha"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], repository(1)),
            (
                vec!["Batch / Unregister", "pair: SUCCEEDED alpha"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], repository(2)),
            (
                vec!["Clone only is unavailable in this build"],
                repository(3),
            ),
            (
                vec!["Clone + register is unavailable in this build"],
                repository(4),
            ),
            (
                vec!["Remove only is unavailable in this build"],
                repository(5),
            ),
            (
                vec!["Remove + unregister is unavailable in this build"],
                b"m\r\x1b".to_vec(),
            ),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"j\r".to_vec(),
            ),
            (vec!["Inspect / alpha"], b"\x1b".to_vec()),
            (vec!["NORMAL", "absent: Inspect /"], b"mjj\r".to_vec()),
            (
                vec!["Copy value / alpha", "Path", "Origin", "Identity", "Branch"],
                b"\x1b".to_vec(),
            ),
            (vec!["All actions", "absent: Copy value /"], b"j\r".to_vec()),
            (
                vec!["Open origin unavailable: no URL opener found"],
                root_entry(4),
            ),
            (
                vec!["Worktrees are unavailable in this build"],
                root_entry(5),
            ),
            (
                vec![
                    "Selection / 0 marked",
                    "Mark this row",
                    "Mark visible concrete rows",
                    "Clear all marks",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Marked alpha", "1 marked · 0 hidden"],
                b"mjjjjj\rjj\r".to_vec(),
            ),
            (
                vec!["Cleared 1 mark", "absent: marked ·"],
                b"mjjjjj\rj\r".to_vec(),
            ),
            (
                vec!["Marked visible concrete rows", "1 marked · 0 hidden"],
                root_entry(6),
            ),
            (
                vec!["Add repository is unavailable in this build"],
                root_entry(7),
            ),
            (
                vec!["Remote discovery is unavailable in this build"],
                root_entry(8),
            ),
            (
                vec![
                    "NORMAL",
                    "absent: All actions",
                    "absent: Remote discovery is unavailable",
                ],
                root_entry(9),
            ),
            (vec!["All actions", "pair: problems… none"], b"j\r".to_vec()),
            (vec!["All actions", "pair: batch no"], b"j\r".to_vec()),
            (
                vec!["Keyboard / effective bindings", "Menu mode"],
                b"\x1b".to_vec(),
            ),
            (vec!["All actions", "absent: Keyboard"], b"j\r".to_vec()),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_session_problems_list_every_failure_behind_exit_one() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    let problems = format!("m{}\r", "j".repeat(9)).into_bytes();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec![
                    "partial scan:",
                    "NORMAL · 1 row · 1 problem",
                    "absent: Scanning",
                ],
                b"m".to_vec(),
            ),
            (
                vec!["All actions", "pair: problems… 1"],
                problems[1..].to_vec(),
            ),
            (
                vec![
                    "PROBLEMS · 1 row · 1 problem",
                    "Session problems / 1",
                    "pair: scan ~/repos:",
                    "~/repos: no such file or directory ·",
                    "Quit returns 1 because problems occurred in this session.",
                    "Declined confirmations, cancellations and missing optional tools are not",
                    "j/k ↑/↓ scroll · Esc close",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row · 1 problem", "absent: Session problems /"],
                b"R".to_vec(),
            ),
            (
                vec!["refresh failed:", "NORMAL · 1 row · 2 problems"],
                problems.clone(),
            ),
            (
                vec![
                    "Session problems / 2",
                    "pair: scan ~/repos:",
                    "pair: refresh inventory",
                    "previous configuration kept",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Session problems /"], b"q".to_vec()),
        ],
        24,
        80,
        1,
        |action| {
            if action == 3 {
                fs::write(
                    &config,
                    "root = \"repos\"\n[inventory.keys.normal]\nquit = []\n",
                )
                .unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_keeps_drawing_while_the_opener_and_clipboard_hang() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let tools = temp.path().join("tools");
    desktop_stubs(
        &tools,
        &[
            "open", "xdg-open", "pbcopy", "wl-copy", "xclip", "xsel", "clip",
        ],
        "exec /bin/sleep 15",
    );

    let started = Instant::now();
    run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools)).arg("inventory"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kkmjjj\r".to_vec(),
            ),
            (
                vec!["Opening https://github.com/org/alpha in the browser"],
                b"j".to_vec(),
            ),
            (vec!["selected: beta"], b"mjj\r\r".to_vec()),
            (
                vec!["Copy unavailable:", "did not finish within 2 s"],
                b"k".to_vec(),
            ),
            (
                vec!["selected: alpha", "absent: Copy unavailable"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    // The clipboard gets 2 s in total, however many utilities it tries.
    assert!(started.elapsed() < Duration::from_secs(6));
}

#[cfg(unix)]
#[test]
fn inventory_registers_and_unregisters_the_current_row_without_touching_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", false);
    let tree = checkout_tree(&root);
    let mut configs = Vec::new();

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Scanning",
                    "selected: beta",
                    "unregistered",
                ],
                b"m\r".to_vec(),
            ),
            (
                vec!["Repository actions / beta", "Register", "Unregister"],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "1 / 1 processed",
                    "pair: SUCCEEDED beta",
                    "pair: beta updated",
                    "Register: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL",
                    "absent: Batch /",
                    "absent: Scanning",
                    "explicit",
                    "absent: unregistered",
                ],
                b"m\rj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Unregister",
                    "pair: SUCCEEDED beta",
                    "pair: beta updated",
                    "Unregister: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Scanning", "unregistered"],
                b"m\rj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Unregister",
                    "pair: NO-OP beta",
                    "Unregister: 1 no-op",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], b"q".to_vec()),
        ],
        32,
        132,
        |action| {
            if matches!(action, 2 | 4 | 6) {
                configs.push(fs::read_to_string(&config).unwrap());
            }
        },
    );

    let registered =
        "root = \"repos\"\n\n[[repositories]]\nurl = \"git@github.com:org/beta.git\"\n";
    assert_eq!(
        cli_config(
            &home,
            original,
            &["register", "git@github.com:org/beta.git"]
        ),
        registered
    );
    let unregistered = cli_config(&home, registered, &["unregister", "github.com/org/beta"]);
    assert_eq!(configs, [registered, &unregistered, &unregistered]);
    assert_eq!(fs::read_to_string(&config).unwrap(), unregistered);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_scan_leaves_a_stat_dirty_index_untouched() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let checkout = root.join("org/alpha");
    init_repo(&checkout, "git@github.com:org/alpha.git", true);
    // A README whose mtime no longer matches its index entry makes a plain `git status` rewrite the index.
    fs::File::options()
        .write(true)
        .open(checkout.join("README"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    let index = checkout.join(".git/index");
    let before = fs::metadata(&index).unwrap();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[(
            vec!["NORMAL · 1 row", "absent: Scanning", "clean"],
            b"q".to_vec(),
        )],
        24,
        100,
        0,
        |_| {},
    );

    let after = fs::metadata(&index).unwrap();
    assert_eq!(
        (after.ino(), after.mtime(), after.mtime_nsec()),
        (before.ino(), before.mtime(), before.mtime_nsec())
    );
    assert!(!checkout.join(".git/index.lock").exists());
}

#[cfg(unix)]
#[test]
fn inventory_registers_every_marked_row_including_hidden_ones_and_reports_each_outcome() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    init_repo_without_origin(&root.join("scratch/local"));
    let tree = checkout_tree(&root);
    let mut configs = Vec::new();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kkjj\r".to_vec(),
            ),
            (
                vec![
                    "Inspect / local",
                    "pair: Register unavailable",
                    "pair: Unregister unavailable",
                    "unavailable · no supported origin observed",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Inspect /", "selected: local"],
                b"kk j j /github.com/org/beta\r".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "3 marked · 2 hidden", "selected: beta"],
                b"m\r\r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM",
                    "Register / preview",
                    "3 exact targets · 2 hidden by filter",
                    "NO-OP alpha (hidden by filter)",
                    "~/repos/org/alpha · github.com/org/alpha · identity is already registered",
                    "ELIGIBLE beta",
                    "~/repos/org/beta · github.com/org/beta",
                    "BLOCKED local (hidden by filter)",
                    "~/repos/scratch/local · no identity · no supported origin observed",
                    "pair: Cancel default",
                    "Register 1 target",
                    "1 no-op · 1 blocked stay unchanged",
                    "Nothing changes until you confirm",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "3 / 3 processed · 2 hidden by filter",
                    "pair: NO-OP alpha",
                    "already registered",
                    "pair: SUCCEEDED beta",
                    "pair: beta updated",
                    "pair: BLOCKED local",
                    "Register: 1 succeeded, 1 no-op, 1 blocked",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                format!("m{}\r", "j".repeat(9)).into_bytes(),
            ),
            (
                vec![
                    "Session problems / 1",
                    "batch · local",
                    "Register blocked: no supported origin observed",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Session problems /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action| {
            if matches!(action, 4 | 5) {
                configs.push(fs::read_to_string(&config).unwrap());
            }
        },
    );

    let registered = cli_config(
        &home,
        original,
        &["register", "git@github.com:org/beta.git"],
    );
    assert_ne!(registered, original);
    assert_eq!(configs, [original, registered.as_str()]);
    assert_eq!(fs::read_to_string(&config).unwrap(), registered);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_registration_edits_wildcard_exclusions_and_keeps_patterns_inspect_only() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/team/*\"\nexclude = [\"gamma\"]\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("team/delta"),
        "git@github.com:team/delta.git",
        true,
    );
    init_repo(
        &root.join("team/gamma"),
        "git@github.com:team/gamma.git",
        true,
    );
    let tree = checkout_tree(&root);
    let mut configs = Vec::new();

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning", "excluded"],
                b"kkjm\r\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "pair: SUCCEEDED gamma",
                    "pair: gamma updated",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL",
                    "absent: Batch /",
                    "absent: Scanning",
                    "absent: excluded",
                    "selected: gamma",
                ],
                b"km\rj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Unregister",
                    "pair: SUCCEEDED delta",
                    "pair: delta updated",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Scanning", "excluded", "selected: delta"],
                b"jjm\r".to_vec(),
            ),
            (
                vec![
                    "Repository actions / team/*",
                    "pair: Register patterns",
                    "pair: Unregister patterns",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Repository actions / team/*", "absent: Batch /"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: All actions", "absent: Batch /"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if matches!(action, 1 | 3) {
                configs.push(fs::read_to_string(&config).unwrap());
            }
        },
    );

    let registered = cli_config(
        &home,
        original,
        &["register", "git@github.com:team/gamma.git"],
    );
    assert!(!registered.contains("gamma"), "{registered}");
    assert!(
        !registered.contains("[[repositories]]\nurl = \"git@"),
        "{registered}"
    );
    let unregistered = cli_config(&home, &registered, &["unregister", "github.com/team/delta"]);
    assert!(
        unregistered.contains("exclude = [\"delta\"]"),
        "{unregistered}"
    );
    assert_eq!(configs, [registered.as_str(), &unregistered]);
    assert_eq!(fs::read_to_string(&config).unwrap(), unregistered);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_cancelled_preview_changes_nothing_and_keeps_query_cursor_and_marks() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    for name in ["alpha1", "alpha2", "zeta"] {
        init_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
            true,
        );
    }
    let tree = checkout_tree(&root);
    let kept = |message: &'static str, selected: &'static str| {
        vec![
            "NORMAL",
            "absent: / preview",
            message,
            "NORMAL · 2 rows",
            "1 marked · 1 hidden",
            "/ github.com/org/alpha",
            selected,
            "absent: problem",
            "absent: Batch /",
        ]
    };

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kkjj /github.com/org/alpha\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "1 marked · 1 hidden", "selected: alpha2"],
                b"m\r\r".to_vec(),
            ),
            (
                vec![
                    "Register / preview",
                    "1 exact target · 1 hidden by filter",
                    "ELIGIBLE zeta (hidden by filter)",
                    "~/repos/org/zeta · github.com/org/zeta",
                    "Register 1 target",
                ],
                b"q".to_vec(),
            ),
            (
                kept("Register cancelled · nothing changed", "selected: alpha2"),
                b"km\rj\r".to_vec(),
            ),
            (
                vec![
                    "Unregister / preview",
                    "NO-OP zeta (hidden by filter)",
                    "identity is not registered",
                ],
                b"\r".to_vec(),
            ),
            (
                kept("Unregister cancelled · nothing changed", "selected: alpha1"),
                b"jm\r\r".to_vec(),
            ),
            (vec!["Register / preview"], b"j".to_vec()),
            (vec!["Register / preview"], b"\x1b".to_vec()),
            (
                kept("Register cancelled · nothing changed", "selected: alpha2"),
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_rechecks_each_target_before_its_write_and_blocks_a_vanished_one_as_stale() {
    recheck_blocks_a_vanished_checkout(false);
}

/// An exported repository with beta's origin must neither stand in for the deleted beta nor
/// make alpha read as changed.
#[cfg(unix)]
#[test]
fn inventory_recheck_ignores_an_exported_git_dir_and_work_tree() {
    recheck_blocks_a_vanished_checkout(true);
}

#[cfg(unix)]
fn recheck_blocks_a_vanished_checkout(exported_git: bool) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let mut command = support::lager(&home, &config);
    if exported_git {
        let decoy = temp.path().join("decoy");
        init_repo(&decoy, "git@github.com:org/beta.git", true);
        command
            .env("GIT_DIR", decoy.join(".git"))
            .env("GIT_WORK_TREE", &decoy);
    }

    run_inventory_pty_with_exit(
        command.arg("inventory"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kk j m\r\r".to_vec(),
            ),
            (
                vec!["Register / preview", "ELIGIBLE alpha", "ELIGIBLE beta"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "2 / 2 processed",
                    "pair: SUCCEEDED alpha",
                    "pair: BLOCKED beta",
                    "stale: checkout is gone",
                    "Register: 1 succeeded, 1 blocked",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action| {
            if action == 1 {
                // Only the repository goes when Git is exported, so the directory still answers.
                let beta = root.join("org/beta");
                fs::remove_dir_all(if exported_git {
                    beta.join(".git")
                } else {
                    beta
                })
                .unwrap();
            }
        },
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(
            &home,
            original,
            &["register", "git@github.com:org/alpha.git"]
        )
    );
}

#[cfg(unix)]
#[test]
fn inventory_blocks_a_previewed_no_op_whose_registration_changed_before_its_write() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    for name in ["alpha", "zeta"] {
        init_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
            true,
        );
    }
    let tree = checkout_tree(&root);
    let zeta = ["register", "git@github.com:org/zeta.git"];

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kk j m\rj\r".to_vec(),
            ),
            (
                vec![
                    "Unregister / preview",
                    "ELIGIBLE alpha",
                    "NO-OP zeta",
                    "identity is not registered",
                    "Unregister 1 target",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Unregister",
                    "2 / 2 processed",
                    "pair: SUCCEEDED alpha",
                    "pair: BLOCKED zeta",
                    "stale: registration changed after preview",
                    "Unregister: 1 succeeded, 1 blocked",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action| {
            if action == 1 {
                let result = support::lager(&home, &config).args(zeta).output().unwrap();
                assert!(result.status.success(), "{result:?}");
            }
        },
    );
    let registered = cli_config(&home, original, &zeta);
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(&home, &registered, &["unregister", "github.com/org/alpha"])
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_long_preview_scrolls_its_targets_while_the_choices_stay_in_view() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();
    for index in 0..12 {
        init_repo(
            &root.join(format!("org/r{index:02}")),
            &format!("git@github.com:org/r{index:02}.git"),
            true,
        );
    }
    let tree = checkout_tree(&root);
    let choices = ["pair: Cancel default", "Register 12 targets"];

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("inventory"),
        &[
            (
                vec!["NORMAL · 12 rows", "absent: Scanning"],
                b"mjjjjj\rj\r".to_vec(),
            ),
            (vec!["12 marked · 0 hidden"], b"m\r\r".to_vec()),
            (
                [
                    &[
                        "Register / preview",
                        "12 exact targets",
                        "ELIGIBLE r00",
                        "absent: ELIGIBLE r11",
                        "PageUp/PageDown scroll",
                    ][..],
                    &choices,
                ]
                .concat(),
                b"\x1b[6~".repeat(4),
            ),
            (
                [&["ELIGIBLE r11", "absent: ELIGIBLE r00"][..], &choices].concat(),
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "12 / 12 processed",
                    "pair: SUCCEEDED r11",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], b"q".to_vec()),
        ],
        24,
        80,
    );
    assert_eq!(
        fs::read_to_string(&config)
            .unwrap()
            .matches("[[repositories]]")
            .count(),
        12
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_failed_registration_write_is_one_problem_per_identity_and_keeps_columns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let directory = temp.path().join("etc");
    let config = directory.join("config.toml");
    fs::create_dir_all(&directory).unwrap();
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("archive/beta"),
        "git@github.com:org/beta.git",
        true,
    );
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let tree = checkout_tree(&root);
    // The lock lives in the test cache beside the config; only the config's own directory is read-only.
    fs::create_dir_all(directory.join("lager-test-cache")).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    let mut frames = Vec::new();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inventory_pty_with_exit_observing(
            support::lager(&home, &config).arg("inventory"),
            &[
                (
                    vec!["NORMAL · 2 rows", "absent: Scanning"],
                    b"kk j ".to_vec(),
                ),
                (vec!["2 marked · 0 hidden"], b"m\r\r".to_vec()),
                (
                    vec!["Register / preview", "ELIGIBLE beta (copy)"],
                    b"j\r".to_vec(),
                ),
                (
                    vec![
                        "Batch / Register",
                        "2 / 2 processed",
                        "pair: FAILED beta",
                        "write failed",
                        "could not write config",
                        "shared with beta",
                        "Register: 2 failed",
                        "· 1 problem",
                    ],
                    b"\x1b".to_vec(),
                ),
                (
                    vec!["NORMAL", "absent: Batch /", "· 1 problem"],
                    b"q".to_vec(),
                ),
            ],
            32,
            132,
            1,
            |action, _, frame| {
                if action == 3 {
                    frames.push(frame.to_owned());
                }
            },
        );
    }));
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }

    let [frame] = frames.as_slice() else {
        panic!("expected one outcomes frame");
    };
    let screen = screen_text(frame, 32, 132);
    let lines = screen
        .lines()
        .map(|line| line.chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let header = lines
        .iter()
        .position(|line| String::from_iter(line).contains("outcome  "))
        .expect("outcome header");
    let header_text = String::from_iter(&lines[header]);
    let outcome = header_text.find("outcome").unwrap();
    let reason = header_text[..header_text.find("reason").unwrap()]
        .chars()
        .count();
    let outcome = header_text[..outcome].chars().count();
    let rows = lines[header + 1..]
        .iter()
        .take_while(|line| {
            line.get(outcome..reason)
                .is_some_and(|cells| cells.iter().any(|c| *c != ' '))
                || line.get(reason).is_some_and(|c| *c != ' ')
        })
        .collect::<Vec<_>>();
    assert!(rows.len() > 2, "the long reason wraps:\n{screen}");
    let copy = rows
        .iter()
        .position(|line| String::from_iter(line.iter()).contains("beta (copy)"))
        .expect("the shared row");
    let copy_reason = rows[copy..]
        .iter()
        .enumerate()
        .take_while(|(offset, line)| {
            *offset == 0 || line[outcome..reason].iter().all(|c| *c == ' ')
        })
        .flat_map(|(_, line)| {
            line[reason..]
                .iter()
                .filter(|c| !c.is_whitespace() && **c != '│')
        })
        .collect::<String>();
    for part in ["couldnotwriteconfig", "sharedwithbeta"] {
        assert!(
            copy_reason.contains(part),
            "{part} missing from the shared row:\n{screen}"
        );
    }
    for line in rows {
        let cells = String::from_iter(&line[outcome..reason]);
        assert!(
            cells.trim().is_empty() || cells.starts_with("FAILED"),
            "reason left its column:\n{screen}"
        );
    }
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

/// The configuration the CLI produces from `before` for the same request.
#[cfg(unix)]
fn cli_config(home: &std::path::Path, before: &str, args: &[&str]) -> String {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    fs::write(&config, before).unwrap();
    let result = support::lager(home, &config).args(args).output().unwrap();
    assert!(result.status.success(), "{result:?}");
    fs::read_to_string(&config).unwrap()
}

/// Every file below `root` with its bytes, except the index `git status` may refresh.
#[cfg(unix)]
fn checkout_tree(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if !path.ends_with(".git/index") {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

#[cfg(unix)]
fn desktop_stubs(directory: &std::path::Path, names: &[&str], body: &str) {
    fs::create_dir_all(directory).unwrap();
    for name in names {
        let stub = directory.join(name);
        fs::write(&stub, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    }
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
    let writer = fs::File::from(pty.controller);
    let before = rustix_openpty::rustix::termios::tcgetattr(&writer).unwrap();
    let flags = rustix::fs::fcntl_getfl(&writer).unwrap();
    rustix::fs::fcntl_setfl(&writer, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
    let terminal = fs::File::from(pty.user);
    command
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.try_clone().unwrap()))
        .stderr(Stdio::from(terminal));
    // SAFETY: the child only calls async-signal-safe libc functions before exec.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = PtyChild {
        process: command.spawn().unwrap(),
        controller: Some(writer),
        abandoned: false,
    };
    let writer = child.controller.as_mut().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut transcript = String::new();
    let mut action_index = 0;
    loop {
        let size = rustix_openpty::rustix::termios::tcgetwinsize(&writer).unwrap();
        let (rows, cols) = (size.ws_row, size.ws_col);
        if Instant::now() >= deadline {
            let stuck = child
                .kill_and_reap()
                .err()
                .map(|stuck| format!("; {stuck}"))
                .unwrap_or_default();
            retain_transcript(&transcript, rows, cols);
            panic!(
                "inventory PTY timed out at action {action_index}{stuck}; transcript retained:\n{transcript}"
            );
        }
        read_pty(writer, &mut bytes);
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
                } else if let Some(pair) = text.strip_prefix("pair: ") {
                    screen_has_pair(
                        &screen_text(completed_frame, rows as usize, cols as usize),
                        pair,
                    )
                } else if let Some(absent) = text.strip_prefix("absent: ") {
                    !screen_text(completed_frame, rows as usize, cols as usize).contains(absent)
                } else {
                    screen_text(completed_frame, rows as usize, cols as usize).contains(text)
                }
            })
        {
            before_action(action_index, writer, completed_frame);
            writer.write_all(input).unwrap();
            writer.flush().unwrap();
            action_index += 1;
        }
        if let Some(status) = child.process.try_wait().unwrap() {
            let after = rustix_openpty::rustix::termios::tcgetattr(&writer).unwrap();
            retain_transcript(&transcript, rows, cols);
            assert_eq!(
                action_index,
                actions.len(),
                "child exited before all actions:\n{transcript}"
            );
            read_pty(writer, &mut bytes);
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
const PTY_REAP_GRACE: Duration = Duration::from_secs(2);

#[cfg(unix)]
struct PtyChild {
    process: std::process::Child,
    controller: Option<fs::File>,
    abandoned: bool,
}

#[cfg(unix)]
impl PtyChild {
    fn kill_and_reap(&mut self) -> Result<(), String> {
        let _ = self.process.kill();
        // macOS drains the controlling terminal while a killed session leader exits, so an
        // open, unread controller holds it in exit state E until the controller closes.
        self.controller = None;
        let reaped = self.reap_within(PTY_REAP_GRACE);
        self.abandoned = reaped.is_err();
        reaped
    }

    fn reap_within(&mut self, grace: Duration) -> Result<(), String> {
        let deadline = Instant::now() + grace;
        loop {
            if !matches!(self.process.try_wait(), Ok(None)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "PTY child pid {} did not exit within {grace:?} while being reaped",
                    self.process.id()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
impl Drop for PtyChild {
    fn drop(&mut self) {
        if self.abandoned {
            return;
        }
        if let Err(stuck) = self.kill_and_reap() {
            if std::thread::panicking() {
                eprintln!("{stuck}");
            } else {
                panic!("{stuck}");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn pty_reap_gives_up_on_a_stuck_child_and_names_it() {
    let mut child = PtyChild {
        process: Command::new("sleep").arg("30").spawn().unwrap(),
        controller: None,
        abandoned: false,
    };
    let pid = child.process.id();
    let started = Instant::now();
    let stuck = child.reap_within(Duration::from_millis(300)).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2), "{stuck}");
    assert!(stuck.contains(&format!("pid {pid} ")), "{stuck}");
    child.kill_and_reap().unwrap();
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

/// The mark slot before every repository name: unmarked, marked, or blank for patterns.
const MARK_SLOTS: [&str; 3] = ["[ ] ", "[x] ", "    "];

/// Splits table rows at the column header offsets; rows end at the first line whose
/// registration cell holds no registration label. The repository cell starts after the
/// mark slot.
#[cfg(unix)]
fn inventory_rows(screen: &str) -> Vec<Vec<String>> {
    let lines = screen.lines().collect::<Vec<_>>();
    let Some(header) = lines.iter().position(|line| {
        line.trim_start().starts_with("repository") && line.contains("registration")
    }) else {
        return Vec::new();
    };
    let mut offsets = Vec::new();
    let mut from = 0;
    for heading in lines[header].split_whitespace() {
        let start = lines[header][from..].find(heading).unwrap() + from;
        offsets.push(start);
        from = start + heading.len();
    }
    let registration = lines[header]
        .split_whitespace()
        .position(|heading| heading == "registration")
        .unwrap();
    let cell = |line: &str, column: usize| {
        let characters = line.chars().collect::<Vec<_>>();
        if column == 0 && characters.len() > 6 {
            let slot = characters[2..6].iter().collect::<String>();
            assert!(
                MARK_SLOTS.contains(&slot.as_str()),
                "mark slot {slot:?}: {line}"
            );
        }
        let start = if column == 0 { 6 } else { offsets[column] };
        let end = offsets.get(column + 1).copied().unwrap_or(characters.len());
        characters[start.min(characters.len())..end.min(characters.len())]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned()
    };
    lines[header + 1..]
        .iter()
        .take_while(|line| {
            [
                "explicit",
                "wildcard",
                "excluded",
                "unregistered",
                "pattern",
            ]
            .contains(&cell(line, registration).as_str())
        })
        .map(|line| {
            (0..offsets.len())
                .map(|column| cell(line, column))
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
            let fixed_mark_slot = start >= 4
                && MARK_SLOTS.contains(
                    &line[start - 4..start]
                        .iter()
                        .map(|(character, _)| *character)
                        .collect::<String>()
                        .as_str(),
                )
                && line[start - 4..start].iter().all(|(_, reversed)| *reversed);
            (selected && fixed_mark_slot).then(|| {
                line.into_iter()
                    .map(|(character, _)| character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
        })
}

/// Matches a key/label or field/value pair as adjacent words on one screen line, whatever
/// column padding separates them.
fn screen_has_pair(screen: &str, pair: &str) -> bool {
    let (first, second) = pair.rsplit_once(' ').expect("pair has two words");
    screen.lines().any(|line| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|words| words == [first, second])
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
