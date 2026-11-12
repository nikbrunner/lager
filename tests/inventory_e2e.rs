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
        support::lager(&home, &config).arg("tui"),
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
        command.arg("tui");
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
            &["  j/k rows  / search  a add  Enter inspect  R refresh  m menu  ? help  q quit"]
        } else {
            &[
                "  j/k rows  / search  Space mark  a add  r remove  Enter inspect  R refresh  m menu  ? help  q quit",
            ]
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
        command.arg("tui");
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
    command.arg("tui").env("NO_COLOR", "1");
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
fn tui_is_the_only_command_that_opens_the_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();

    let help = output(support::lager(&home, &config).args(["tui", "--help"]));
    assert!(help.status.success(), "{help:?}");
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage: lager tui"));

    for removed in ["inventory", "inv"] {
        let rejected = output(support::lager(&home, &config).arg(removed));
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert_eq!(rejected.status.code(), Some(2), "{removed}: {rejected:?}");
        assert!(
            stderr.contains("unrecognized subcommand"),
            "{removed}: {stderr}"
        );
        assert!(!stderr.contains("requires a TTY"), "{removed}: {stderr}");
        assert!(rejected.stdout.is_empty());
    }
}

#[test]
fn inventory_aliases_reject_invalid_non_tty_entry_without_provider_requests() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();

    {
        let command_name = "tui";
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
            support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
    {
        let name = "tui";
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
                support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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
    {
        let name = "tui";
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
        support::lager(&home, &config).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
    {
        let alias = "tui";
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
            .arg("tui")
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["selected: alpha", "pair: n/k rows", "pair: x refresh"],
                b"j?".to_vec(),
            ),
            (
                vec!["HELP", "n ↓ · k/Up ↑", "pair: x refresh"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "selected: alpha"], b"n".to_vec()),
            (vec!["selected: beta"], b"Rx".to_vec()),
            (vec!["pair: y refresh", "selected: beta"], b"y".to_vec()),
            (
                vec![
                    "! Config refresh failed: [inventory.keys.normal] binds j to down and to refresh",
                    "pair: y refresh",
                ],
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        12,
        100,
        0,
        |action, writer, frame| match action {
            4 => {
                frames.push(frame.to_owned());
                resize_pty(writer, 32, 100);
            }
            5 => frames.push(frame.to_owned()),
            _ => {}
        },
    );

    let [scrolled_frame, resized_frame] = frames.as_slice() else {
        panic!("expected a scrolled and resized help frame: {frames:?}");
    };
    let scrolled = screen_text(scrolled_frame, 12, 100);
    assert!(scrolled.contains("? / q/Esc"), "{scrolled}");
    assert!(
        !scrolled.contains("j/k ↑/↓"),
        "custom page-down did not scroll help: {scrolled}"
    );

    let resized = screen_text(resized_frame, 32, 100);
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        (
            "[inventory.keys.normal]\nrefresh = [\"j\"]",
            "[inventory.keys.normal] binds j to down and to refresh",
        ),
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
            "[inventory.keys.confirmation]\ndown = [\"Ctrl+q\"]\nhelp = [\"q\"]",
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
            support::lager(&home, &config).arg("tui"),
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
fn inventory_bindings_survive_symlinked_registration_and_show_direct_actions() {
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
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["No repositories found", "pair: x refresh"],
                b"g".to_vec(),
            ),
            (vec!["Register unavailable: no row"], b"?".to_vec()),
            (
                vec!["HELP", "pair: Backspace close", "pair: g Register"],
                vec![127],
            ),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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
        (
            "[inventory.keys.normal]\nquit = [\"Ctrl+m\"]",
            "[inventory.keys.normal] binds Ctrl+m to quit and to inspect",
        ),
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
            "[inventory.keys.search] binds Ctrl+[ to accept and to cancel",
        ),
        (
            "[inventory.keys.search]\naccept = [\"Ctrl+?\"]",
            "preserve ordinary input",
        ),
    ] {
        fs::write(&config, format!("{base}\n{settings}\n")).unwrap();
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("tui"),
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
            support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["Path and origin:"], b"\r".to_vec()),
            (vec!["Inspect / alpha"], vec![b'j'; 40]),
            (
                vec![
                    "pair: Register no-op",
                    "pair: Unregister available",
                    "pair: Clone no-op",
                    "pair: Remove available",
                    "removes the found path",
                ],
                b"ar \x1b".to_vec(),
            ),
            (vec!["NORMAL", "selected: alpha"], b"q".to_vec()),
        ],
        24,
        80,
    );
    // Typed inside Inspect, `r` is no action: no removal confirmation ever opened.
    assert!(!transcript.contains("Remove checkouts /"));
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
            support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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
            .arg("tui")
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
                support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        &format!("exec cat > {}", shell_word(&copied)),
    );

    let mut values = Vec::new();
    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
                    "pair: repository a",
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
            // Cloning needs the observed checkout: wait out the refresh that follows a batch.
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                repository(2),
            ),
            (
                vec!["Batch / Clone only", "pair: NO-OP alpha"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                repository(3),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "pair: SUCCEEDED alpha",
                    "pair: alpha updated",
                ],
                b"\x1b".to_vec(),
            ),
            // Removal needs the observed checkout too.
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                repository(4),
            ),
            (
                vec![
                    "Remove checkouts / keep registration",
                    "LOSS ACKNOWLEDGMENT REQUIRED alpha",
                    "no upstream",
                    // At 80 columns the exit routes stay in the popup and in the hint line.
                    "Space acknowledge · Enter choose · Esc cancel",
                    "Space acknowledge  Enter choose  Esc cancel",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "Remove only cancelled · nothing changed",
                    "absent: Remove checkouts /",
                ],
                repository(5),
            ),
            (vec!["Remove checkouts / unregister"], b"\x1b".to_vec()),
            (
                vec![
                    "Remove + unregister cancelled · nothing changed",
                    "absent: Remove checkouts /",
                ],
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
            (vec!["Add / URL or reference"], b"\x1b".to_vec()),
            (
                vec!["Add cancelled · nothing changed", "absent: Add /"],
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
fn inventory_direct_bindings_run_their_menu_actions_on_the_current_row() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let keys = "[inventory.keys.normal]\nregister = [\"g\"]\nunregister = [\"u\"]\n";
    fs::write(&config, format!("root = \"repos\"\n{keys}")).unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", false);
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "unregistered"],
                b"kk?".to_vec(),
            ),
            (
                vec![
                    "Keyboard / effective bindings",
                    "pair: g Register",
                    "pair: u Unregister",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"u".to_vec()),
            (
                vec![
                    "Batch / Unregister",
                    "pair: NO-OP beta",
                    "Unregister: 1 no-op",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"g".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "pair: SUCCEEDED beta",
                    "Register: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning", "explicit"],
                b"u".to_vec(),
            ),
            (
                vec![
                    "Batch / Unregister",
                    "pair: SUCCEEDED beta",
                    "Unregister: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL",
                    "absent: Batch /",
                    "absent: Scanning",
                    "unregistered",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        format!("root = \"repos\"\n{keys}")
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_direct_bindings_cover_marks_copy_open_and_unbuilt_remote_actions() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[inventory.keys.normal]\nregister = [\"g\"]\ncopy = [\"y\"]\n\
                    open = [\"o\"]\nremote = [\"e\"]\nretry = [\"t\"]\narchives = [\"v\"]\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    let tree = checkout_tree(&root);
    let tools = temp.path().join("tools");
    let opened = temp.path().join("opened");
    desktop_stubs(
        &tools,
        &["open", "xdg-open"],
        &format!("printf '%s\\n' \"$@\" > {}", shell_word(&opened)),
    );
    let unavailable = "Remote discovery is unavailable in this build";
    let cleared = vec!["NORMAL", "absent: Remote discovery is", "absent: problem"];

    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"kko".to_vec()),
            (
                vec!["Opening https://github.com/org/alpha in the browser"],
                b"y".to_vec(),
            ),
            // Copy opens the Copy value menu alone; cancelling it returns to the table.
            (vec!["Copy value / alpha", "Identity"], b"\x1b".to_vec()),
            (
                vec!["NORMAL", "absent: Copy value /", "absent: All actions"],
                b"e".to_vec(),
            ),
            (vec![unavailable, "absent: problem"], b"j".to_vec()),
            (cleared.clone(), b"t".to_vec()),
            (vec![unavailable, "absent: problem"], b"j".to_vec()),
            (cleared.clone(), b"v".to_vec()),
            (vec![unavailable, "absent: problem"], b" ".to_vec()),
            // With a mark the direct key previews, and Cancel is the default.
            (vec!["1 marked"], b"g".to_vec()),
            (
                vec!["Register / preview", "ELIGIBLE alpha", "Register 1 target"],
                b"\r".to_vec(),
            ),
            (
                vec!["Register cancelled · nothing changed", "absent: problem"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 1 {
                wait_for_file(&opened);
                assert_eq!(
                    fs::read_to_string(&opened).unwrap(),
                    "https://github.com/org/alpha\n"
                );
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

/// Issue #16 remap acceptance: in every mode the rebound key works, the default key it replaced
/// is dead, and the hints and Help show the new key. A dead key is sent together with a marker
/// key whose effect differs depending on whether the dead key acted.
#[cfg(unix)]
#[test]
fn inventory_remapped_keys_work_in_every_mode_and_their_defaults_are_dead() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n\
        [inventory.keys.normal]\nremove = [\"x\"]\nmenu = [\"M\"]\n\
        [inventory.keys.menu]\ncancel = [\"b\"]\n\
        [inventory.keys.search]\naccept = [\"Ctrl+a\"]\n\
        [inventory.keys.input]\naccept = [\"Ctrl+p\"]\n\
        [inventory.keys.confirmation]\ncancel = [\"n\"]\n\
        [inventory.keys.inspection]\ncancel = [\"h\"]\n";
    fs::write(&config, original).unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            // Normal: r and m are dead, so ? opens Help on the normal map.
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Scanning",
                    "pair: x remove",
                    "pair: M menu",
                ],
                b"kkrm?".to_vec(),
            ),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Normal mode",
                    "Remove checkout, keep registration (config: remove = [\"x\"])",
                    "all actions (config: menu = [\"M\"])",
                    "pair: x Remove",
                    "pair: M all",
                    "Other modes",
                    "pair: b back",
                    "search    Ctrl+a / Esc keep the filter",
                    "pair: Ctrl+p accept",
                    "pair: n cancel",
                    "pair: h close",
                ],
                // Inspection: q is dead, so ? closes Help instead of reopening it.
                b"q?".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"x".to_vec()),
            // Confirmation: q is dead, so ? opens Help on the confirmation map.
            (
                vec!["Remove checkouts / keep registration", "pair: n cancel"],
                b"q?".to_vec(),
            ),
            (
                vec![
                    "Confirmation mode",
                    "cancel (config: cancel = [\"n\"])",
                    "pair: M all",
                ],
                b"h".to_vec(),
            ),
            (
                vec!["Remove checkouts / keep registration", "absent: Keyboard /"],
                b"n".to_vec(),
            ),
            (
                vec!["Remove only cancelled · nothing changed"],
                b"M".to_vec(),
            ),
            // Menu: q is dead, so ? opens Help on the menu map.
            (vec!["All actions", "pair: b back"], b"q?".to_vec()),
            (
                vec!["Menu mode", "back (config: cancel = [\"b\"])"],
                b"h".to_vec(),
            ),
            (vec!["All actions", "absent: Keyboard /"], b"\r".to_vec()),
            (vec!["Repository actions / beta"], b"b".to_vec()),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"b".to_vec(),
            ),
            // Inspection: q is dead, so c opens the copy menu over Inspect.
            (vec!["NORMAL", "absent: All actions"], b"\r".to_vec()),
            (vec!["Inspect / beta", "pair: h close"], b"qc".to_vec()),
            (vec!["Copy value / beta", "pair: b back"], b"b".to_vec()),
            (
                vec!["Inspect / beta", "absent: Copy value /"],
                b"h".to_vec(),
            ),
            // Search: Enter is dead, so a is typed into the query.
            (vec!["NORMAL", "absent: Inspect /"], b"/bet".to_vec()),
            (vec!["SEARCH ·", "pair: Ctrl+a accept"], b"\ra".to_vec()),
            (vec!["SEARCH ·", "/ beta"], vec![1]),
            (vec!["NORMAL ·", "/ beta"], vec![127]),
            // Input: Enter is dead, so a is typed into the reference.
            (vec!["NORMAL ·", "absent: / beta"], b"a".to_vec()),
            (
                vec!["Add / URL or reference", "pair: Ctrl+p preview"],
                b"github.com/org/gamm\ra".to_vec(),
            ),
            (
                vec!["github.com/org/gamma", "absent: Add / preview"],
                vec![16],
            ),
            (vec!["Add / preview", "pair: n back"], b"n".to_vec()),
            (
                vec!["Add / URL or reference", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (vec!["Add cancelled · nothing changed"], b"q".to_vec()),
        ],
        32,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

/// Inspection `copy` and the confirmation Space and Ctrl-Q defaults give way to an explicit key
/// of the same mode. With Ctrl-Q taken, confirmation quit sits on `q`.
#[cfg(unix)]
#[test]
fn inventory_later_defaults_yield_and_confirmation_quit_falls_back_to_q() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n\
         [inventory.keys.inspection]\nup = [\"c\"]\n\
         [inventory.keys.confirmation]\ndown = [\"Ctrl+q\"]\naccept = [\"Space\"]\n",
    )
    .unwrap();
    for name in ["alpha", "beta"] {
        tracked_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
        );
    }

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kk\r".to_vec(),
            ),
            // c moves up in Inspect instead of opening the copy menu.
            (vec!["Inspect /", "absent: c copy"], b"c?".to_vec()),
            (
                vec![
                    "Inspect mode",
                    "absent: Copy value /",
                    "No key in inspection mode: copy.",
                    "No key in confirmation mode: acknowledge.",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["Inspect /", "absent: Keyboard /"], b"\x1b".to_vec()),
            (vec!["NORMAL", "absent: Inspect /"], b"r".to_vec()),
            (vec!["Remove checkouts / keep registration"], b"?".to_vec()),
            (
                vec![
                    "Confirmation mode",
                    "Ctrl+q ↓ · k/Up/Shift+Tab ↑ move focus",
                    "pair: Space choose",
                    "? / q",
                ],
                b"\x1b".to_vec(),
            ),
            // Ctrl-Q moves focus from Cancel to Remove and Space accepts it.
            (
                vec!["Remove checkouts / keep registration", "absent: Keyboard /"],
                vec![17, b' '],
            ),
            (
                vec![
                    "Remove only: 1 succeeded",
                    "RESULTS · 1 row",
                    "absent: Scanning",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Batch /"], b"r".to_vec()),
            // q quits from a confirmation.
            (vec!["Remove checkouts / keep registration"], b"q".to_vec()),
        ],
        32,
        132,
    );
    let left = ["alpha", "beta"]
        .into_iter()
        .filter(|name| root.join("org").join(name).exists())
        .count();
    assert_eq!(left, 1);
}

/// Configured key lists show which keys go which way, and an emptied default action is named
/// as keyless rather than as an action without a default.
#[cfg(unix)]
#[test]
fn inventory_help_shows_configured_directions_and_emptied_actions() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n[inventory.keys.normal]\nmark = []\n\
         [inventory.keys.inspection]\ndown = [\"c\", \"j\"]\n",
    )
    .unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"kk\r".to_vec()),
            (vec!["Inspect / beta"], b"?".to_vec()),
            (
                vec![
                    "Inspect mode",
                    "c/j ↓ · k/Up/Shift+Tab ↑",
                    "scroll (config: down = [\"c\", \"j\"])",
                    "No key in normal mode: mark.",
                    "No default key: Register,",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Inspect / beta", "absent: Keyboard /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Inspect /"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

/// Help is drawn above every status line, so a third one never hides the failure notice.
#[cfg(unix)]
#[test]
fn inventory_help_keeps_the_refresh_failure_visible_above_three_status_lines() {
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

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["ignored repositories:", "absent: Scanning"],
                b"R".to_vec(),
            ),
            (
                vec!["! Config refresh failed:", "ignored repositories:"],
                b"?".to_vec(),
            ),
            (
                vec![
                    "Keyboard / effective bindings",
                    "! Config refresh failed:",
                    "ignored repositories:",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"q".to_vec()),
        ],
        24,
        80,
        1,
        |action| {
            if action == 0 {
                fs::write(
                    &config,
                    "root = \"repos\"\n[inventory.keys.normal]\nremove = [\"x\"]\nclone = [\"x\"]\n",
                )
                .unwrap();
            }
        },
    );
}

/// Confirmation cancel keeps Esc when its `q` goes to an explicit binding.
#[cfg(unix)]
#[test]
fn inventory_confirmation_cancel_q_yields_to_an_explicit_help_key() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n[inventory.keys.confirmation]\nhelp = [\"q\"]\n",
    )
    .unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"kkr".to_vec()),
            (vec!["Remove checkouts / keep registration"], b"q".to_vec()),
            (
                vec![
                    "Confirmation mode",
                    "q / Ctrl+q",
                    "help / quit (config: help = [\"q\"])",
                    "pair: Esc cancel",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove checkouts / keep registration", "absent: Keyboard /"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove only cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_help_scrolls_to_every_mode_and_the_keyless_actions_at_80_columns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["No repositories found.", "Path and origin:"],
                b"?".to_vec(),
            ),
            (
                vec![
                    "Normal mode",
                    "absent: No default key:",
                    "No active operation",
                ],
                b"\x1b[6~".to_vec(),
            ),
            (
                vec![
                    "Other modes",
                    "pair: confirm j/k",
                    "No default key: Register, Unregister",
                    "absent: No key in",
                    "No active operation",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"q".to_vec()),
        ],
        24,
        80,
    );
}

/// The Help board: a refresh that binds one key to two actions keeps the previous map, says so
/// on the status line and counts as a session problem.
#[cfg(unix)]
#[test]
fn inventory_invalid_binding_refresh_keeps_the_previous_map_and_is_a_session_problem() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let valid = "root = \"repos\"\n[inventory.keys.normal]\nremove = [\"x\"]\n";
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&root.join("org/beta"), "git@github.com:org/beta.git", true);

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "pair: r remove"],
                b"R".to_vec(),
            ),
            // The valid map is installed whole: x removes and r is dead.
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "pair: x remove"],
                b"kkr?".to_vec(),
            ),
            (
                vec!["Normal mode", "(config: remove = [\"x\"])"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"R".to_vec()),
            (
                vec![
                    "! Config refresh failed: [inventory.keys.normal] binds x to remove and to clone · previous bindings stay active",
                    "NORMAL · 1 row · 1 problem",
                    "pair: x remove",
                ],
                b"x".to_vec(),
            ),
            (
                vec!["Remove checkouts / keep registration"],
                b"\x1b".to_vec(),
            ),
            (vec!["Remove only cancelled"], b"m".to_vec()),
            (
                vec!["All actions", "pair: 1 problem"],
                b"jjjjjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Session problems /",
                    "refresh         [inventory.keys.normal] binds x to remove and to clone",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Session problems /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action| match action {
            0 => fs::write(&config, valid).unwrap(),
            3 => fs::write(&config, format!("{valid}clone = [\"x\"]\n")).unwrap(),
            _ => {}
        },
    );
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
fn inventory_add_rejects_wildcards_and_credentials_without_showing_them() {
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
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"a".to_vec()),
            (
                vec![
                    "INPUT · 1 row",
                    "Add / URL or reference",
                    "Repository reference",
                    "Enter preview · Esc back",
                    "absent: marked row",
                ],
                b"acme/*\r".to_vec(),
            ),
            (
                vec![
                    "Rejected: wildcards are not accepted",
                    "acme/*█",
                    "absent: Add / preview",
                ],
                [vec![0x7f; 6], b"org/*/repo\r".to_vec()].concat(),
            ),
            (
                vec![
                    "Rejected: wildcards are not accepted",
                    "org/*/repo█",
                    "absent: acme/*",
                    "absent: Add / preview",
                ],
                [
                    vec![0x7f; 10],
                    b"https://user:hunter2@github.com/org/beta.git\r".to_vec(),
                ]
                .concat(),
            ),
            (
                vec![
                    "Rejected: repository reference is malformed",
                    "absent: hunter2",
                    "absent: user:",
                    "absent: org/*/repo",
                    "absent: Add / preview",
                ],
                b"org/beta?token=s3cr3t".to_vec(),
            ),
            (
                vec!["org/beta?token=s3cr3t", "absent: Rejected:"],
                b"\r".to_vec(),
            ),
            (
                vec!["Rejected:", "absent: s3cr3t", "absent: token"],
                b"git@github.com:org/beta.git#frag".to_vec(),
            ),
            (
                vec!["git@github.com:org/beta.git#frag", "absent: Rejected:"],
                b"\r".to_vec(),
            ),
            (
                vec!["Rejected:", "absent: #frag", "absent: Add / preview"],
                b"https://user:secret-token-123@github.com/org/*".to_vec(),
            ),
            (
                vec![
                    "https://user:secret-token-123@github.com/org/*",
                    "absent: Rejected:",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Rejected: repository reference is malformed",
                    "absent: secret-token-123",
                    "absent: user",
                    "absent: Add / preview",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL",
                    "Add cancelled · nothing changed",
                    "absent: Add /",
                    "absent: problem",
                ],
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
fn inventory_cancelled_add_changes_nothing_and_keeps_query_cursor_and_marks() {
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
    let kept = vec![
        "NORMAL · 2 rows",
        "Add cancelled · nothing changed",
        "absent: Add /",
        "1 marked · 1 hidden",
        "/ github.com/org/alpha",
        "selected: alpha2",
        "absent: problem",
        "absent: Batch /",
    ];

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kkjj /github.com/org/alpha\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "1 marked · 1 hidden", "selected: alpha2"],
                b"a".to_vec(),
            ),
            (
                vec![
                    "Add / URL or reference",
                    "1 marked row is not a target of Add.",
                ],
                b"qam/new\r".to_vec(),
            ),
            (
                vec![
                    "Add / preview",
                    "pair: Reference qam/new",
                    "pair: Identity github.com/qam/new",
                    "~/repos/qam/new",
                    "pair: Checkout missing",
                    "pair: Registration unregistered",
                    "pair: hook none",
                    "1 · unaffected by this action",
                    "Register only",
                    "Clone only",
                    "Clone + register",
                    "Back to reference",
                    "j/k ↑/↓ select · Enter choose · Esc back",
                ],
                b"kkkkjjj\r".to_vec(),
            ),
            (
                vec!["Add / URL or reference", "qam/new", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (kept.clone(), b"mjjjjjj\r".to_vec()),
            (
                vec!["Add / URL or reference", "absent: qam/new"],
                b"x/y\r".to_vec(),
            ),
            (vec!["Add / preview", "pair: Reference x/y"], b"q".to_vec()),
            (
                vec!["Add / URL or reference", "x/y", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (kept, b"q".to_vec()),
        ],
        32,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_add_never_offers_an_unknown_destination_as_missing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(root.join("org/plain")).unwrap();
    fs::write(root.join("org/plain/notes"), "kept\n").unwrap();
    fs::create_dir_all(temp.path().join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("elsewhere"), root.join("org/link")).unwrap();
    init_repo(
        &root.join("org/other"),
        "git@github.com:org/elsewhere.git",
        true,
    );
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:org/alpha.git",
        true,
    );
    let tree = checkout_tree(&root);
    let mut actions = vec![(vec!["NORMAL · 2 rows", "absent: Scanning"], b"a".to_vec())];
    for (reference, checkout, clone_only, clone_register) in [
        (
            "org/plain",
            "unknown · no checkout observed here",
            "Clone only unavailable: the destination exists and holds no observed checkout",
            "Clone + register unavailable: the destination exists and holds no observed checkout",
        ),
        (
            "org/link",
            "pair: Checkout unreadable",
            "Clone only unavailable: destination is unreadable",
            "Clone + register unavailable: destination is unreadable",
        ),
        (
            "org/other",
            "pair: Checkout conflict",
            "Clone only unavailable: destination conflicts with repository",
            "Clone + register unavailable: destination conflicts with repository",
        ),
        // The controls: both clone choices are offered here, so they are not pressed.
        ("org/alpha", "pair: Checkout cloned", "", ""),
        ("org/fresh", "pair: Checkout missing", "", ""),
    ] {
        actions.push((
            vec!["Add / URL or reference"],
            format!("{reference}\r").into_bytes(),
        ));
        if clone_only.is_empty() {
            actions.push((
                vec![
                    "Add / preview",
                    checkout,
                    "absent: destination conflicts",
                    "absent: destination is unreadable",
                    "absent: holds no observed checkout",
                ],
                b"\x1b".to_vec(),
            ));
        } else {
            actions.extend([
                (vec!["Add / preview", checkout], b"kkkkj\r".to_vec()),
                (vec!["Add / preview", clone_only], b"j\r".to_vec()),
                (vec!["Add / preview", clone_register], b"\x1b".to_vec()),
            ]);
        }
        actions.extend([
            (
                vec!["Add / URL or reference", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "Add cancelled · nothing changed"],
                b"a".to_vec(),
            ),
        ]);
    }
    actions.pop();
    actions.push((
        vec![
            "NORMAL",
            "Add cancelled · nothing changed",
            "absent: problem",
        ],
        b"q".to_vec(),
    ));
    run_inventory_pty_actions(support::lager(&home, &config).arg("tui"), &actions, 32, 132);
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_add_stays_usable_while_a_probe_is_pending_and_keeps_its_state_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    init_repo(
        &root.join("org/arrives"),
        "git@github.com:org/arrives.git",
        true,
    );
    let tree = checkout_tree(&root);
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n/bin/sleep 6\nexec {} \"$@\"\n",
            shell_word(support::real_tool("git").as_path())
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["Scanning local repositories", "arrives", "pending"],
                b"a".to_vec(),
            ),
            (vec!["Add / URL or reference"], b"org/arrives\r".to_vec()),
            (
                vec![
                    "Add / preview",
                    "pair: Checkout unknown",
                    "unknown · not observed yet",
                    "pending",
                    "checkout state is unknown until it is observed",
                ],
                b"kkkkj\r".to_vec(),
            ),
            (
                vec![
                    "Clone only unavailable: checkout state is unknown until it is observed",
                    "pending",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["Add / URL or reference"], b"\x1b".to_vec()),
            (
                vec!["NORMAL", "Add cancelled · nothing changed", "pending"],
                b"q".to_vec(),
            ),
        ],
        40,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_add_registers_a_typed_reference_through_the_batch_and_keeps_marks() {
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
    let tree = checkout_tree(&root);
    let reference = "https://github.com/org/beta.git";

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"kk a".to_vec()),
            (
                vec![
                    "Add / URL or reference",
                    "1 marked row is not a target of Add.",
                ],
                format!("{reference}\r").into_bytes(),
            ),
            (
                vec![
                    "Add / preview",
                    "pair: Identity github.com/org/beta",
                    "pair: Checkout missing",
                    "pair: Registration unregistered",
                    "pair: only configuration",
                ],
                b"kkkk\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "1 / 1 processed",
                    "pair: SUCCEEDED github.com/org/beta",
                    "Register: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 2 rows",
                    "absent: Batch /",
                    "absent: Add /",
                    "1 marked · 0 hidden",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                assert_eq!(fs::read_to_string(&config).unwrap(), original);
            }
        },
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(&home, original, &["register", reference])
    );
    assert_eq!(checkout_tree(&root), tree);
}

/// A Git wrapper that asks for a line on the terminal before every clone, records the answer,
/// then runs the real Git. The reset after the prompt ends a frame for the PTY reader.
#[cfg(unix)]
fn prompting_git(tools: &std::path::Path, answers: &std::path::Path) {
    fs::create_dir_all(tools).unwrap();
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = clone ]; then\n  printf 'Passphrase for %s: \\033[0m' \"$(basename \"$2\")\" > /dev/tty\n  IFS= read -r answer < /dev/tty\n  printf '%s\\n' \"$answer\" >> {}\nfi\nexec {} \"$@\"\n",
            shell_word(answers),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
}

/// A local repository with one commit and the `file://` reference that clones it.
#[cfg(unix)]
fn clone_source(directory: &std::path::Path, name: &str) -> String {
    let source = directory.join("remotes").join(name);
    init_repo(&source, "git@github.com:org/unused.git", true);
    format!("file://{}", source.display())
}

#[cfg(unix)]
fn cooked(terminal: &fs::File) -> bool {
    use rustix_openpty::rustix::termios::LocalModes;
    rustix_openpty::rustix::termios::tcgetattr(terminal)
        .unwrap()
        .local_modes
        .contains(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG)
}

/// The last thing the session did to the screen: it left the alternate screen and showed the
/// cursor.
#[cfg(unix)]
fn assert_screen_left(transcript: &str) {
    let last = |sequence: &str| transcript.rfind(sequence);
    assert!(
        last("\x1b[?1049l") > last("\x1b[?1049h"),
        "alternate screen was not left:\n{transcript}"
    );
    assert!(
        last("\x1b[?25h") > last("\x1b[?25l"),
        "cursor was not shown:\n{transcript}"
    );
}

const HANDOFF_NOTICE: &str =
    "lager: Git owns this terminal until it returns. Ctrl-C interrupts the batch and exits 130.";

#[cfg(unix)]
#[test]
fn inventory_clone_register_hands_the_terminal_to_git_and_returns_with_the_new_state() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();
    prompting_git(&tools, &answers);
    let reference = clone_source(temp.path(), "beta");
    let mut modes = Vec::new();

    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["NORMAL · 0 rows", "absent: Scanning"], b"a".to_vec()),
            (
                vec!["Add / URL or reference"],
                format!("{reference}\r").into_bytes(),
            ),
            (
                vec![
                    "Add / preview",
                    "~/repos/beta",
                    "pair: Checkout missing",
                    "pair: Registration unregistered",
                ],
                // Typed ahead of the handoff: it must not answer Git's prompt.
                b"kkkkjj\rleak\r".to_vec(),
            ),
            // The stub reads one line. The `q` it leaves unread must not act on the inventory.
            (vec!["Passphrase for beta:"], b"quit q a\rq".to_vec()),
            (
                vec![
                    "Batch / Clone + register",
                    "1 / 1 processed",
                    "SUCCEEDED",
                    "pair: cloned updated",
                    "Clone + register: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            // Had `q` closed the popup, this Esc would have quit and the menu would never open.
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Batch /",
                    "absent: Scanning",
                    "pair: explicit cloned",
                    "absent: problem",
                ],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"\x1b".to_vec()),
            (vec!["NORMAL · 1 row", "absent: All actions"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action, terminal, _| {
            if matches!(action, 2..=4) {
                modes.push(cooked(terminal));
            }
            if action == 3 {
                assert_eq!(fs::read_to_string(&config).unwrap(), original);
            }
        },
    );

    assert_eq!(modes, [false, true, false], "{transcript}");
    assert_eq!(fs::read_to_string(&answers).unwrap(), "quit q a\n");
    assert!(root.join("beta/.git").is_dir());
    let other = temp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(&other, original, &["add", &reference, "--register"])
    );
    let handoff = transcript.find("lager: clone 1 of 1 · ").unwrap();
    let prompt = transcript.find("Passphrase for beta:").unwrap();
    let left = transcript[..handoff].rfind("\x1b[?1049l").unwrap();
    assert!(
        transcript[..handoff].rfind("\x1b[?25h") > transcript[..handoff].rfind("\x1b[?25l"),
        "{transcript}"
    );
    assert!(transcript[handoff..prompt].contains(HANDOFF_NOTICE));
    assert!(transcript[handoff..prompt].contains("~/repos/beta"));
    assert!(!transcript[left..prompt].contains("\x1b[?1049h"));
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 2);
    assert_eq!(transcript.matches("\x1b[?1049l").count(), 2);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_clone_only_from_the_row_menu_runs_the_hook_on_the_terminal_and_keeps_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    fs::create_dir_all(&root).unwrap();
    prompting_git(&tools, &answers);
    let reference = clone_source(temp.path(), "alpha");
    let hook = "printf 'hook ready: \\033[0m'; IFS= read -r line; printf '%s' \"$line\" > \"$HOME/hook-answer\"";
    let original =
        format!("root = \"repos\"\n[[repositories]]\nurl = {reference:?}\npost_clone = {hook:?}\n");
    fs::write(&config, &original).unwrap();

    let transcript = run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "missing"],
                b"kkm\r".to_vec(),
            ),
            (
                vec!["Repository actions / alpha", "Clone only"],
                b"jj\r".to_vec(),
            ),
            // The second line is typed during the clone; it must not answer the hook.
            (
                vec!["Passphrase for alpha:"],
                b"git says q\rhook-leak\r".to_vec(),
            ),
            (vec!["hook ready:"], b"hook says j\r".to_vec()),
            (
                vec![
                    "Batch / Clone only",
                    "1 / 1 processed",
                    "pair: SUCCEEDED alpha",
                    "pair: alpha cloned",
                    "Clone only: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Batch /",
                    "absent: Scanning",
                    "pair: explicit cloned",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::read_to_string(&answers).unwrap(), "git says q\n");
    assert_eq!(
        fs::read_to_string(home.join("hook-answer")).unwrap(),
        "hook says j"
    );
    assert!(root.join("alpha/.git").is_dir());
    assert!(transcript.contains("lager: clone 1 of 1 · "));
    assert!(transcript.contains(HANDOFF_NOTICE));
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 2);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_clone_batch_reports_each_target_and_continues_past_a_failed_and_a_stale_one() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    fs::create_dir_all(&root).unwrap();
    prompting_git(&tools, &answers);
    let mut original = "root = \"repos\"\n".to_owned();
    for name in ["alpha", "delta", "gamma"] {
        let reference = clone_source(temp.path(), name);
        original.push_str(&format!("[[repositories]]\nurl = {reference:?}\n"));
    }
    let broken = format!("file://{}", temp.path().join("remotes/broken").display());
    original.push_str(&format!("[[repositories]]\nurl = {broken:?}\n"));
    fs::write(&config, &original).unwrap();
    let mut modes = Vec::new();

    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 4 rows", "absent: Scanning"],
                b"kkk j j j ".to_vec(),
            ),
            (vec!["4 marked · 0 hidden"], b"m\rjj\r".to_vec()),
            (
                vec![
                    "CONFIRM",
                    "Clone only / preview",
                    "4 exact targets",
                    "ELIGIBLE alpha",
                    "ELIGIBLE broken",
                    "ELIGIBLE delta",
                    "ELIGIBLE gamma",
                    "~/repos/delta",
                    "pair: Cancel default",
                    "Clone only 4 targets",
                    "Nothing changes until you confirm",
                ],
                b"j\r".to_vec(),
            ),
            // `extra` stays unread in the terminal's line buffer; it must not join the next answer.
            (vec!["Passphrase for alpha:"], b"a1\rextra".to_vec()),
            (vec!["Passphrase for broken:"], b"b2\r".to_vec()),
            // A line typed before the failure is reported must not dismiss the wait.
            (vec!["Passphrase for gamma:"], b"g3\rskip\r".to_vec()),
            (vec!["Enter returns to the inventory."], b"\r".to_vec()),
            (
                vec![
                    "Batch / Clone only",
                    "4 / 4 processed",
                    "pair: SUCCEEDED alpha",
                    "pair: alpha cloned",
                    "pair: FAILED broken",
                    "git exited with status 128",
                    "pair: BLOCKED delta",
                    "stale: destination is no longer missing after",
                    "pair: SUCCEEDED gamma",
                    "Clone only: 2 succeeded, 1 failed, 1 blocked",
                    "· 2 problems",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 4 rows", "absent: Batch /", "absent: Scanning"],
                format!("m{}\r", "j".repeat(9)).into_bytes(),
            ),
            (
                vec![
                    "Session problems / 2",
                    "batch · broken",
                    "Clone only failed: git exited with status 128",
                    "batch · delta",
                    "Clone only blocked: stale: destination is no longer missing after",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Session problems /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action, terminal, frame| {
            if action == 2 {
                fs::create_dir_all(root.join("delta")).unwrap();
            }
            if matches!(action, 4 | 6 | 7) {
                modes.push(cooked(terminal));
            }
            if action == 6 {
                // Git's own error is still on the normal screen and the inventory has not
                // come back over it.
                let shown = &frame[frame.rfind("\x1b[?1049l").unwrap()..];
                assert!(!shown.contains("\x1b[?1049h"), "{frame}");
                assert!(shown.contains("fatal:"), "{frame}");
                assert!(
                    shown.contains("lager: failed · broken · git exited with status 128"),
                    "{frame}"
                );
            }
        },
    );

    assert_eq!(modes, [true, true, false], "{transcript}");
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 2);
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::read_to_string(&answers).unwrap(), "a1\nb2\ng3\n");
    assert!(root.join("alpha/.git").is_dir());
    assert!(root.join("gamma/.git").is_dir());
    assert!(!root.join("broken").exists());
    assert_eq!(fs::read_dir(root.join("delta")).unwrap().count(), 0);
    assert!(transcript.contains("lager: clone 1 of 4 · "));
    assert!(transcript.contains("lager: clone 2 of 4 · "));
    assert!(!transcript.contains("lager: clone 3 of 4 · "));
    assert!(transcript.contains("lager: clone 4 of 4 · "));
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_c_during_a_clone_handoff_exits_130_and_keeps_the_earlier_clone() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(root.join("misc")).unwrap();
    prompting_git(&tools, &answers);
    let alpha = clone_source(temp.path(), "alpha");
    let beta = clone_source(temp.path(), "beta");
    run_git(&root, &["clone", "-q", &alpha, "misc/one"]);
    run_git(&root, &["clone", "-q", &beta, "misc/two"]);
    let tree = checkout_tree(&root.join("misc"));
    let mut modes = Vec::new();

    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "absent: pending"],
                b"kk j ".to_vec(),
            ),
            (vec!["2 marked · 0 hidden"], b"m\rjjj\r".to_vec()),
            (
                vec![
                    "Clone + register / preview",
                    "2 exact targets",
                    "ELIGIBLE alpha",
                    "found at ~/repos/misc/one · clones at ~/repos/alpha",
                    "ELIGIBLE beta",
                    "found at ~/repos/misc/two · clones at ~/repos/beta",
                    "Clone + register 2 targets",
                ],
                b"j\r".to_vec(),
            ),
            (vec!["Passphrase for alpha:"], b"first\r".to_vec()),
            (vec!["Passphrase for beta:"], vec![3]),
        ],
        32,
        132,
        130,
        |action, terminal, _| {
            if matches!(action, 2 | 4) {
                modes.push(cooked(terminal));
            }
        },
    );

    assert_eq!(modes, [false, true], "{transcript}");
    assert_eq!(fs::read_to_string(&answers).unwrap(), "first\n");
    assert!(root.join("alpha/.git").is_dir());
    assert!(!root.join("beta").exists());
    assert_eq!(checkout_tree(&root.join("misc")), tree);
    let other = temp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(&other, original, &["add", &alpha, "--register"])
    );
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 1);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_clone_register_on_a_matching_checkout_registers_without_cloning() {
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
    let tree = checkout_tree(&root);

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "absent: pending"],
                b"aorg/alpha\r".to_vec(),
            ),
            (
                vec![
                    "Add / preview",
                    "pair: Checkout cloned",
                    "pair: Registration unregistered",
                ],
                b"kkkkjj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "1 / 1 processed",
                    "pair: SUCCEEDED github.com/org/alpha",
                    "pair: github.com/org/alpha updated",
                    "Clone + register: 1 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Batch /",
                    "pair: explicit cloned",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );

    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(&home, original, &["add", "org/alpha", "--register"])
    );
    assert_ne!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 1);
}

#[cfg(unix)]
#[test]
fn inventory_add_blocks_a_clone_whose_destination_appeared_after_the_preview() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();
    prompting_git(&tools, &answers);
    let reference = clone_source(temp.path(), "late");

    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["NORMAL · 0 rows", "absent: Scanning"], b"a".to_vec()),
            (
                vec!["Add / URL or reference"],
                format!("{reference}\r").into_bytes(),
            ),
            (
                vec!["Add / preview", "pair: Checkout missing"],
                b"kkkkjj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "1 / 1 processed",
                    "BLOCKED",
                    "stale: destination is no longer missing after preview",
                    "Clone + register: 1 blocked",
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
            if action == 2 {
                fs::create_dir_all(root.join("late")).unwrap();
            }
        },
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!answers.exists());
    assert_eq!(fs::read_dir(root.join("late")).unwrap().count(), 0);
}

/// A Git wrapper that clones every URL from the local repository of the same name and then
/// records the requested URL as the origin, so hosted references never leave the machine.
#[cfg(unix)]
fn redirecting_git(tools: &std::path::Path, directory: &std::path::Path) {
    fs::create_dir_all(tools).unwrap();
    let git = shell_word(&support::real_tool("git"));
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = clone ]; then\n  {git} clone -q \"file://\"{}\"/$(basename \"$2\" .git)\" . || exit\n  exec {git} remote set-url origin \"$2\"\nfi\nexec {git} \"$@\"\n",
            shell_word(&directory.join("remotes")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn inventory_failed_hook_keeps_the_clone_skips_registration_and_lets_the_next_target_run() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    fs::create_dir_all(&root).unwrap();
    redirecting_git(&tools, temp.path());
    clone_source(temp.path(), "alpha");
    clone_source(temp.path(), "beta");
    // Registering alpha would lift its exclusion; a failed hook must leave it in place.
    let original = "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"alpha\"]\n[[repositories]]\nurl = \"git@github.com:org/alpha.git\"\npost_clone = \"echo hook broke >&2; exit 3\"\n[[repositories]]\nurl = \"git@github.com:org/beta.git\"\n";
    fs::write(&config, original).unwrap();
    let mut modes = Vec::new();

    let transcript = run_inventory_pty_with_exit_observing(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kk j ".to_vec(),
            ),
            (vec!["2 marked · 0 hidden"], b"m\rjjj\r".to_vec()),
            (
                vec![
                    "Clone + register / preview",
                    "ELIGIBLE alpha",
                    "ELIGIBLE beta",
                    "Clone + register 2 targets",
                ],
                b"j\r".to_vec(),
            ),
            (vec!["Enter returns to the inventory."], b"\r".to_vec()),
            (
                vec![
                    "Batch / Clone + register",
                    "2 / 2 processed",
                    "pair: FAILED alpha",
                    "pair: alpha cloned",
                    "hook exited with status 3",
                    "pair: SUCCEEDED beta",
                    "pair: beta cloned",
                    "Clone + register: 1 succeeded, 1 failed",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 3 rows", "absent: Batch /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action, terminal, frame| {
            if matches!(action, 3 | 4) {
                modes.push(cooked(terminal));
            }
            if action == 3 {
                let shown = &frame[frame.rfind("\x1b[?1049l").unwrap()..];
                assert!(!shown.contains("\x1b[?1049h"), "{frame}");
                assert!(shown.contains("hook broke"), "{frame}");
                assert!(
                    shown.contains("lager: failed · alpha · hook exited with status 3"),
                    "{frame}"
                );
                assert!(
                    shown.contains("lager: clone 2 of 2 · github.com/org/beta → ~/repos/org/beta")
                );
            }
        },
    );

    assert_eq!(modes, [true, false], "{transcript}");
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(root.join("org/alpha/.git").is_dir());
    assert!(root.join("org/beta/.git").is_dir());
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_c_during_a_hook_exits_130_and_keeps_the_clone() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let answers = temp.path().join("answers");
    fs::create_dir_all(&root).unwrap();
    prompting_git(&tools, &answers);
    let reference = clone_source(temp.path(), "alpha");
    let hook = "printf 'hook ready: \\033[0m'; IFS= read -r line; touch \"$HOME/hook-finished\"";
    let original =
        format!("root = \"repos\"\n[[repositories]]\nurl = {reference:?}\npost_clone = {hook:?}\n");
    fs::write(&config, &original).unwrap();

    let transcript = run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "missing"],
                b"kkm\r".to_vec(),
            ),
            (
                vec!["Repository actions / alpha", "Clone only"],
                b"jj\r".to_vec(),
            ),
            (vec!["Passphrase for alpha:"], b"open\r".to_vec()),
            (vec!["hook ready:"], vec![3]),
        ],
        32,
        132,
        130,
        |_| {},
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(root.join("alpha/.git").is_dir());
    assert!(!home.join("hook-finished").exists());
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 1);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_c_while_a_failed_clone_waits_for_enter_exits_130() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    let broken = format!("file://{}", temp.path().join("remotes/broken").display());
    let original = format!("root = \"repos\"\n[[repositories]]\nurl = {broken:?}\n");
    fs::write(&config, &original).unwrap();

    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "missing"],
                b"kkm\r".to_vec(),
            ),
            (
                vec!["Repository actions / broken", "Clone only"],
                b"jj\r".to_vec(),
            ),
            (vec!["Enter returns to the inventory."], vec![3]),
        ],
        32,
        132,
        130,
        |_| {},
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("broken").exists());
    assert!(transcript.contains("lager: failed · broken · git exited with status 128"));
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 1);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_keeps_keys_typed_at_it_when_a_suspend_starts_no_child() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();
    let reference = clone_source(temp.path(), "late");

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 0 rows", "absent: Scanning"], b"a".to_vec()),
            (
                vec!["Add / URL or reference"],
                format!("{reference}\r").into_bytes(),
            ),
            // The target is stale, so no child starts: the second Enter belongs to the
            // inventory and closes the outcomes.
            (
                vec!["Add / preview", "pair: Checkout missing"],
                b"kkkkjj\r\r".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "· 1 problem"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 2 {
                fs::create_dir_all(root.join("late")).unwrap();
            }
        },
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::read_dir(root.join("late")).unwrap().count(), 0);
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
        command.arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
        support::lager(&home, &config).arg("tui"),
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
            support::lager(&home, &config).arg("tui"),
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

/// A fixture checkout whose branch tracks its origin, so removal finds nothing to lose.
#[cfg(unix)]
fn tracked_repo(path: &std::path::Path, origin: &str) {
    init_repo(path, origin, true);
    run_git(path, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    run_git(path, &["branch", "-q", "--set-upstream-to=origin/main"]);
}

/// A primary checkout Git still lists a linked worktree for, outside the root.
#[cfg(unix)]
fn add_dependent_worktree(primary: &std::path::Path, outside: &std::path::Path) {
    run_git(
        primary,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            outside.to_str().unwrap(),
        ],
    );
}

/// alpha is clean and declared, core has a dependent worktree, draft has local changes.
#[cfg(unix)]
fn removal_fixture(root: &std::path::Path, outside: &std::path::Path) {
    for name in ["alpha", "core", "draft"] {
        tracked_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
        );
    }
    add_dependent_worktree(&root.join("org/core"), outside);
    fs::write(root.join("org/draft/README"), "modified\n").unwrap();
    fs::write(root.join("org/draft/notes"), "untracked\n").unwrap();
}

#[cfg(unix)]
#[test]
fn inventory_removal_confirmation_lists_clean_loss_and_blocked_targets_and_cancelling_changes_nothing()
 {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    removal_fixture(&root, &temp.path().join("dependent"));
    init_repo_without_origin(&root.join("scratch/local"));
    let tree = checkout_tree(&root);
    let kept = |message: &'static str| {
        vec![
            "NORMAL · 1 row",
            "absent: Remove checkouts /",
            message,
            "4 marked · 3 hidden",
            "selected: draft",
            "absent: problem",
            "absent: Batch /",
        ]
    };

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 4 rows", "absent: Scanning"],
                b"kk j j j /github.com/org/draft\r".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "4 marked · 3 hidden", "pair: r remove"],
                b"r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM",
                    "Remove checkouts / keep registration",
                    "4 exact targets · 3 hidden by filter",
                    "Registration and exclusions stay unchanged. ensure can recreate these checkouts.",
                    "ELIGIBLE alpha (hidden by filter)",
                    "~/repos/org/alpha · github.com/org/alpha · clean",
                    "BLOCKED core (hidden by filter)",
                    "dependent worktree metadata",
                    "force cannot waive this",
                    "LOSS ACKNOWLEDGMENT REQUIRED draft",
                    "~/repos/org/draft · github.com/org/draft · unstaged changes, untracked files",
                    "BLOCKED local (hidden by filter)",
                    "~/repos/scratch/local · no identity · no supported origin observed",
                    "[ ] Also remove draft and accept its listed losses",
                    "pair: losses Space",
                    "pair: Cancel default",
                    "Remove 1 checkout",
                    "alpha · cannot be undone",
                    "Nothing changes until you confirm",
                    "Space acknowledge",
                    "pair: Esc cancel",
                ],
                b"\x1b".to_vec(),
            ),
            (
                kept("Remove only cancelled · nothing changed"),
                b"r".to_vec(),
            ),
            (
                vec!["[ ] Also remove draft and accept its listed losses"],
                b" ".to_vec(),
            ),
            (
                vec![
                    "[x] Also remove draft and accept its listed losses",
                    "Remove 2 checkouts",
                    "alpha, draft · cannot be undone",
                    "pair: Cancel default",
                ],
                b"\r".to_vec(),
            ),
            (
                kept("Remove only cancelled · nothing changed"),
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
fn inventory_remove_only_continues_past_blocked_targets_and_keeps_registration() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    removal_fixture(&root, &temp.path().join("dependent"));
    let kept = [
        checkout_tree(&root.join("org/core")),
        checkout_tree(&root.join("org/draft")),
    ];

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kk j j ".to_vec(),
            ),
            (vec!["3 marked · 0 hidden"], b"r".to_vec()),
            (
                vec![
                    "Remove checkouts / keep registration",
                    "ELIGIBLE alpha",
                    "BLOCKED core",
                    "LOSS ACKNOWLEDGMENT REQUIRED draft",
                    "[ ] Also remove draft",
                    "Remove 1 checkout",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "3 / 3 processed",
                    "pair: SUCCEEDED alpha",
                    "pair: alpha removed",
                    "pair: BLOCKED core",
                    "dependent worktree metadata",
                    "pair: BLOCKED draft",
                    "losses not acknowledged: unstaged changes",
                    "Remove only: 1 succeeded, 2 blocked",
                    "· 2 problems",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 3 rows", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |_| {},
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("org/alpha").exists());
    assert_eq!(
        [
            checkout_tree(&root.join("org/core")),
            checkout_tree(&root.join("org/draft"))
        ],
        kept
    );
}

#[cfg(unix)]
#[test]
fn inventory_acknowledged_losses_are_removed_while_a_blocker_and_a_new_loss_stay() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    removal_fixture(&root, &temp.path().join("dependent"));
    fs::remove_dir_all(root.join("org/alpha")).unwrap();
    // The blocker is dirty too: the acknowledgment must not reach it.
    fs::write(root.join("org/core/notes"), "untracked\n").unwrap();
    tracked_repo(&root.join("org/shifty"), "git@github.com:org/shifty.git");
    fs::write(root.join("org/shifty/notes"), "untracked\n").unwrap();
    let core = checkout_tree(&root.join("org/core"));

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kk j j ".to_vec(),
            ),
            (vec!["3 marked · 0 hidden"], b"r".to_vec()),
            (
                vec![
                    "BLOCKED core",
                    "LOSS ACKNOWLEDGMENT REQUIRED draft",
                    "LOSS ACKNOWLEDGMENT REQUIRED shifty",
                    "~/repos/org/shifty · github.com/org/shifty · untracked files",
                    "[ ] Also remove 2 checkouts and accept their listed losses",
                    "Remove 0 checkouts",
                ],
                b" ".to_vec(),
            ),
            (
                vec![
                    "[x] Also remove 2 checkouts and accept their listed losses",
                    "Remove 2 checkouts",
                    "draft, shifty · cannot be undone",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "3 / 3 processed",
                    "pair: BLOCKED core",
                    "dependent worktree metadata",
                    "pair: SUCCEEDED draft",
                    "pair: draft removed",
                    "pair: BLOCKED shifty",
                    "stale: local state changed after preview",
                    "Remove only: 1 succeeded, 2 blocked",
                    "· 2 problems",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 3 {
                // A loss class that was not on the list when the box was ticked.
                fs::write(root.join("org/shifty/README"), "modified\n").unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("org/draft").exists());
    assert_eq!(checkout_tree(&root.join("org/core")), core);
    assert_eq!(
        fs::read_to_string(root.join("org/shifty/README")).unwrap(),
        "modified\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("org/shifty/notes")).unwrap(),
        "untracked\n"
    );
    assert!(root.join("org/shifty/.git").is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_remove_unregister_reports_a_deleted_checkout_and_a_failed_config_write_separately() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let directory = temp.path().join("etc");
    let config = directory.join("config.toml");
    fs::create_dir_all(&directory).unwrap();
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    for directory in ["org", "archive"] {
        tracked_repo(
            &root.join(directory).join("alpha"),
            "git@github.com:org/alpha.git",
        );
    }
    tracked_repo(&root.join("org/beta"), "git@github.com:org/beta.git");
    let beta = checkout_tree(&root.join("org/beta"));
    // The lock lives in the test cache beside the config; only the config's own directory is read-only.
    fs::create_dir_all(directory.join("lager-test-cache")).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("tui"),
            &[
                (
                    vec!["NORMAL · 3 rows", "absent: Scanning"],
                    b"kk j ".to_vec(),
                ),
                (vec!["2 marked · 0 hidden"], b"m\rjjjjj\r".to_vec()),
                (
                    vec![
                        "Remove checkouts / unregister",
                        "2 exact targets",
                        "ELIGIBLE alpha",
                        "ELIGIBLE alpha (copy)",
                        "Remove + unregister 2 targets",
                    ],
                    b"j\r".to_vec(),
                ),
                (
                    vec![
                        "Batch / Remove + unregister",
                        "2 / 2 processed",
                        "pair: SUCCEEDED alpha",
                        // The first checkout is gone; its row points at the one that failed.
                        "registration kept: see alpha (copy)",
                        "pair: FAILED alpha",
                        "pair: (copy) removed",
                        "pair: removed write",
                        "pair: write failed",
                        "could not write config",
                        "Remove + unregister: 1 succeeded, 1 failed",
                        "· 1 problem",
                    ],
                    b"\x1b".to_vec(),
                ),
                (
                    vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                    b"q".to_vec(),
                ),
            ],
            32,
            132,
            1,
            |_| {},
        );
    }));
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("org/alpha").exists());
    assert!(!root.join("archive/alpha").exists());
    assert_eq!(checkout_tree(&root.join("org/beta")), beta);
}

#[cfg(unix)]
#[test]
fn inventory_remove_unregister_keeps_registration_while_a_selected_checkout_of_the_identity_stays()
{
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    for directory in ["org", "archive"] {
        tracked_repo(
            &root.join(directory).join("alpha"),
            "git@github.com:org/alpha.git",
        );
    }
    add_dependent_worktree(&root.join("org/alpha"), &temp.path().join("dependent"));
    let declared = checkout_tree(&root.join("org/alpha"));

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kk j ".to_vec(),
            ),
            (vec!["2 marked · 0 hidden"], b"m\rjjjjj\r".to_vec()),
            (
                vec![
                    "Remove checkouts / unregister",
                    "2 exact targets",
                    "BLOCKED alpha",
                    "~/repos/org/alpha · github.com/org/alpha",
                    "ELIGIBLE alpha (copy)",
                    "~/repos/archive/alpha · github.com/org/alpha · clean",
                    "Remove + unregister 1 target",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove + unregister",
                    "2 / 2 processed",
                    "pair: BLOCKED alpha",
                    "dependent worktree metadata",
                    "pair: (copy) removed",
                    "registration kept: alpha was not removed",
                    "Remove + unregister: 2 blocked",
                    "· 2 problems",
                    "absent: updated",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |_| {},
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("archive/alpha").exists());
    assert_eq!(checkout_tree(&root.join("org/alpha")), declared);
}

#[cfg(unix)]
#[test]
fn inventory_remove_unregister_writes_once_per_identity_and_previews_the_unselected_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/beta\"\n[[repositories]]\nurl = \"github.com/org/gone\"\n";
    fs::write(&config, original).unwrap();
    for directory in ["org", "archive", "keep"] {
        tracked_repo(
            &root.join(directory).join("beta"),
            "git@github.com:org/beta.git",
        );
    }
    let unselected = checkout_tree(&root.join("keep/beta"));
    let mut configs = Vec::new();

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 4 rows", "absent: Scanning"],
                b"kk j j ".to_vec(),
            ),
            (vec!["3 marked · 0 hidden"], b"m\rjjjjj\r".to_vec()),
            (
                vec![
                    "Remove checkouts / unregister",
                    "3 exact targets",
                    "ELIGIBLE beta",
                    "~/repos/org/beta · github.com/org/beta · clean",
                    "also unregisters ~/repos/keep/beta · not selected, its files stay",
                    "ELIGIBLE gone",
                    "~/repos/org/gone · github.com/org/gone · known missing · absence is rechecked first",
                    "ELIGIBLE beta (copy)",
                    "~/repos/archive/beta · github.com/org/beta · clean",
                    "Remove + unregister 3 targets",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove + unregister",
                    "3 / 3 processed",
                    "pair: SUCCEEDED beta",
                    "pair: beta removed",
                    "unregistered with beta (copy)",
                    "pair: SUCCEEDED gone",
                    "pair: gone updated",
                    "absent · rechecked",
                    "pair: (copy) removed",
                    "pair: removed updated",
                    "Remove + unregister: 3 succeeded",
                    "absent: problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                configs.push(fs::read_to_string(&config).unwrap());
            }
        },
    );
    let gone = cli_config(&home, original, &["unregister", "github.com/org/gone"]);
    let both = cli_config(&home, &gone, &["unregister", "git@github.com:org/beta.git"]);
    assert!(!both.contains("[[repositories]]"), "{both}");
    assert_eq!(configs, [original]);
    assert_eq!(fs::read_to_string(&config).unwrap(), both);
    assert!(!root.join("org/beta").exists());
    assert!(!root.join("archive/beta").exists());
    assert_eq!(checkout_tree(&root.join("keep/beta")), unselected);
}

#[cfg(unix)]
#[test]
fn inventory_removal_blocks_targets_whose_origin_path_or_registration_changed_after_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n[[repositories]]\nurl = \"github.com/org/delta\"\n[[repositories]]\nurl = \"github.com/org/gamma\"\n";
    fs::write(&config, original).unwrap();
    for name in ["alpha", "beta", "delta", "gamma"] {
        tracked_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
        );
    }
    let gamma = ["unregister", "github.com/org/gamma"];
    let mut changed = None;

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 4 rows", "absent: Scanning"],
                b"kk j j j ".to_vec(),
            ),
            (vec!["4 marked · 0 hidden"], b"m\rjjjjj\r".to_vec()),
            (
                vec![
                    "Remove checkouts / unregister",
                    "4 exact targets",
                    "ELIGIBLE alpha",
                    "ELIGIBLE delta",
                    "ELIGIBLE gamma",
                    "ELIGIBLE beta",
                    "Remove + unregister 4 targets",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove + unregister",
                    "4 / 4 processed",
                    "pair: BLOCKED alpha",
                    "stale: origin changed after preview",
                    "pair: SUCCEEDED delta",
                    "pair: delta removed",
                    "pair: removed updated",
                    "pair: BLOCKED gamma",
                    "stale: registration changed after preview",
                    "pair: BLOCKED beta",
                    "stale: checkout is gone or unreadable",
                    "Remove + unregister: 1 succeeded, 3 blocked",
                    "· 3 problems",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 2 {
                run_git(
                    &root.join("org/alpha"),
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "git@github.com:org/other.git",
                    ],
                );
                fs::rename(root.join("org/beta"), root.join("org/moved")).unwrap();
                let result = support::lager(&home, &config).args(gamma).output().unwrap();
                assert!(result.status.success(), "{result:?}");
                changed = Some(checkout_tree(&root));
            }
        },
    );
    let unregistered = cli_config(&home, original, &gamma);
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        cli_config(
            &home,
            &unregistered,
            &["unregister", "git@github.com:org/delta.git"]
        )
    );
    let mut expected = changed.expect("the hook ran");
    expected.retain(|path, _| !path.starts_with("org/delta"));
    assert!(expected.keys().any(|path| path.starts_with("org/moved")));
    assert_eq!(checkout_tree(&root), expected);
}

#[cfg(unix)]
#[test]
fn inventory_remove_unregister_rechecks_that_a_known_missing_destination_is_still_absent() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/gone\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "missing"],
                b"kkr".to_vec(),
            ),
            (
                vec![
                    "Remove only unavailable: no checkout at the configured destination",
                    "absent: Remove checkouts /",
                ],
                b"m\rjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Remove checkouts / unregister",
                    "ELIGIBLE gone",
                    "known missing · absence is rechecked first",
                    "Remove + unregister 1 target",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove + unregister",
                    "pair: BLOCKED gone",
                    "stale: destination is no longer missing",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 2 {
                fs::create_dir_all(root.join("org/gone")).unwrap();
                fs::write(root.join("org/gone/keep"), "arrived\n").unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(
        fs::read_to_string(root.join("org/gone/keep")).unwrap(),
        "arrived\n"
    );
}

#[cfg(unix)]
#[test]
fn inventory_keys_typed_before_a_confirmation_is_drawn_never_answer_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    tracked_repo(&root.join("org/alpha"), "git@github.com:org/alpha.git");
    fs::write(root.join("org/alpha/notes"), "untracked\n").unwrap();
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            // Acknowledge, move to the run choice, choose: all typed with the `r`.
            (
                vec!["NORMAL · 1 row", "absent: Scanning"],
                b"kkr j\r".to_vec(),
            ),
            // The box is unticked and Enter lands on Cancel, where the cursor still is.
            (
                vec![
                    "Remove checkouts / keep registration",
                    "LOSS ACKNOWLEDGMENT REQUIRED alpha",
                    "[ ] Also remove alpha and accept its listed losses",
                    "pair: Cancel default",
                    "absent: Batch /",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Remove only cancelled · nothing changed",
                    "absent: Batch /",
                    "absent: problem",
                ],
                b"kkm\r\r".to_vec(),
            ),
            (
                vec!["Batch / Register", "pair: SUCCEEDED alpha"],
                b"\x1b".to_vec(),
            ),
            // The same holds for a registration preview opened from the menu with marks.
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"kk m\rj\rj\r".to_vec(),
            ),
            (
                vec![
                    "Unregister / preview",
                    "ELIGIBLE alpha",
                    "pair: Cancel default",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Unregister cancelled · nothing changed", "absent: Batch /"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(checkout_tree(&root), tree);
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
fn inventory_removal_never_deletes_a_checkout_that_replaced_the_confirmed_one() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    let alpha = root.join("org/alpha");
    tracked_repo(&alpha, "git@github.com:org/alpha.git");
    let confirmed = checkout_tree(&alpha);
    let mut replacement = None;

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"kkr".to_vec()),
            (
                vec![
                    "Remove checkouts / keep registration",
                    "ELIGIBLE alpha",
                    "~/repos/org/alpha · github.com/org/alpha · clean",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "pair: BLOCKED alpha",
                    "stale: checkout was replaced after preview",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 1 {
                // Another clean, pushed repository of the same origin at the same path.
                fs::rename(&alpha, temp.path().join("moved")).unwrap();
                tracked_repo(&alpha, "git@github.com:org/alpha.git");
                fs::write(alpha.join("REPLACEMENT"), "other work\n").unwrap();
                run_git(&alpha, &["add", "REPLACEMENT"]);
                run_git(&alpha, &["commit", "-qm", "replacement"]);
                run_git(&alpha, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
                replacement = Some(checkout_tree(&alpha));
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(Some(checkout_tree(&alpha)), replacement);
    assert_eq!(checkout_tree(&temp.path().join("moved")), confirmed);
}

#[cfg(unix)]
#[test]
fn inventory_removal_never_follows_an_ancestor_that_became_a_symlink_to_another_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    for directory in ["archive", "org"] {
        tracked_repo(
            &root.join(directory).join("alpha"),
            "git@github.com:org/alpha.git",
        );
    }
    let trees = [
        checkout_tree(&root.join("archive")),
        checkout_tree(&root.join("org")),
    ];

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"kkjr".to_vec(),
            ),
            (
                vec![
                    "Remove checkouts / keep registration",
                    "1 exact target",
                    "ELIGIBLE alpha (copy)",
                    "~/repos/org/alpha · github.com/org/alpha · clean",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "1 / 1 processed",
                    "BLOCKED",
                    "stale: selected clone path changed",
                    "· 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Batch /", "absent: Scanning"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action| {
            if action == 1 {
                // `~/repos/org/alpha` now resolves to the other checkout of the same origin.
                fs::rename(root.join("org"), temp.path().join("moved")).unwrap();
                std::os::unix::fs::symlink(root.join("archive"), root.join("org")).unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(root.join("org").is_symlink());
    assert_eq!(
        [
            checkout_tree(&root.join("archive")),
            checkout_tree(&temp.path().join("moved"))
        ],
        trees
    );
}

#[cfg(unix)]
#[test]
fn inventory_remove_unregister_waits_for_a_complete_scan_and_then_previews_every_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    for directory in ["org", "zeta"] {
        tracked_repo(
            &root.join(directory).join("alpha"),
            "git@github.com:org/alpha.git",
        );
    }
    let tree = checkout_tree(&root);
    // The second checkout of the identity stays unobserved until the release.
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\ncase \"$2\" in */zeta/alpha) while [ ! -f {} ]; do /bin/sleep 0.02; done;; esac\nexec {} \"$@\"\n",
            shell_word(&release),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();

    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["Scanning", "pair: main clean", "selected: org/alpha"],
                b"kkm\rjjjjj".to_vec(),
            ),
            // The entry says why it is unavailable, and choosing it does nothing: Esc then
            // leaves the submenu, not a confirmation.
            (
                vec![
                    "Repository actions /",
                    "Scanning",
                    "pair: unregister the",
                    "the scan is not complete",
                ],
                b"\r\x1b".to_vec(),
            ),
            (
                vec![
                    "All actions",
                    "Scanning",
                    "absent: Repository actions /",
                    "absent: Remove checkouts /",
                ],
                Vec::new(),
            ),
            (
                vec!["All actions", "absent: Scanning"],
                b"\rjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Remove checkouts / unregister",
                    "1 exact target",
                    "ELIGIBLE alpha",
                    "also unregisters ~/repos/zeta/alpha · not selected, its files stay",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove + unregister cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_direct_remove_unregister_is_blocked_while_the_scan_is_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n\
                    [inventory.keys.normal]\nremove_unregister = [\"U\"]\n";
    fs::write(&config, original).unwrap();
    for directory in ["org", "zeta"] {
        tracked_repo(
            &root.join(directory).join("alpha"),
            "git@github.com:org/alpha.git",
        );
    }
    let tree = checkout_tree(&root);
    // The second checkout of the identity stays unobserved until the release.
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\ncase \"$2\" in */zeta/alpha) while [ ! -f {} ]; do /bin/sleep 0.02; done;; esac\nexec {} \"$@\"\n",
            shell_word(&release),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();

    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["Scanning", "pair: main clean"], b"kkU".to_vec()),
            (
                vec![
                    "Remove + unregister unavailable: the scan is not complete",
                    "Scanning",
                    "absent: Remove checkouts /",
                ],
                Vec::new(),
            ),
            (vec!["NORMAL", "absent: Scanning"], b"q".to_vec()),
        ],
        32,
        132,
        |action| {
            if action == 1 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_removal_names_the_declaration_whose_destination_a_found_checkout_occupies() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n";
    fs::write(&config, original).unwrap();
    tracked_repo(&root.join("org/alpha"), "git@github.com:org/other.git");
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "conflict"],
                b"kkr".to_vec(),
            ),
            // The declaration's own row has no checkout to remove.
            (
                vec![
                    "Remove only unavailable: destination conflicts with repository",
                    "absent: Remove checkouts /",
                    "selected: alpha",
                ],
                b"jr".to_vec(),
            ),
            (
                vec![
                    "Remove checkouts / keep registration",
                    "1 exact target",
                    "ELIGIBLE other",
                    "~/repos/org/alpha · github.com/org/other · clean",
                    "sits at the configured destination of github.com/org/alpha · that declaration stays",
                    "Remove 1 checkout",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove only cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

/// Three clean marked checkouts, Remove only. A `git` stub holds the batch inside beta, its
/// second target, while `typed` is written; the keys are read once beta is done and before
/// gamma starts. `tail` continues from there.
#[cfg(unix)]
fn removal_held_at_its_second_target(
    typed: &[u8],
    tail: Vec<(Vec<&'static str>, Vec<u8>)>,
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let armed = temp.path().join("armed");
    let holding = temp.path().join("holding");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    for name in ["alpha", "beta", "gamma"] {
        tracked_repo(
            &root.join("org").join(name),
            &format!("git@github.com:org/{name}.git"),
        );
    }
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\ncase \"$2\" in */beta) if [ -f {} ]; then : > {}; while [ ! -f {} ]; do /bin/sleep 0.02; done; fi;; esac\nexec {} \"$@\"\n",
            shell_word(&armed),
            shell_word(&holding),
            shell_word(&release),
            shell_word(&support::real_tool("git")),
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let held = vec![
        "Batch / Remove only",
        "1 / 3 processed",
        "pair: SUCCEEDED alpha",
        "pair: ACTIVE beta",
        "pair: UNATTEMPTED gamma",
        "Batch: 1 / 3 processed · m → cancel batch",
        "Esc return · q quit",
    ];
    let mut actions = vec![
        (
            vec!["NORMAL · 3 rows", "absent: Scanning"],
            b"kk j j ".to_vec(),
        ),
        (vec!["3 marked · 0 hidden"], b"r".to_vec()),
        (
            vec![
                "Remove checkouts / keep registration",
                "ELIGIBLE alpha",
                "ELIGIBLE beta",
                "ELIGIBLE gamma",
                "Remove 3 checkouts",
            ],
            b"j\r".to_vec(),
        ),
        (held.clone(), typed.to_vec()),
        (held, Vec::new()),
    ];
    actions.extend(tail);

    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &actions,
        32,
        132,
        |action| {
            if action == 2 {
                fs::write(&armed, "hold").unwrap();
            }
            if action == 3 {
                // The keys are written only once Git is held inside beta.
                wait_for_file(&holding);
            }
            if action == 4 {
                assert!(root.join("org/beta").exists(), "beta is held");
                fs::write(&release, "go").unwrap();
            }
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!root.join("org/alpha").exists());
    assert!(!root.join("org/beta").exists());
    (temp, root, transcript)
}

#[cfg(unix)]
fn cancelled_batch_outcomes() -> Vec<(Vec<&'static str>, Vec<u8>)> {
    vec![
        (
            vec![
                "Batch / Remove only",
                "2 / 3 processed",
                "pair: SUCCEEDED alpha",
                "pair: SUCCEEDED beta",
                "pair: UNATTEMPTED gamma",
                "batch cancelled",
                "Batch cancelled · 1 unattempted · completed changes remain",
                "absent: problem",
            ],
            b"\x1b".to_vec(),
        ),
        (
            vec!["NORMAL · 1 row", "absent: Batch /", "absent: Scanning"],
            b"q".to_vec(),
        ),
    ]
}

#[cfg(unix)]
#[test]
fn inventory_menu_cancellation_stops_the_remaining_targets_and_keeps_completed_ones() {
    // Fifteen keys typed while the batch is held: all are read, in order, before gamma starts.
    // Esc directly before `m` reaches the inventory as one Alt+m and still is both keys. The
    // extra `j` and the `k` end on Cancel active batch in the menu only: in the batch popup
    // they would end on Return to browsing.
    let (_temp, root, _) =
        removal_held_at_its_second_target(b"\x1bmjjjjjjjjjjjk\r", cancelled_batch_outcomes());
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_batch_popup_cancels_the_remaining_targets() {
    let (_temp, root, _) = removal_held_at_its_second_target(b"j\r", cancelled_batch_outcomes());
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
const QUIT_PROMPT: [&str; 7] = [
    "CONFIRM",
    "A batch is running",
    "2 of 3 targets processed. Completed changes remain.",
    "Cancelling leaves gamma unattempted.",
    "pair: inventory default",
    "Cancel remaining work and quit",
    "pair: Esc return",
];

#[cfg(unix)]
#[test]
fn inventory_quit_during_a_batch_returns_to_it_by_default() {
    // One `q` with the batch popup open asks; it neither closes the popup nor quits.
    let (_temp, root, _) = removal_held_at_its_second_target(
        b"q",
        vec![
            (QUIT_PROMPT.to_vec(), b"\r".to_vec()),
            (
                vec![
                    "Batch / Remove only",
                    "3 / 3 processed",
                    "absent: A batch is running",
                    "absent: Scanning",
                    "Remove only: 3 succeeded",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 0 rows", "absent: Batch /"], b"q".to_vec()),
        ],
    );
    assert!(!root.join("org/gamma").exists());
}

#[cfg(unix)]
#[test]
fn inventory_quit_during_a_batch_can_cancel_the_remaining_targets_and_quit() {
    let (_temp, root, _) = removal_held_at_its_second_target(
        b"q",
        vec![
            (QUIT_PROMPT.to_vec(), b"?".to_vec()),
            // Help over the prompt keeps the batch waiting.
            (
                vec!["Keyboard / effective bindings", "Confirmation mode"],
                b"q".to_vec(),
            ),
            (QUIT_PROMPT.to_vec(), b"j\r".to_vec()),
        ],
    );
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_esc_and_the_key_typed_after_it_during_a_target_are_both_applied_in_order() {
    // Typed while beta is held, the popup open: Esc Enter, Esc R, q. Each Esc reaches the
    // inventory glued to the next key as Alt+Enter and Alt+Shift+R.
    // Esc closes the popup and Enter inspects; Esc closes Inspect and R refreshes; q, now in
    // normal mode, asks. With Enter lost nothing is ever inspected; with R lost Inspect stays
    // open, q closes it and no prompt appears.
    let (_temp, root, transcript) = removal_held_at_its_second_target(
        b"\x1b\r\x1bRq",
        vec![(QUIT_PROMPT.to_vec(), b"j\r".to_vec())],
    );
    assert!(transcript.contains("Inspect"), "Enter inspected a row");
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_alt_keys_outside_a_batch_do_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n";
    fs::write(&config, original).unwrap();
    tracked_repo(&root.join("org/alpha"), "git@github.com:org/alpha.git");
    let tree = checkout_tree(&root);

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            // Esc and `r` in one write are Alt+r, which no binding names: neither a quit
            // nor a removal. Alt+Enter is no Inspect either.
            (
                vec!["NORMAL · 1 row", "absent: Scanning"],
                b"\x1br\x1b\r".to_vec(),
            ),
            (vec!["NORMAL · 1 row"], b"?".to_vec()),
            (
                vec!["Keyboard / effective bindings", "Normal mode"],
                b"q".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Keyboard"], b"q".to_vec()),
        ],
        32,
        132,
    );
    assert!(!transcript.contains("Remove checkouts /"));
    assert!(!transcript.contains("Inspect /"));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_keys_typed_before_the_add_preview_is_drawn_never_choose_in_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            // The second Enter would choose the focused Clone + register.
            (
                vec!["NORMAL · 0 rows", "absent: Scanning"],
                b"aorg/fresh\r\r".to_vec(),
            ),
            (
                vec![
                    "Add / preview",
                    "github.com/org/fresh",
                    "Clone + register",
                    "absent: Batch /",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add / URL or reference", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add cancelled · nothing changed", "absent: Batch /"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
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
    // Covers the whole action script and has to hold in a parallel suite on a loaded machine.
    let deadline = Instant::now() + Duration::from_secs(30);
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
