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
fn inventory_frames_follow_the_canvas_regions_at_wide_and_compact_sizes() {
    let temp = temp_dir();
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
                    vec!["absent: Scanning", "NORMAL · 5 rows", "1 modified"],
                    b"/alpha".to_vec(),
                ),
                (vec!["SEARCH · 2 rows", "/ alpha"], b"\r".to_vec()),
                // The filtered rows order by owner, so `alpha` in `org` is the second.
                (vec!["NORMAL · 2 rows"], b"j\r".to_vec()),
                (
                    vec![
                        "INSPECT · 2 rows",
                        "Inspect / alpha",
                        "j/k scroll · c copy · Esc close",
                        "absent: q quit",
                    ],
                    b"?".to_vec(),
                ),
                (
                    vec![
                        "HELP · 2 rows",
                        "Keyboard / effective bindings",
                        "j/k scroll · Esc close",
                        "absent: R refresh",
                    ],
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
        // Column starts of the grid at 132 and at 80 columns.
        let headings = [
            "repository",
            "owner",
            "local",
            "branch",
            "status",
            "registration",
        ];
        let narrow_headings = ["repository", "branch", "status"];
        let starts: &[usize] = if compact {
            &[2, 50, 68]
        } else {
            &[2, 24, 42, 88, 106, 118]
        };
        let shown = |wide: [&str; 6], narrow: [&str; 3]| -> Vec<String> {
            // The cases list registration before the observation cells.
            if compact {
                narrow.map(str::to_owned).to_vec()
            } else {
                [0, 1, 2, 4, 5, 3].map(|i| wide[i].to_owned()).to_vec()
            }
        };

        let screen = screen_text(main, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(
            lines[0].starts_with("  LAGER / inventory") && lines[0].ends_with("NORMAL · 5 rows"),
            "{screen}"
        );
        assert_eq!(lines[1], "  Local + declarations · remote off", "{screen}");
        assert_eq!(
            lines[2], "  / Search repositories, or paths with /…",
            "{screen}"
        );
        assert_eq!(lines[3], "", "{screen}");
        assert_eq!(
            lines[4].split_whitespace().collect::<Vec<_>>(),
            if compact {
                &narrow_headings[..]
            } else {
                &headings[..]
            },
            "{screen}"
        );
        let heading_starts = (if compact {
            &narrow_headings[..]
        } else {
            &headings[..]
        })
        .iter()
        .map(|heading| lines[4].find(heading).unwrap())
        .collect::<Vec<_>>();
        assert_eq!(heading_starts, starts, "{screen}");
        assert!(
            !screen.contains(" | ") && !screen.contains('│') && !screen.contains("generation"),
            "{screen}"
        );
        let (_, table) = inventory_grid(&screen);
        assert_eq!(table.len(), 5, "{table:?}\n{screen}");
        assert!(
            !screen.contains("team/*"),
            "a wildcard pattern is no row:\n{screen}"
        );
        for (wide, narrow) in [
            (
                [
                    "alpha",
                    "org",
                    "~/repos/org/alpha",
                    "explicit",
                    "main",
                    "1 modified",
                ],
                ["alpha", "main", "1 modified"],
            ),
            (
                [
                    "alpha (copy)",
                    "archive",
                    "~/repos/archive/alpha",
                    "explicit",
                    "main",
                    "clean",
                ],
                ["alpha (copy)", "main", "clean"],
            ),
            (
                ["beta", "org", "missing", "explicit", "—", "—"],
                ["beta", "—", "—"],
            ),
            (
                [
                    "dots",
                    "team",
                    "~/repos/team/dots",
                    "wildcard",
                    "main",
                    "clean",
                ],
                ["dots", "main", "clean"],
            ),
            (
                [
                    "scratch",
                    "experiments",
                    "~/repos/experiments/scratch",
                    "unregistered",
                    "main",
                    "clean",
                ],
                ["scratch", "main", "clean"],
            ),
        ] {
            let expected = shown(wide, narrow);
            assert!(
                table.contains(&expected),
                "missing {expected:?} in {table:?}\n{screen}"
            );
        }
        for line in &lines[5..10] {
            let characters = line.chars().collect::<Vec<_>>();
            let slot = characters[2..6].iter().collect::<String>();
            assert!(
                characters[..2].iter().all(|character| *character == ' ')
                    && slot == "[ ] "
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
                "  j/k rows  / search  Space mark  a add  r remove  w worktrees  g lazygit  Enter inspect  R refresh  m menu  ? help  q quit",
            ]
        };
        let status = rows - 1 - footer.len();
        assert!(
            lines[10..status].iter().all(|line| line.is_empty()),
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
        // The selection follows the row it started on, which a re-sort may move down.
        assert!(
            table
                .iter()
                .any(|row| highlighted_inventory_row(main, rows, cols, &row[0]).is_some()),
            "{screen}"
        );

        let screen = screen_text(search, rows, cols);
        let lines = screen.lines().collect::<Vec<_>>();
        assert!(lines[0].ends_with("SEARCH · 2 rows"), "{screen}");
        assert_eq!(lines[2], "  / alpha█", "{screen}");
        assert_eq!(inventory_rows(&screen).len(), 2, "{screen}");
        assert_eq!(
            lines[rows - 2],
            "  Enter / Esc retain search · action letters are text",
            "{screen}"
        );
        assert_eq!(
            lines[rows - 1],
            "  Enter accept  Esc cancel  Backspace delete",
            "{screen}"
        );

        let screen = screen_text(inspect, rows, cols);
        let lines = screen.split('\n').collect::<Vec<_>>();
        assert!(lines[0].ends_with("INSPECT · 2 rows"), "{screen}");
        assert_eq!(lines[rows - 2], "  No active operation", "{screen}");
        assert_eq!(lines[rows - 1], "", "{screen}");
        assert_eq!(
            styled_screen(inspect, rows, cols)[rows - 1]
                .iter()
                .all(|(_, reversed)| *reversed),
            !no_color,
            "under a popup the hint bar stays, empty:\n{screen}"
        );
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
        // A long value wraps at a space inside its value column; rejoin it to compare whole fields.
        let body = body.iter().fold(Vec::<String>::new(), |mut joined, line| {
            match joined.last_mut() {
                Some(previous) if line.starts_with(&" ".repeat(19)) && !line.trim().is_empty() => {
                    previous.push(' ');
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
            field("Declaration", "explicit · satisfied by ~/repos/org/alpha"),
            field("Checkout", "cloned"),
            field("Origin", "git@github.com:org/alpha.git"),
            field("Branch", "main"),
            field("Changes", "1 modified"),
            "j/k scroll · c copy · Esc close".to_owned(),
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
            body.contains(&"j/k scroll · c copy · Esc close"),
            "{screen}"
        );

        let screen = screen_text(help, rows, cols);
        let lines = screen.split('\n').collect::<Vec<_>>();
        assert!(lines[0].ends_with("HELP · 2 rows"), "{screen}");
        assert_eq!(lines[rows - 2], "  No active operation", "{screen}");
        assert_eq!(lines[rows - 1], "", "{screen}");
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
            .take_while(|line| !line.is_empty() && !line.starts_with("j/k scroll"))
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
        assert!(body.contains(&"j/k scroll · Esc close"), "{screen}");

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
fn inventory_shows_the_first_folder_below_the_root_as_the_owner() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &root.join("org/alpha"),
        "git@github.com:other/alpha.git",
        true,
    );
    // The provider's grouping (here `group/sub`) is not the owner.
    init_repo(
        &root.join("imfusion/cp/repo"),
        "git@gitlab.com:group/sub/repo.git",
        true,
    );
    init_repo(
        &root.join("black-atom-industries/lager"),
        "git@github.com:black-atom-industries/lager.git",
        true,
    );
    init_repo_without_origin(&root.join("scratch"));

    // 113 columns is the narrowest terminal that shows the owner beside every other column.
    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config)
            .arg("tui")
            .env("NO_COLOR", "1"),
        &["NORMAL · 4 rows", "absent: Scanning"],
        b"q",
        24,
        113,
    );
    let screen = screen_text(&transcript, 24, 113);
    let (headings, rows) = inventory_grid(&screen);
    assert_eq!(headings[..2], ["repository", "owner"], "{screen}");
    let owner = |name: &str| {
        rows.iter()
            .find(|row| row[0] == name)
            .map(|row| row[1].clone())
            .unwrap_or_else(|| panic!("no row {name}:\n{screen}"))
    };
    assert_eq!(owner("alpha"), "org", "{screen}");
    assert_eq!(owner("repo"), "imfusion", "{screen}");
    assert_eq!(owner("scratch"), "—", "{screen}");
    let long = owner("lager");
    assert!(
        long.starts_with("black-atom-") && long.ends_with('…') && long.chars().count() <= 16,
        "an owner keeps its head: {long:?}\n{screen}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_and_status_order_rows_by_owner_then_name_while_json_keeps_key_order() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    // Owners and identities disagree on purpose: `Beta` sorts before `alpha` byte-wise.
    for (path, origin) in [
        ("gamma/kiwi", "git@github.com:aaa/kiwi.git"),
        ("Beta/pear", "git@github.com:x/pear.git"),
        ("alpha/mango", "git@github.com:zzz/mango.git"),
        ("Beta/apple", "git@github.com:x/apple.git"),
    ] {
        init_repo(&root.join(path), origin, true);
    }
    init_repo_without_origin(&root.join("scratch"));
    let expected = ["mango", "apple", "pear", "kiwi", "scratch"];

    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config)
            .arg("tui")
            .env("NO_COLOR", "1"),
        &["NORMAL · 5 rows", "absent: Scanning"],
        b"q",
        24,
        132,
    );
    let screen = screen_text(&transcript, 24, 132);
    let (_, rows) = inventory_grid(&screen);
    let names = rows.iter().map(|row| row[0].as_str()).collect::<Vec<_>>();
    assert_eq!(names, expected, "{screen}");
    let owners = rows.iter().map(|row| row[1].as_str()).collect::<Vec<_>>();
    assert_eq!(owners, ["alpha", "Beta", "Beta", "gamma", "—"], "{screen}");

    let status = output(support::lager(&home, &config).arg("status"));
    assert!(status.status.success(), "{status:?}");
    let printed = status_rows(&String::from_utf8(status.stdout).unwrap());
    let names = printed
        .iter()
        .map(|row| row[0].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, expected, "{printed:?}");

    let json = output(support::lager(&home, &config).args(["status", "--json"]));
    assert!(json.status.success(), "{json:?}");
    let document: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let identities = document["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["identity"].as_str().unwrap_or("-").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            "github.com/aaa/kiwi",
            "github.com/x/apple",
            "github.com/x/pear",
            "github.com/zzz/mango",
            "-"
        ]
    );
}

#[cfg(unix)]
#[test]
fn inventory_shows_the_widest_column_set_that_fits_the_terminal() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    let full = [
        "repository",
        "owner",
        "local",
        "branch",
        "status",
        "registration",
    ];
    for (cols, columns) in [
        (113, &full[..]),
        (112, &full[..5]),
        (99, &full[..5]),
        (98, &["repository", "branch", "status"][..]),
    ] {
        let transcript = run_inventory_pty_with_size(
            support::lager(&home, &config)
                .arg("tui")
                .env("NO_COLOR", "1"),
            &["NORMAL · 1 row", "absent: Scanning"],
            b"q",
            24,
            cols,
        );
        let screen = screen_text(&transcript, 24, cols as usize);
        let (headings, _) = inventory_grid(&screen);
        assert_eq!(headings, columns, "{cols} columns:\n{screen}");
        // A hidden local column is the only reason for the path and origin hint.
        assert_eq!(
            screen.contains("Path and origin: Enter inspects"),
            !columns.contains(&"local"),
            "{cols} columns:\n{screen}"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_duplicate_and_same_named_rows_distinct_at_both_sizes() {
    let temp = temp_dir();
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
        "a-very-long-backup-directory-name-that-keeps-going/alpha",
        "a-very-long-backup-directory-name-that-keeps-going/older/alpha",
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
            .map(|row| row[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(lone.len(), 2, "{screen}");
        if cols >= 132 {
            assert!(
                lone.contains(&"missing") && lone.iter().any(|local| local.ends_with("/lone")),
                "{screen}"
            );
        } else {
            // Without the local column the copies differ by their branch.
            assert!(lone.contains(&"—") && lone.contains(&"main"), "{screen}");
        }
        if cols >= 132 {
            let path = |name: &str| {
                rows.iter()
                    .find(|row| row[0] == name)
                    .map(|row| row[1].clone())
                    .unwrap()
            };
            let (first, second) = (path("alpha (copy)"), path("alpha (copy 2)"));
            assert!(
                first.starts_with('…') && first.ends_with("keeps-going/alpha"),
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
fn inventory_keeps_long_named_copies_and_cross_host_rows_distinct_in_the_narrow_grid() {
    let temp = temp_dir();
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
    // 66 columns leave the repository column 28 cells of text in the narrow grid.
    let transcript = run_inventory_pty_with_size(
        &mut command,
        &["NORMAL · 5 rows", "(copy 2)", "absent: Scanning"],
        b"q",
        24,
        66,
    );
    let screen = screen_text(&transcript, 24, 66);
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
    assert!(names.contains(&"github.com/org…hared-tooling"), "{screen}");
    assert!(names.contains(&"gitlab.com/org…hared-tooling"), "{screen}");
}

#[cfg(unix)]
#[test]
fn inventory_root_menu_uses_effective_hints_and_preserves_search_and_selection() {
    let temp = temp_dir();
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
        26,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_search_with_a_slash_matches_local_paths_and_keeps_text_and_filters_safe() {
    let temp = temp_dir();
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
            (vec!["alpha", "beta"], b"/hdn/".to_vec()),
            (
                vec!["SEARCH · 1 row", "/ hdn/", "alpha", "excluded"],
                b"raRq".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ hdn/raRq", "No matching repositories"],
                b"\x7f\x7f\x7f\x7f".to_vec(),
            ),
            (
                vec!["SEARCH ·", "/ hdn/", "selected: alpha"],
                b"\r".to_vec(),
            ),
            (vec!["NORMAL ·", "/ hdn/", "selected: alpha"], b"/".to_vec()),
            (
                vec!["SEARCH ·", "/ hdn/", "selected: alpha"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "/ hdn/"], b"\x7f".to_vec()),
            (
                vec!["NORMAL · 2 rows", "/ Search repositories", "beta"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_search_ranks_name_matches_first_and_ignores_status() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/aid/.github\"\n\n[[repositories]]\nurl = \"github.com/aid/node-tests\"\n\n[[repositories]]\nurl = \"github.com/aid/notes\"\n",
    )
    .unwrap();

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["No active operation"], b"/missing\r".to_vec()),
            (
                vec!["NORMAL · 0 rows", "/ missing", "No matching repositories"],
                b"\x7f/notes\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "/ notes", "selected: notes"],
                b"q".to_vec(),
            ),
        ],
        24,
        220,
    );
    let screen = screen_text(&transcript, 24, 220);
    let rows = inventory_rows(&screen);
    let names = rows.iter().map(|row| row[0].as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["notes", "node-tests"], "{screen}");
}

#[cfg(unix)]
#[test]
fn inventory_filter_pulls_the_scrolled_table_back_to_the_first_match() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    let mut text = String::from("root = \"repos\"\n");
    for name in (1..=30)
        .map(|index| format!("r{index:02}"))
        .chain(["zphqxjvk1".to_owned(), "zphqxjvk2".to_owned()])
    {
        text.push_str(&format!(
            "\n[[repositories]]\nurl = \"github.com/aid/{name}\"\n"
        ));
    }
    fs::write(&config, text).unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL ·", "r01"], "j".repeat(60).into_bytes()),
            (vec!["zphqxjvk2"], b"/zphqxjvk\r".to_vec()),
            (
                vec!["NORMAL · 2 rows", "zphqxjvk1", "zphqxjvk2"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[test]
fn tui_is_the_only_command_that_opens_the_inventory() {
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
        let temp = temp_dir();
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
    let temp = temp_dir();
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
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter().any(|row| row[1].ends_with(" · unknown")),
        "{rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row[1].contains("conflict")),
        "{rows:?}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_refresh_reconciles_deleted_checkouts_without_turning_incomplete_into_missing() {
    for incomplete in [false, true] {
        let temp = temp_dir();
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
                            "Scan incomplete:"
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
        if incomplete {
            assert!(row[1].ends_with(" · unreadable"), "{rows:?}");
        } else {
            assert_eq!(row[1], "missing", "{rows:?}");
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_bare_probe_failure_is_partial_even_when_stderr_says_cancelled() {
    let temp = temp_dir();
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
                "NORMAL · 0 rows · 1 problem",
                "! Scan incomplete: ~/repos/bare.git cancelled",
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
            let temp = temp_dir();
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
            // A failed origin probe shows only in the inspect panel; the other failures
            // show in the grid.
            let in_grid = operation != "remote";
            let mut actions = if in_grid {
                vec![(vec!["injected probe failure"], b"R".to_vec())]
            } else {
                vec![
                    (vec!["checkout", "1 problem"], b"\r".to_vec()),
                    (
                        vec!["Inspect / checkout", "injected probe failure"],
                        b"\x1b".to_vec(),
                    ),
                    (vec!["NORMAL ·", "absent: Inspect /"], b"R".to_vec()),
                ]
            };
            let refreshing = actions.len() - 1;
            if refresh {
                actions.push((
                    vec![
                        "absent: injected probe failure",
                        "No active operation",
                        "clean",
                        "main",
                    ],
                    b"q".to_vec(),
                ));
            } else {
                actions[refreshing].1 = b"q".to_vec();
            }
            let transcript = run_inventory_pty_with_exit(
                support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
                &actions,
                24,
                300,
                1,
                |action| {
                    if refresh && action == refreshing {
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
        let temp = temp_dir();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        let root = home.join("repos");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = RestoreMode(root.clone());
        fs::write(&config, "root = \"repos\"\n").unwrap();
        let actions = if refresh {
            vec![
                (
                    vec!["Scan incomplete:", "scan is incomplete"],
                    b"R".to_vec(),
                ),
                (
                    vec!["absent: Scan incomplete:", "Scan complete · 0 repositories"],
                    b"q".to_vec(),
                ),
            ]
        } else {
            vec![(
                vec!["Scan incomplete:", "scan is incomplete"],
                b"q".to_vec(),
            )]
        };
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config).arg("tui"),
            &actions,
            24,
            180,
            1,
            |action| {
                if refresh && action == 0 {
                    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
                }
            },
        );
        if refresh {
            assert!(!screen_text(&transcript, 24, 180).contains("Scan incomplete:"));
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_missing_root_is_empty_with_a_note_until_it_exists_and_exits_zero() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec![
                    "NORMAL · 0 rows",
                    "~/repos does not exist yet",
                    "absent: problem",
                    "absent: Scan incomplete",
                ],
                b"R".to_vec(),
            ),
            (
                vec!["NORMAL · 0 rows", "absent: does not exist yet"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
        0,
        |action| {
            if action == 0 {
                fs::create_dir_all(home.join("repos")).unwrap();
            }
        },
    );
    assert!(!screen_text(&transcript, 24, 80).contains("does not exist yet"));

    fs::remove_dir(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[(
            vec![
                "NORMAL · 1 row",
                "  ~/repos does not exist yet",
                "missing",
                "absent: problem",
            ],
            b"q".to_vec(),
        )],
        24,
        100,
        0,
        |_| {},
    );
}

/// The Empty and Partial boards: an inventory with nothing in it, and one whose scan is still
/// running behind an unreadable directory while Git status is pending.
#[cfg(unix)]
#[test]
fn inventory_empty_and_partial_states_follow_the_canvas_at_both_sizes() {
    for (rows, cols) in [(32u16, 132u16), (24, 80)] {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(home.join("repos")).unwrap();
        fs::write(&config, "root = \"repos\"\n").unwrap();
        let mut frame = String::new();
        run_inventory_pty_with_exit_observing(
            support::lager(&home, &config).arg("tui"),
            &[(vec!["NORMAL · 0 rows", "Scan complete"], b"q".to_vec())],
            rows,
            cols,
            0,
            |_, _, screen| frame = screen.to_owned(),
        );
        let screen = screen_text(&frame, rows.into(), cols.into());
        let lines = screen.lines().collect::<Vec<_>>();
        let (rows, cols) = (usize::from(rows), usize::from(cols));
        assert!(
            lines[0].starts_with("  LAGER / inventory") && lines[0].ends_with("NORMAL · 0 rows"),
            "{screen}"
        );
        assert_eq!(lines[1], "  Local + declarations · remote off", "{screen}");
        assert_eq!(
            lines[2], "  / Search repositories, or paths with /…",
            "{screen}"
        );
        assert!(lines[4].trim_start().starts_with("repository"), "{screen}");
        assert_eq!(lines[6], "  No repositories yet.", "{screen}");
        assert_eq!(
            lines[8], "  ~/repos holds no Git repositories and the configuration declares none.",
            "{screen}"
        );
        assert_eq!(
            lines[9],
            "  a adds one by reference or from your providers. Nothing has been contacted.",
            "{screen}"
        );
        assert_eq!(
            lines[rows - 2],
            "  Scan complete · 0 repositories · 0 declarations",
            "{screen}"
        );
        assert_eq!(
            lines[rows - 1],
            "  a add  R refresh  m menu  ? help  q quit",
            "{screen}"
        );

        let root = home.join("repos");
        init_repo(
            &root.join("org/alpha"),
            "git@github.com:org/alpha.git",
            true,
        );
        run_git(
            temp.path(),
            &[
                "init",
                "-q",
                "--bare",
                root.join("mirror.git").to_str().unwrap(),
            ],
        );
        let private = root.join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = RestoreMode(private);
        fs::write(
            &config,
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
        )
        .unwrap();
        let tools = temp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(
            tools.join("git"),
            format!(
                "#!/bin/sh\n/bin/sleep 1\nexec {} \"$@\"\n",
                shell_word(support::real_tool("git").as_path())
            ),
        )
        .unwrap();
        fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let mut frames = Vec::new();
        run_inventory_pty_with_exit_observing(
            support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
            &[
                (
                    // Directory order decides whether `mirror.git` or `private` is listed first,
                    // and at 80 columns the second one is cut. Only `private` is a problem.
                    [
                        "· 1 problem",
                        "Local: incomplete, still scanning · status: ",
                        "! Scan incomplete: ",
                    ]
                    .into_iter()
                    .chain((cols >= 132).then_some("~/repos/private permission denied"))
                    .collect(),
                    Vec::new(),
                ),
                (
                    // At 80 columns the status line is cut; Problems lists the failure.
                    if cols < 132 {
                        vec!["Local: incomplete · remote off"]
                    } else {
                        vec![
                            "Local: incomplete · remote off",
                            "~/repos/mirror.git ignored: bare repository",
                        ]
                    },
                    b"q".to_vec(),
                ),
            ],
            rows as u16,
            cols as u16,
            1,
            |_, _, screen| frames.push(screen.to_owned()),
        );
        for (frame, source) in frames.iter().zip([
            "  Local: incomplete, still scanning · status: ",
            "  Local: incomplete · remote off",
        ]) {
            let screen = screen_text(frame, rows, cols);
            let lines = screen.lines().collect::<Vec<_>>();
            assert!(lines[0].ends_with("· 1 problem"), "{screen}");
            assert!(lines[1].starts_with(source), "{screen}");
            assert!(lines[2].starts_with("  / Search"), "{screen}");
            assert!(lines[4].trim_start().starts_with("repository"), "{screen}");
            assert!(
                lines[rows - 3].starts_with("  ! Scan incomplete: "),
                "{screen}"
            );
            assert!(lines[rows - 1].contains("q quit"), "{screen}");
        }
        let pending = screen_text(&frames[0], rows, cols);
        assert!(
            pending
                .lines()
                .nth(1)
                .unwrap()
                .ends_with("pending · remote off"),
            "{pending}"
        );
        assert!(pending.contains("pending"), "{pending}");
    }
}

#[cfg(unix)]
#[test]
fn inventory_root_that_is_a_file_is_a_session_problem() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("repos"), "not a directory\n").unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[(
            vec!["NORMAL · 0 rows · 1 problem", "root is not a directory"],
            b"q".to_vec(),
        )],
        24,
        80,
        1,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_starts_offline_and_rejects_archives_without_remote_before_raw_mode() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    {
        let name = "tui";
        let help = output(support::lager(&home, &config).args([name, "--help"]));
        let help = String::from_utf8_lossy(&help.stdout).into_owned();
        for line in [
            "Start with remote discovery on and fetch the configured providers",
            "Include archived repositories; requires --remote",
        ] {
            assert!(help.contains(line), "{help}");
        }
        support::clear_outbound_requests();
        let transcript = run_inventory_pty_with_size(
            support::lager(&home, &config).arg(name),
            &[
                "Scan complete · 0 repositories · 0 declarations",
                "No repositories yet.",
            ],
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
        assert!(screen_has_pair(&plain, "a add"), "{plain}");
        for unavailable in ["all actions", "Add to", "Navigation is ready"] {
            assert!(!plain.contains(unavailable), "{plain}");
        }
        assert!(support::outbound_requests().is_empty());
        let transcript = run_inventory_pty_with_exit(
            support::lager(&home, &config)
                .arg(name)
                .arg("--include-archived"),
            &[],
            24,
            132,
            2,
            |_| {},
        );
        assert!(
            transcript.contains("--include-archived requires --remote"),
            "{transcript}"
        );
        assert!(!transcript.contains("\u{1b}[?1049h"));
        assert!(support::outbound_requests().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn inventory_keeps_explicit_registration_wildcard_unions_destination_uncertainty_and_origin_facts()
{
    let temp = temp_dir();
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
                vec!["5 rows", "No active operation"],
                b"/elsewhere\r\r".to_vec(),
            ),
            (
                vec![
                    "Inspect / managed",
                    "elsewhere/managed · nonstandard",
                    "absent: misplaced checkout",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"\x7f".to_vec()),
            (vec!["NORMAL · 5 rows"], b"/relative\r\r".to_vec()),
            (
                vec![
                    "Inspect / relative-origin",
                    "pair: Origin ../relative-origin",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"\x7f".to_vec()),
            (vec!["NORMAL · 5 rows"], b"/sub/\r\r".to_vec()),
            (
                vec![
                    "Inspect / repo",
                    "pair: Identity github.com/group/sub/repo",
                    "pair: Origin git@github.com:group/sub/repo.git",
                ],
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

    for expected in ["explicit", "elsewhere/managed", "unknown", "wildcard"] {
        assert!(plain.contains(expected), "missing {expected}:\n{plain}");
    }
    let rows = inventory_rows(&plain);
    let missing = rows
        .iter()
        .find(|row| row[0] == "missing")
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert_ne!(
        missing[1], "missing",
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
        let temp = temp_dir();
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
                    vec!["No active operation", "~/repos/group/sub/repo"],
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
        assert_eq!(row[2], "main");
        assert_eq!(row[3], "clean");
        assert_eq!(row[4], "wildcard");
    }
}

#[cfg(unix)]
#[test]
fn inventory_shows_a_discovered_checkout_pending_before_a_delayed_probe_completes() {
    let temp = temp_dir();
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
    let temp = temp_dir();
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
            (vec!["fresh", "missing"], b"q".to_vec()),
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
    let rows = inventory_rows(&plain);
    assert!(
        rows.iter().any(|row| row[0] == "fresh"),
        "refresh did not install the new config:\n{plain}"
    );
    assert!(
        rows.iter().all(|row| row[0] == "fresh") && !plain.contains("delayed"),
        "stale result from the prior config generation was accepted:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_refresh_keeps_completed_observations_stale_until_reprobed() {
    let temp = temp_dir();
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
            (vec!["~/repos/org/stale", "main", "clean"], b"R".to_vec()),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
            (
                vec!["changed-target", "· unknown", "stale: target"],
                b"\r".to_vec(),
            ),
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
            && !inspect.contains("old-origin"),
        "changed target remained forever pending instead of becoming stale:\n{inspect}"
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(
        !plain.contains("old-origin"),
        "stale probe result was accepted after target replacement:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_rejects_a_probe_result_after_in_place_origin_config_change() {
    let temp = temp_dir();
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
            (vec!["origin-changed", "stale: target"], b"\r".to_vec()),
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
        !inspect.contains("old-origin"),
        "stale probe result was accepted after origin config changed:\n{inspect}"
    );
    let plain = screen_text(&transcript, 24, 132);
    assert!(
        !plain.contains("old-origin"),
        "stale probe result was accepted after origin config changed:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_declared_rows_keep_wrong_corrupt_and_unreadable_facts_distinct() {
    let temp = temp_dir();
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

    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["other/wrong", "org/corrupt", "unreadable", "error: git"],
                b"/corrupt\rj\r".to_vec(),
            ),
            (
                vec!["Inspect / org/corrupt", "origin error:"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"\x7f".to_vec()),
            (vec!["NORMAL · 5 rows"], b"/other\r\r".to_vec()),
            (
                vec![
                    "Inspect / other/wrong",
                    "pair: Identity github.com/other/wrong",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
        ],
        24,
        500,
        1,
        |action, _, frame| {
            if action == 0 {
                frames.push(frame.to_owned());
            }
        },
    );
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    let rows = inventory_rows(&screen_text(&frames[0], 24, 500));
    let wrong_row = rows
        .iter()
        .find(|row| row[0] == "other/wrong")
        .unwrap_or_else(|| panic!("wrong-origin checkout row; parsed rows: {rows:?}"));
    assert_eq!(wrong_row[1], "~/repos/org/wrong");
    assert_eq!(wrong_row[4], "unregistered");
    let declared_wrong = rows
        .iter()
        .find(|row| row[0] == "org/wrong")
        .unwrap_or_else(|| panic!("declared wrong-origin row; parsed rows: {rows:?}"));
    assert!(declared_wrong[1].ends_with(" · conflict"), "{rows:?}");
    let unreadable_row = rows
        .iter()
        .find(|row| row[0].ends_with("unreadable"))
        .unwrap_or_else(|| panic!("declared unreadable row; parsed rows: {rows:?}"));
    assert!(unreadable_row[1].ends_with(" · unreadable"), "{rows:?}");
    let corrupt_row = rows
        .iter()
        .find(|row| row[0] == "org/corrupt" && row[2].starts_with("error:"))
        .unwrap_or_else(|| panic!("corrupt-origin checkout row; parsed rows: {rows:?}"));
    assert_eq!(corrupt_row[1], "~/repos/org/corrupt");
}

#[cfg(unix)]
#[test]
fn inventory_keeps_partial_summary_visible_when_rows_overflow_the_viewport() {
    let temp = temp_dir();
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
        &[(vec!["· 30 rows", "Scan incomplete:"], b"q".to_vec())],
        12,
        220,
        1,
        |_| {},
    );
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();
    let plain = screen_text(&transcript, 12, 220);
    assert!(
        plain.contains("Scan incomplete:"),
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
    let temp = temp_dir();
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
        !plain.contains("Scan incomplete:"),
        "exclusions became partial:\n{plain}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_excludes_git_file_entries_reports_unreadable_paths_escapes_controls_and_restores_terminal()
 {
    let temp = temp_dir();
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
                "ignored: linked worktree or submodule",
                "reachable-submodule",
                "linked-worktree",
                "Scan incomplete:",
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
        plain.contains("ignored: linked worktree or submodule")
            && plain.contains("reachable-submodule"),
        "reachable submodule was not independently excluded:\n{plain}"
    );
    assert!(
        plain.contains("linked-worktree"),
        "linked worktree was not independently excluded:\n{plain}"
    );
    assert!(
        plain.contains("Scan incomplete:"),
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
fn status_prints_the_rows_the_inventory_screen_shows() {
    let temp = temp_dir();
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
    init_repo(
        &root.join("other/stray"),
        "git@github.com:other/stray.git",
        true,
    );
    init_repo_without_origin(&root.join("local/no-origin"));

    let transcript = run_inventory_pty_with_size(
        support::lager(&home, &config).arg("tui"),
        &["7 rows", "No active operation"],
        b"q",
        40,
        220,
    );
    let (_, screen) = inventory_grid(&screen_text(&transcript, 40, 220));
    let status = output(support::lager(&home, &config).arg("status"));
    assert!(status.status.success(), "{status:?}");
    let printed = status_rows(&String::from_utf8(status.stdout).unwrap());
    assert_eq!(printed.len(), 7, "{printed:?}");
    assert_eq!(printed, screen);
    assert!(printed.iter().all(|row| row[0] != "org/*"), "{printed:?}");
}

/// Splits `lager status` table lines at the header's column starts.
fn status_rows(table: &str) -> Vec<Vec<String>> {
    let mut lines = table.lines();
    let header = lines.next().unwrap();
    let mut offsets = Vec::new();
    let mut from = 0;
    for heading in header.split_whitespace() {
        let start = header[from..].find(heading).unwrap() + from;
        offsets.push(start);
        from = start + heading.len();
    }
    lines
        .map(|line| {
            let characters = line.chars().collect::<Vec<_>>();
            (0..offsets.len())
                .map(|column| {
                    let end = offsets.get(column + 1).copied().unwrap_or(characters.len());
                    characters[offsets[column].min(characters.len())..end.min(characters.len())]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_owned()
                })
                .collect()
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn offline_inventory_renders_discovery_identity_and_status_rows_in_a_pty() {
    let temp = temp_dir();
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
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec![
                    "7 rows",
                    "ignored repositories: bare.git: bare repository ignored",
                    "No active operation",
                ],
                b"/no-origin\r\r".to_vec(),
            ),
            (vec!["Inspect / no-origin", "No origin"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "absent: Inspect /"], b"\x7f".to_vec()),
            (vec!["NORMAL · 7 rows"], b"/extra\r\r".to_vec()),
            (
                vec![
                    "Inspect / extra",
                    "pair: Identity github.com/org/extra",
                    "pair: Origin git@github.com:org/extra.git",
                ],
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

    for header in [
        "repository",
        "owner",
        "local",
        "branch",
        "status",
        "registration",
    ] {
        assert!(plain.contains(header), "missing header {header}:\n{plain}");
    }
    let expected = "ignored repositories: bare.git: bare repository ignored";
    assert!(plain.contains(expected), "missing {expected}:\n{plain}");
    let rows = inventory_rows(&plain);
    let assert_row = |label: &str, expected: [&str; 5]| {
        assert!(
            rows.iter()
                .any(|row| { [0, 1, 4, 2, 3].map(|i| row[i].as_str()) == expected }),
            "missing exact {label} row {expected:?}; parsed rows: {rows:?}"
        );
    };
    assert!(rows.iter().all(|row| row[0] != "org/*"), "{rows:?}");
    assert_row(
        "missing declaration",
        ["missing", "missing", "explicit", "—", "—"],
    );
    assert_row(
        "exact managed checkout",
        [
            "managed",
            "~/repos/org/managed",
            "explicit",
            "main",
            "clean",
        ],
    );
    assert_row(
        "misplaced duplicate managed checkout",
        [
            "managed (copy)",
            "~/repos/elsewhere/managed",
            "explicit",
            "main",
            "clean",
        ],
    );
    assert_row(
        "wildcard checkout",
        [
            "extra",
            "~/repos/org/extra",
            "wildcard",
            "main",
            "1 modified",
        ],
    );
    assert_row(
        "excluded checkout",
        ["skip", "~/repos/org/skip", "excluded", "main", "clean"],
    );
    assert_row(
        "originless checkout",
        [
            "no-origin",
            "~/repos/local/no-origin",
            "unregistered",
            "main",
            "clean",
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
    let temp = temp_dir();
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
        for text in ["branch", "status", "—"] {
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
        assert_eq!(selected.find("—"), adjacent.find("—"));
        assert!(transcript.contains("\x1b[?1049l"));
        assert!(transcript.contains("\x1b[?25h"));
    }
}

#[cfg(unix)]
#[test]
fn inventory_resizes_from_wide_to_compact_without_losing_whole_row_selection() {
    let temp = temp_dir();
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
                    "local",
                    "branch",
                    "registration",
                    "status",
                    "selected: alpha",
                    "No active operation",
                ],
                b"j".to_vec(),
            ),
            (vec!["selected: beta", "branch", "status"], b"q".to_vec()),
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
            "owner",
            "local",
            "branch",
            "status",
            "registration"
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
    let compact_header = compact
        .lines()
        .find(|line| line.trim_start().starts_with("repository"))
        .unwrap();
    assert_eq!(
        compact_header.split_whitespace().collect::<Vec<_>>(),
        ["repository", "branch", "status"],
        "{compact}"
    );
    let selected = highlighted_inventory_row(compact_frame, 24, 80, "beta").unwrap();
    let adjacent = compact.lines().find(|line| line.contains("alpha")).unwrap();
    assert_eq!(
        selected.find("—"),
        adjacent.find("—"),
        "selection shifted a compact table column:\n{compact}"
    );
    assert!(transcript.contains("\x1b[?1049l") && transcript.contains("\x1b[?25h"));
}

#[cfg(unix)]
#[test]
fn inventory_navigation_remains_responsive_while_status_is_pending() {
    let temp = temp_dir();
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
            // Rows run org/alpha twice, then org/beta twice: the top, then two down.
            (vec!["alpha", "beta", "pending"], b"kkjj".to_vec()),
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
    let temp = temp_dir();
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
                    "Scan complete · 0 repositories",
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
            (
                vec!["Scan complete · 0 repositories", "pair: ? help"],
                b"?".to_vec(),
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
        13,
        100,
        0,
        |action, writer, frame| match action {
            4 => {
                frames.push(frame.to_owned());
                resize_pty(writer, 40, 100);
            }
            5 => frames.push(frame.to_owned()),
            _ => {}
        },
    );

    let [scrolled_frame, resized_frame] = frames.as_slice() else {
        panic!("expected a scrolled and resized help frame: {frames:?}");
    };
    let scrolled = screen_text(scrolled_frame, 13, 100);
    assert!(scrolled.contains("? / q/Esc"), "{scrolled}");
    assert!(
        !scrolled.contains("j/k ↑/↓"),
        "custom page-down did not scroll help: {scrolled}"
    );

    let resized = screen_text(resized_frame, 40, 100);
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
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&root.join("a-ready"), "git@github.com:org/zzz.git", true);
    // The pending row sorts after `zzz` by its path until its identity `aaa` arrives.
    let pending = root.join("zzzz-pending");
    init_repo(&pending, "git@github.com:org/aaa.git", true);
    let tools = temp.path().join("tools");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), format!(
        "#!/bin/sh\ncase \"$*\" in *zzzz-pending*) while [ ! -f {} ]; do /bin/sleep 0.02; done;; esac\nexec {} \"$@\"\n",
        shell_word(&release), shell_word(&support::real_tool("git"))
    )).unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let selected_pending = "selected: zzzz-pending".to_owned();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["selected: zzz", "pending", "zzzz-pending"],
                b"j".to_vec(),
            ),
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
fn inventory_cursor_stays_on_the_top_row_while_the_scan_re_sorts_until_the_user_moves() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(&root.join("a-ready"), "git@github.com:org/zzz.git", true);
    init_repo(
        &root.join("zzzz-pending"),
        "git@github.com:org/aaa.git",
        true,
    );
    let tools = temp.path().join("tools");
    let release = temp.path().join("release");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("git"), format!(
        "#!/bin/sh\ncase \"$*\" in *zzzz-pending*) while [ ! -f {} ]; do /bin/sleep 0.02; done;; esac\nexec {} \"$@\"\n",
        shell_word(&release), shell_word(&support::real_tool("git"))
    )).unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    // The pending row is named by its path until its identity `aaa` arrives and sorts above
    // `zzz`; nothing is pressed, so the cursor moves with the top row.
    let transcript = run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["selected: zzz", "pending", "zzzz-pending"], Vec::new()),
            (vec!["selected: aaa", "No active operation"], b"q".to_vec()),
        ],
        24,
        500,
        |index| {
            if index == 0 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
    let (_, rows) = inventory_grid(&screen_text(&transcript, 24, 500));
    let names = rows.iter().map(|row| row[0].as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["aaa", "zzz"]);
}

#[cfg(unix)]
#[test]
fn inventory_validates_all_binding_modes_before_entering_raw_mode() {
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                vec!["No repositories yet.", "pair: x refresh"],
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
    let temp = temp_dir();
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
                b"/~/repos/\rkk".to_vec(),
            ),
            (
                vec!["NORMAL · 3 rows", "/ ~/repos/", "selected: same"],
                b"R".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 3 rows",
                    "/ ~/repos/",
                    "absent: No active operation",
                ],
                Vec::new(),
            ),
            (
                vec!["No active operation", "explicit", "selected: same"],
                b"R".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 3 rows",
                    "/ ~/repos/",
                    "absent: No active operation",
                ],
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
            screen.contains("NORMAL · 3 rows") && screen.contains("  / ~/repos/"),
            "search did not survive refresh: {screen}"
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
    // `same` has the owner `org` and `alpha` has none, in the declaration and checkout form alike.
    for rows in [&declared_rows, &checkout_rows] {
        assert!(
            row_position(rows, &configured) < row_position(rows, &root.join("alpha")),
            "owner order changed with the row form: {rows:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_terminal_equivalent_keys_share_reachability_and_collision_checks() {
    let temp = temp_dir();
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
        let temp = temp_dir();
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
    let temp = temp_dir();
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
                b"jjjjjjjjjj\r".to_vec(),
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
    let temp = temp_dir();
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
                vec!["All actions", "j/k select · Enter choose · Esc back"],
                b"j\r".to_vec(),
            ),
            (
                vec!["Inspect / alpha", "j/k scroll · c copy · Esc close"],
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
fn inventory_menu_holds_its_entries_and_one_hint_row_over_an_empty_hint_bar() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    for (rows, cols) in [(32, 132), (24, 80)] {
        let mut menu = String::new();
        run_inventory_pty_with_exit_observing(
            support::lager(&home, &config).arg("tui"),
            &[
                (vec!["NORMAL · 1 row", "absent: Scanning"], b"m".to_vec()),
                (
                    vec![
                        "All actions",
                        "j/k select · Enter choose · Esc back",
                        "absent: q quit",
                    ],
                    b"\x1b".to_vec(),
                ),
                (vec!["NORMAL", "absent: All actions"], b"q".to_vec()),
            ],
            rows,
            cols,
            0,
            |action, _, frame| {
                if action == 1 {
                    menu = frame.to_owned();
                }
            },
        );
        let (rows, cols) = (rows as usize, cols as usize);
        let screen = screen_text(&menu, rows, cols);
        let lines = screen.split('\n').collect::<Vec<_>>();
        let body = lines
            .iter()
            .map(|line| line.split('│').nth(1).unwrap_or_default().trim())
            .collect::<Vec<_>>();
        for line in [
            "Repository actions…",
            "Selection…",
            "j/k select · Enter choose · Esc back",
        ] {
            assert!(body.contains(&line), "{line}:\n{screen}");
        }
        assert_eq!(lines[rows - 1], "", "{screen}");
        assert!(
            styled_screen(&menu, rows, cols)[rows - 1]
                .iter()
                .all(|(_, reversed)| *reversed),
            "the empty hint bar stays a bar:\n{screen}"
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_help_follows_its_board_with_one_hint_row_over_an_empty_hint_bar() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let mut help = String::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["Scan complete · 0 repositories"], b"?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Other modes",
                    "j/k scroll · Esc close",
                    "absent: q quit",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"q".to_vec()),
        ],
        35,
        132,
        0,
        |action, _, frame| {
            if action == 1 {
                help = frame.to_owned();
            }
        },
    );
    let screen = screen_text(&help, 35, 132);
    let lines = screen.split('\n').collect::<Vec<_>>();
    let body = lines
        .iter()
        .filter_map(|line| line.split('│').nth(1))
        .map(|inner| inner.strip_prefix("  ").unwrap_or(inner).trim_end())
        .collect::<Vec<_>>();
    let at = |text: &str| {
        body.iter()
            .position(|line| line.starts_with(text))
            .unwrap_or_else(|| panic!("{text}:\n{screen}"))
    };
    // The keyless actions close the normal keys; a blank line separates the sections.
    assert_eq!(at("unbound"), at("? / q") + 1, "{screen}");
    assert!(
        body[at("unbound")].starts_with("unbound       Register, Unregister, "),
        "{screen}"
    );
    let other = at("Other modes");
    assert_eq!(body[other - 1], "", "{screen}");
    assert_eq!(
        body[other + 1..other + 9],
        [
            "search        Enter / Esc keep the search · letters are text",
            "menu          j/k select · Enter choose · Esc back one level",
            "input         Enter accept · Esc cancel",
            "confirm       j/k focus · Space acknowledge · Enter choose · Esc cancel",
            "inspect       j/k PageUp/PageDown scroll · c copy a value · Esc close",
            "worktrees     j/k select · a add · r remove · c copy path · Esc close",
            "filter        j/k move · Space toggle · c clear · Enter/Esc close",
            "j/k scroll · Esc close",
        ],
        "{screen}"
    );
    assert!(!screen.contains("No default key"), "{screen}");
    assert_eq!(lines[34], "", "{screen}");
    assert!(
        styled_screen(&help, 35, 132)[34]
            .iter()
            .all(|(_, reversed)| *reversed),
        "the empty hint bar stays a bar:\n{screen}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_help_uses_the_invoking_mode_and_returns_to_it() {
    let temp = temp_dir();
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
fn inventory_inspect_shows_no_actions_and_typed_action_keys_change_nothing() {
    let temp = temp_dir();
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
                    "pair: Changes clean",
                    "absent: available",
                    "absent: no-op",
                    "absent: removes the found path",
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
        let temp = temp_dir();
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
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["Scan complete · 0 repositories"], b"m".to_vec()),
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
    let temp = temp_dir();
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
                b"\x7f/~/repos/hidden/zephyr-target\r".to_vec(),
            ),
            (
                vec!["NORMAL ·", "/ ~/repos/hidden/zephyr-target", "1 row"],
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
fn inventory_during_a_scan_inspect_and_the_menu_keep_an_unobserved_destination_unknown() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let marker = temp.path().join("started");
    let release = temp.path().join("release");
    fs::create_dir_all(root.join("org/zphqxjvk")).unwrap();
    fs::write(root.join("org/zphqxjvk/notes"), "kept\n").unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"github.com/org/zphqxjvk\"\n",
    )
    .unwrap();
    init_repo(
        &root.join("hold/zephyr"),
        "git@github.com:org/zephyr.git",
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
            (vec!["pending"], b"/zphqxjvk".to_vec()),
            (vec!["SEARCH ·", "1 row"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "/ zphqxjvk"], b"\r".to_vec()),
            (
                vec!["INSPECT", "pair: Checkout unknown", "absent: unavailable"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "/ zphqxjvk"], b"m\r".to_vec()),
            (
                vec![
                    "Repository actions / zphqxjvk",
                    "checkout state is unknown until it is observed",
                    "absent: holds no observed checkout",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: All actions"], b"r".to_vec()),
            (
                vec!["Remove only unavailable: checkout state is unknown until it is observed"],
                b"q".to_vec(),
            ),
        ],
        40,
        132,
        |action| {
            if action == 0 {
                wait_for_file(&marker);
            }
            if action == 8 {
                fs::write(&release, "go").unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_dialog_overrides_refresh_atomically_and_keep_all_exit_routes_reachable() {
    let temp = temp_dir();
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
            (vec!["No active operation"], b"m".to_vec()),
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
        let temp = temp_dir();
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
            "Scan incomplete:"
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
    let temp = temp_dir();
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
                vec!["No active operation", "explicit", "missing"],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"j\r".to_vec()),
            (vec!["Inspect / alpha"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], vec![17]),
        ],
        24,
        132,
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
    let temp = temp_dir();
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
            (vec!["Scan complete · 0 repositories"], b"?".to_vec()),
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

    let temp = temp_dir();
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
            (vec!["Path and origin:"], b"/org/beta\r".to_vec()),
            (
                vec!["NORMAL ·", "/ org/beta", "selected: beta"],
                b"m".to_vec(),
            ),
            (vec!["All actions"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (vec!["All actions", "absent: HELP"], b"q".to_vec()),
            (
                vec!["NORMAL ·", "/ org/beta", "selected: beta", "absent: MENU"],
                b"\r".to_vec(),
            ),
            (vec!["Inspect / beta"], b"?".to_vec()),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (vec!["Inspect / beta", "absent: HELP"], b"q".to_vec()),
            (
                vec![
                    "NORMAL ·",
                    "/ org/beta",
                    "selected: beta",
                    "absent: INSPECT",
                ],
                b"?".to_vec(),
            ),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (
                vec!["NORMAL ·", "/ org/beta", "selected: beta", "absent: HELP"],
                b"/\x1bOP".to_vec(),
            ),
            (vec!["Keyboard / effective bindings"], b"q".to_vec()),
            (
                vec!["SEARCH ·", "/ org/beta", "absent: HELP"],
                b"q".to_vec(),
            ),
            (vec!["SEARCH ·", "/ org/betaq"], b"\x1b".to_vec()),
            (vec!["NORMAL ·", "/ org/betaq"], vec![127]),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                // The cursor may start on any row while the scan runs; clamp it to the top.
                b"kk ".to_vec(),
            ),
            (
                vec!["[x] alpha", "[ ] beta", "1 marked · 0 hidden"],
                b"jj ".to_vec(),
            ),
            (
                vec!["[x] gone", "2 marked · 0 hidden"],
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
                vec!["NORMAL · 2 rows", "absent: gone", "[x] alpha", "[ ] beta"],
                b"mjjjjjj\r".to_vec(),
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
                b"mjjjjjj\rjj\r".to_vec(),
            ),
            (
                vec!["Cleared 2 marks", "absent: marked ·", "absent: [x]"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 3 {
                fs::remove_dir_all(root.join("scratch")).unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_copy_value_sends_the_full_original_value_to_the_clipboard() {
    let temp = temp_dir();
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
    warm_clipboard_stub(&tools);

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
            (vec!["Inspect / alpha", "c copy · Esc close"], b"c".to_vec()),
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
    let temp = temp_dir();
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
        // The file appears with its content: written beside it, then moved into place.
        &format!(
            "printf '%s\\n' \"$@\" > {tmp} && /bin/mv {tmp} {}",
            shell_word(&opened),
            tmp = shell_word(&opened.with_extension("tmp"))
        ),
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
    let temp = temp_dir();
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
                    "absent: clone, register",
                    "pair: repository Enter",
                    "Copy value…",
                    "Open origin in browser",
                    "Worktrees…",
                    "pair: lazygit g",
                    "Selection · 0 marked",
                    "absent: marks the row",
                    "Inventory",
                    "Add repository",
                    "pair: repository a",
                    "pair: Filter… f",
                    "pair: discovery… off",
                    "pair: config R",
                    "pair: Problems… none",
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
                    // At 80 columns the exit routes stay in the popup.
                    "Space acknowledge · Enter choose · Esc cancel",
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
            (vec!["Worktrees / alpha", "WORKTREES"], b"\x1b".to_vec()),
            (vec!["NORMAL", "absent: Worktrees /"], root_entry(5)),
            (vec!["lazygit not found on PATH"], root_entry(6)),
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
                b"mjjjjjj\rjj\r".to_vec(),
            ),
            (
                vec!["Cleared 1 mark", "absent: marked ·"],
                b"mjjjjjj\rj\r".to_vec(),
            ),
            (
                vec!["Marked visible concrete rows", "1 marked · 0 hidden"],
                root_entry(7),
            ),
            (vec!["Add / URL or reference"], b"\x1b".to_vec()),
            (
                vec!["Add cancelled · nothing changed", "absent: Add /"],
                root_entry(8),
            ),
            (
                vec!["FILTER · 1 row", "absent: All actions"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Filter"], root_entry(9)),
            // Without a configured provider the submenu opens and explains itself.
            (
                vec![
                    "Remote discovery",
                    "Enable remote",
                    "no providers configured",
                    "[ ] Include archived",
                ],
                b"qq".to_vec(),
            ),
            (vec!["NORMAL", "absent: All actions"], root_entry(10)),
            (vec!["NORMAL", "absent: All actions"], root_entry(11)),
            (vec!["All actions", "pair: Problems… none"], b"j\r".to_vec()),
            (vec!["All actions", "pair: batch no"], b"j\r".to_vec()),
            (
                vec!["Keyboard / effective bindings", "Menu mode"],
                b"\x1b".to_vec(),
            ),
            (vec!["All actions", "absent: Keyboard"], b"j\r".to_vec()),
        ],
        26,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_session_problems_list_every_failure_behind_exit_one() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let root = home.join("repos");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = RestoreMode(root);
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    let problems = format!("m{}\r", "j".repeat(11)).into_bytes();
    let mut popup = String::new();

    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec![
                    "Scan incomplete:",
                    "NORMAL · 1 row · 1 problem",
                    "absent: Scanning",
                ],
                b"m".to_vec(),
            ),
            (
                vec!["All actions", "pair: Problems… 1"],
                problems[1..].to_vec(),
            ),
            (
                vec![
                    "PROBLEMS · 1 row · 1 problem",
                    "Problems / 1",
                    "pair: scan ~/repos:",
                    "~/repos: permission denied ·",
                    "j/k scroll · Esc close",
                    "absent: q quit",
                    "absent: Quit returns 1",
                    "absent: are not problems",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row · 1 problem", "absent: Problems /"],
                b"R".to_vec(),
            ),
            (
                vec!["refresh failed:", "NORMAL · 1 row · 2 problems"],
                problems.clone(),
            ),
            (
                vec![
                    "Problems / 2",
                    "pair: scan ~/repos:",
                    "pair: refresh inventory",
                    "previous configuration kept",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Problems /"], b"q".to_vec()),
        ],
        24,
        80,
        1,
        |action, _, frame| {
            if action == 2 {
                popup = frame.to_owned();
            }
            if action == 3 {
                fs::write(
                    &config,
                    "root = \"repos\"\n[inventory.keys.normal]\nquit = []\n",
                )
                .unwrap();
            }
        },
    );
    let screen = screen_text(&popup, 24, 80);
    assert_eq!(screen.split('\n').nth(23), Some(""), "{screen}");
    assert!(
        styled_screen(&popup, 24, 80)[23]
            .iter()
            .all(|(_, reversed)| *reversed),
        "the empty hint bar stays a bar:\n{screen}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_keeps_drawing_while_the_opener_and_clipboard_hang() {
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
fn inventory_direct_bindings_cover_marks_copy_open_and_remote_actions() {
    let temp = temp_dir();
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
        // The file appears with its content: written beside it, then moved into place.
        &format!(
            "printf '%s\\n' \"$@\" > {tmp} && /bin/mv {tmp} {}",
            shell_word(&opened),
            tmp = shell_word(&opened.with_extension("tmp"))
        ),
    );

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
            // Remote opens its submenu alone; retry and archives run their entries.
            (
                vec!["Remote discovery", "Enable remote", "absent: All actions"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Enable remote", "absent: All actions"],
                b"t".to_vec(),
            ),
            (
                vec![
                    "Retry failed providers unavailable: remote is off",
                    "absent: problem",
                ],
                b"v".to_vec(),
            ),
            (
                vec![
                    "Archived repositories included · applies when remote is on",
                    "absent: problem",
                ],
                b" ".to_vec(),
            ),
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
    let temp = temp_dir();
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
                    "pair: search Ctrl+a",
                    "Ctrl+a / Esc keep the search",
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                    "pair: unbound Register,",
                    "absent: No default key",
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["No repositories yet.", "Scan complete · 0 repositories"],
                b"?".to_vec(),
            ),
            (
                vec![
                    "Normal mode",
                    "pair: unbound Register,",
                    "Register, Unregister",
                    "Scan complete · 0 repositories",
                ],
                b"\x1b[6~".to_vec(),
            ),
            (
                vec![
                    "Other modes",
                    "pair: confirm j/k",
                    "absent: No default key",
                    "absent: No key in",
                    "Scan complete · 0 repositories",
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
    let temp = temp_dir();
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
                b"jjjjjjjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Problems /",
                    "refresh         [inventory.keys.normal] binds x to remove and to clone",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Problems /"], b"q".to_vec()),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                vec!["Inspect / local", "pair: Origin No", "absent: unavailable"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "absent: Inspect /", "selected: local"],
                b"kk j j /org/beta\r".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "3 marked · 2 hidden", "selected: beta"],
                b"m\r\r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM",
                    "Register / preview",
                    "absent: exact target",
                    "NO-OP alpha (hidden)",
                    "~/repos/org/alpha · github.com/org/alpha · identity is already registered",
                    "ELIGIBLE beta",
                    "~/repos/org/beta · github.com/org/beta",
                    "BLOCKED local (hidden)",
                    "~/repos/scratch/local · no identity · no supported origin observed",
                    "absent: default",
                    "Register 1 target",
                    "absent: stay unchanged",
                    "No active operation",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "j/k scroll · Esc return",
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
                format!("m{}\r", "j".repeat(11)).into_bytes(),
            ),
            (
                vec![
                    "Problems / 1",
                    "batch · local",
                    "Register blocked: no supported origin observed",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Problems /"], b"q".to_vec()),
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
fn inventory_preview_and_outcomes_name_hidden_targets_at_80_by_24() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n\n\
         [inventory.keys.normal]\nregister = [\"g\"]\n",
    )
    .unwrap();
    for index in 1..=14 {
        let name = format!("r{index:02}");
        init_repo(
            &root.join("org").join(&name),
            &format!("git@github.com:org/{name}.git"),
            true,
        );
    }
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 14 rows", "absent: Scanning"],
                [b"k".repeat(14), b" j".repeat(14)].concat(),
            ),
            (vec!["14 marked"], b"/r01\r".to_vec()),
            (
                vec!["NORMAL · 1 row", "14 marked · 13 hidden"],
                b"g".to_vec(),
            ),
            (
                vec![
                    "Register / preview",
                    "more lines",
                    "Register 14 targets  13 hidden",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "j/k scroll · Esc return",
                    "r02 (hidden)",
                    "absent: r01 (hidden",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /"], b"q".to_vec()),
        ],
        24,
        80,
    );
    assert!(fs::read_to_string(&config).unwrap().contains("org/r14"));
}

#[cfg(unix)]
#[test]
fn inventory_registration_edits_wildcard_exclusions() {
    let temp = temp_dir();
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
                vec!["NORMAL · 2 rows", "absent: Scanning", "excluded"],
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
    let temp = temp_dir();
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
            "/ org/alpha",
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
                b"kkjj /org/alpha\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "1 marked · 1 hidden", "selected: alpha1"],
                b"m\r\r".to_vec(),
            ),
            (
                vec![
                    "Register / preview",
                    "absent: exact target",
                    "ELIGIBLE zeta (hidden)",
                    "~/repos/org/zeta · github.com/org/zeta",
                    "Register 1 target",
                ],
                b"q".to_vec(),
            ),
            (
                kept("Register cancelled · nothing changed", "selected: alpha1"),
                b"km\rj\r".to_vec(),
            ),
            (
                vec![
                    "Unregister / preview",
                    "NO-OP zeta (hidden)",
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
fn inventory_add_follows_its_boards_with_one_hint_row_over_an_empty_hint_bar() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos/org")).unwrap();
    fs::create_dir_all(temp.path().join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("elsewhere"), home.join("repos/org/link")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    for (rows, cols) in [(32, 132), (24, 80)] {
        let mut frames = Vec::new();
        run_inventory_pty_with_exit_observing(
            support::lager(&home, &config).arg("tui"),
            &[
                (vec!["NORMAL · 0 rows", "absent: Scanning"], b"a".to_vec()),
                (
                    vec![
                        "Add / URL or reference",
                        "Enter preview · Tab focus · Esc back",
                        "absent: q quit",
                    ],
                    b"org/*\r".to_vec(),
                ),
                (
                    vec![
                        "Rejected: wildcards are not accepted",
                        "Enter preview · Tab focus · Esc back",
                        "absent: q quit",
                    ],
                    [vec![0x7f; 5], b"org/link\r".to_vec()].concat(),
                ),
                (
                    vec![
                        "Add / preview",
                        "pair: checkout unreadable",
                        "j/k select · Enter choose · Esc back",
                        "absent: q quit",
                    ],
                    b"kkkkj\r".to_vec(),
                ),
                (
                    vec![
                        "Clone only unavailable: destination is unreadable",
                        "j/k select · Enter choose · Esc back",
                        "absent: q quit",
                    ],
                    b"\x1b".to_vec(),
                ),
                (vec!["Add / URL or reference"], b"\x1b".to_vec()),
                (
                    vec!["NORMAL", "Add cancelled · nothing changed"],
                    b"q".to_vec(),
                ),
            ],
            rows,
            cols,
            0,
            |action, _, frame| frames.push((action, frame.to_owned())),
        );
        let (rows, cols) = (rows as usize, cols as usize);
        let frame = |index: usize| {
            let (_, frame) = frames.iter().find(|(action, _)| *action == index).unwrap();
            let screen = screen_text(frame, rows, cols);
            assert_eq!(screen.split('\n').nth(rows - 1), Some(""), "{screen}");
            assert!(
                styled_screen(frame, rows, cols)[rows - 1]
                    .iter()
                    .all(|(_, reversed)| *reversed),
                "the empty hint bar stays a bar:\n{screen}"
            );
            let body = screen
                .split('\n')
                .map(|line| line.split('│').nth(1).unwrap_or_default().trim().to_owned())
                .collect::<Vec<_>>();
            (screen, body)
        };
        let (screen, body) = frame(1);
        for line in [
            "reference",
            "Preview destination",
            "Browse remote candidates",
            "Enter preview · Tab focus · Esc back",
        ] {
            assert!(body.iter().any(|body| body == line), "{line}:\n{screen}");
        }
        for absent in ["Shorthand", "Cancel", "type text"] {
            assert!(!screen.contains(absent), "{absent}:\n{screen}");
        }
        for index in [3, 4] {
            let (screen, body) = frame(index);
            for line in [
                "Register only",
                "Back to reference",
                "j/k select · Enter choose · Esc back",
            ] {
                assert!(body.iter().any(|body| body == line), "{line}:\n{screen}");
            }
            for (label, value) in [
                ("identity", "github.com/org/link"),
                ("checkout", "unreadable"),
                ("hook", "none"),
            ] {
                assert!(
                    body.iter().any(|body| body.starts_with(label)
                        && body.split_whitespace().nth(1) == Some(value)),
                    "{label} {value}:\n{screen}"
                );
            }
            for absent in [
                "Reference",
                "marks",
                "never shown as missing",
                "until you choose",
            ] {
                assert!(!screen.contains(absent), "{absent}:\n{screen}");
            }
        }
        assert!(
            frame(4)
                .0
                .contains("Clone only unavailable: destination is unreadable")
        );
    }
}

#[cfg(unix)]
#[test]
fn inventory_add_rejects_wildcards_and_credentials_without_showing_them() {
    let temp = temp_dir();
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
                    "Browse remote candidates",
                    "Enter preview · Tab focus · Esc back",
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
    let temp = temp_dir();
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
        "/ org/alpha",
        "selected: alpha1",
        "absent: problem",
        "absent: Batch /",
    ];

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                b"kkjj /org/alpha\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "1 marked · 1 hidden", "selected: alpha1"],
                b"a".to_vec(),
            ),
            (
                vec!["Add / URL or reference", "absent: target of Add"],
                b"qam/new\r".to_vec(),
            ),
            (
                vec![
                    "Add / preview",
                    "absent: Reference",
                    "pair: identity github.com/qam/new",
                    "~/repos/qam/new",
                    "pair: checkout missing",
                    "pair: registration unregistered",
                    "pair: hook none",
                    "absent: unaffected",
                    "Register only",
                    "Clone only",
                    "Clone + register",
                    "Back to reference",
                    "j/k select · Enter choose · Esc back",
                ],
                b"kkkkjjj\r".to_vec(),
            ),
            (
                vec!["Add / URL or reference", "qam/new", "absent: Add / preview"],
                b"\x1b".to_vec(),
            ),
            (kept.clone(), b"mjjjjjjj\r".to_vec()),
            (
                vec!["Add / URL or reference", "absent: qam/new"],
                b"x/y\r".to_vec(),
            ),
            (
                vec!["Add / preview", "pair: identity github.com/x/y"],
                b"q".to_vec(),
            ),
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
    let temp = temp_dir();
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
            "pair: checkout unknown",
            "Clone only unavailable: the destination exists and holds no observed checkout",
            "Clone + register unavailable: the destination exists and holds no observed checkout",
        ),
        (
            "org/link",
            "pair: checkout unreadable",
            "Clone only unavailable: destination is unreadable",
            "Clone + register unavailable: destination is unreadable",
        ),
        (
            "org/other",
            "pair: checkout conflict",
            "Clone only unavailable: destination conflicts with repository",
            "Clone + register unavailable: destination conflicts with repository",
        ),
        // The controls: both clone choices are offered here, so they are not pressed.
        ("org/alpha", "pair: checkout cloned", "", ""),
        ("org/fresh", "pair: checkout missing", "", ""),
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
    let temp = temp_dir();
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
                    "pair: checkout unknown",
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
    let temp = temp_dir();
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
                vec!["Add / URL or reference", "absent: target of Add"],
                format!("{reference}\r").into_bytes(),
            ),
            (
                vec![
                    "Add / preview",
                    "pair: identity github.com/org/beta",
                    "pair: checkout missing",
                    "pair: registration unregistered",
                    "absent: configuration only",
                ],
                b"kkkk\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Register",
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "pair: checkout missing",
                    "pair: registration unregistered",
                ],
                // Typed ahead of the handoff: it must not answer Git's prompt.
                b"kkkkjj\rleak\r".to_vec(),
            ),
            // The stub reads one line. The `q` it leaves unread must not act on the inventory.
            (vec!["Passphrase for beta:"], b"quit q a\rq".to_vec()),
            (
                vec![
                    "Batch / Clone + register",
                    "j/k scroll · Esc return",
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
                    "pair: clean explicit",
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
                    "pair: clean explicit",
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

/// A provider catalog of two repositories that exist only remotely, each cloneable from a local
/// bare repository through `insteadOf`.
#[cfg(unix)]
fn remote_only_clone_fixture(
    temp: &std::path::Path,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let home = temp.join("home");
    let config = temp.join("config.toml");
    let tools = temp.join("tools");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n",
    )
    .unwrap();
    for name in ["yak", "zebra"] {
        let source = clone_source(temp, name);
        let bare = temp.join("remotes/me").join(format!("{name}.git"));
        fs::create_dir_all(bare.parent().unwrap()).unwrap();
        let source = source.strip_prefix("file://").unwrap();
        let status = Command::new(support::real_tool("git"))
            .args(["clone", "--bare", "--quiet", source])
            .arg(&bare)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let node = |name: &str| {
        format!(
            r#"{{"name":"{name}","isArchived":false,"sshUrl":"git@github.com:me/{name}.git","owner":{{"login":"me"}}}}"#
        )
    };
    fs::write(
        temp.join("viewer.json"),
        format!(
            r#"[{{"data":{{"viewer":{{"repositories":{{"nodes":[{},{}],"pageInfo":{{"hasNextPage":false,"endCursor":null}}}}}}}}}}]"#,
            node("yak"),
            node("zebra")
        ),
    )
    .unwrap();
    fs::write(
        tools.join("gh"),
        format!(
            "#!/bin/sh\nexec cat {}\n",
            shell_word(&temp.join("viewer.json"))
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
    let git_config = temp.join("gitconfig");
    fs::write(
        &git_config,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = git@github.com:\n",
            temp.join("remotes").display()
        ),
    )
    .unwrap();
    (home, config, tools, git_config)
}

#[cfg(unix)]
#[test]
fn inventory_clone_only_clones_a_remote_only_row_and_leaves_the_config_alone() {
    let temp = temp_dir();
    let (home, config, tools, git_config) = remote_only_clone_fixture(temp.path());
    let original = fs::read_to_string(&config).unwrap();

    run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools))
            .env("GIT_CONFIG_GLOBAL", &git_config)
            .args(["tui", "--remote"]),
        &[
            (
                vec!["github.com: ready, 2 candidates", "remote only"],
                b"jjm\r".to_vec(),
            ),
            (
                vec!["Repository actions / zebra", "Clone only"],
                b"jj\r".to_vec(),
            ),
            (
                vec!["Batch / Clone only", "Clone only: 1 succeeded"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·"], b"q".to_vec()),
        ],
        32,
        132,
    );

    assert!(home.join("repos/me/zebra/.git").is_dir());
    assert!(!home.join("repos/me/yak").exists());
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn inventory_clone_register_clones_a_remote_only_row_and_registers_it() {
    let temp = temp_dir();
    let (home, config, tools, git_config) = remote_only_clone_fixture(temp.path());

    run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools))
            .env("GIT_CONFIG_GLOBAL", &git_config)
            .args(["tui", "--remote"]),
        &[
            (
                vec!["github.com: ready, 2 candidates", "remote only"],
                b"jjm\r".to_vec(),
            ),
            (
                vec!["Repository actions / zebra", "Clone + register"],
                b"jjj\r".to_vec(),
            ),
            (
                vec!["Batch / Clone + register", "Clone + register: 1 succeeded"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·"], b"q".to_vec()),
        ],
        32,
        132,
    );

    assert!(home.join("repos/me/zebra/.git").is_dir());
    assert!(!home.join("repos/me/yak").exists());
    let saved = fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("url = \"git@github.com:me/zebra.git\""),
        "{saved}"
    );
    assert!(!saved.contains("yak"), "{saved}");
}

#[cfg(unix)]
#[test]
fn inventory_clone_batch_reports_each_target_and_continues_past_a_failed_and_a_stale_one() {
    let temp = temp_dir();
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
                    "ELIGIBLE alpha",
                    "ELIGIBLE broken",
                    "ELIGIBLE delta",
                    "ELIGIBLE gamma",
                    "~/repos/delta",
                    "absent: default",
                    "Clone only 4 targets",
                    "No active operation",
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
                    "j/k scroll · Esc return",
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
                format!("m{}\r", "j".repeat(11)).into_bytes(),
            ),
            (
                vec![
                    "Problems / 2",
                    "batch · broken",
                    "Clone only failed: git exited with status 128",
                    "batch · delta",
                    "Clone only blocked: stale: destination is no longer missing after",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Problems /"], b"q".to_vec()),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                    "pair: checkout cloned",
                    "pair: registration unregistered",
                ],
                b"kkkkjj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "j/k scroll · Esc return",
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
                    "pair: clean explicit",
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
    let temp = temp_dir();
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
                vec!["Add / preview", "pair: checkout missing"],
                b"kkkkjj\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "j/k scroll · Esc return",
                    "BLOCKED",
                    // The reason wraps or moves below its row, depending on the URL's width.
                    "stale: destination",
                    "preview",
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
    let temp = temp_dir();
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
                vec!["NORMAL · 2 rows", "absent: Scanning"],
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
                    "j/k scroll · Esc return",
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
            (vec!["NORMAL · 2 rows", "absent: Batch /"], b"q".to_vec()),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                vec!["Add / preview", "pair: checkout missing"],
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
    let choices = ["Cancel", "Register 12 targets"];

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 12 rows", "absent: Scanning"],
                b"mjjjjjj\rj\r".to_vec(),
            ),
            (vec!["12 marked · 0 hidden"], b"m\r\r".to_vec()),
            (
                [
                    &[
                        "Register / preview",
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                        "j/k scroll · Esc return",
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
    // The outcome rows end at the popup's hint row.
    let rows = lines[header + 1..]
        .iter()
        .take_while(|line| !String::from_iter(line.iter()).contains("Esc return"))
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
    let temp = temp_dir();
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
                b"kk j j j /org/draft\r".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "4 marked · 3 hidden", "pair: r remove"],
                b"r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM",
                    "Remove checkouts / keep registration",
                    "absent: exact target",
                    "absent: ensure can recreate",
                    "ELIGIBLE alpha (hidden)",
                    "~/repos/org/alpha · github.com/org/alpha · clean",
                    "BLOCKED core (hidden)",
                    "dependent worktree metadata",
                    "absent: force cannot waive",
                    "LOSS ACKNOWLEDGMENT REQUIRED draft",
                    "  - unstaged changes",
                    "  - untracked files",
                    "BLOCKED local (hidden)",
                    "~/repos/scratch/local · no identity · no supported origin observed",
                    "[ ] Also remove draft and accept its listed losses",
                    "absent: losses  Space",
                    "absent: default",
                    "Remove 1 checkout",
                    "pair: checkout cannot",
                    "cannot be undone · 3 hidden",
                    "No active operation",
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
                    "pair: checkouts cannot",
                    "absent: alpha, draft",
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
fn inventory_removal_names_state_it_cannot_prove_safe_as_a_loss() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let planted = root.join("org/planted");
    tracked_repo(&planted, "git@github.com:org/planted.git");
    fs::create_dir(planted.join(".git/toolstate")).unwrap();
    fs::write(planted.join(".git/toolstate/session"), "state\n").unwrap();
    run_git(&planted, &["switch", "-qc", "feature"]);
    run_git(&planted, &["commit", "-q", "--allow-empty", "-m", "local"]);
    run_git(&planted, &["switch", "-q", "main"]);
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"r".to_vec()),
            (
                vec![
                    "CONFIRM",
                    "LOSS ACKNOWLEDGMENT REQUIRED planted",
                    "commits not on any remote: branch feature",
                    "unknown entries in .git: toolstate",
                    "[ ] Also remove planted and accept its listed losses",
                    "Remove 0 checkouts",
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
    assert_eq!(checkout_tree(&root), tree);
}

/// A checkout with more named losses than an 80×24 confirmation shows at once.
#[cfg(unix)]
fn overflowing_losses(root: &std::path::Path) -> std::path::PathBuf {
    let planted = root.join("org/planted");
    tracked_repo(&planted, "git@github.com:org/planted.git");
    for number in 0..11 {
        run_git(&planted, &["branch", &format!("feature-{number}")]);
    }
    run_git(&planted, &["commit", "-q", "--allow-empty", "-m", "local"]);
    for number in 0..11 {
        run_git(&planted, &["branch", "-qf", &format!("feature-{number}")]);
    }
    fs::write(planted.join("scratch"), "untracked\n").unwrap();
    for name in [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ] {
        let nested = planted.join(name);
        fs::create_dir_all(&nested).unwrap();
        run_git(&nested, &["init", "-q"]);
    }
    fs::create_dir(planted.join(".git/toolstate")).unwrap();
    fs::write(planted.join(".git/toolstate/state"), "x\n").unwrap();
    planted
}

#[cfg(unix)]
#[test]
fn inventory_removal_confirmation_at_80_by_24_shows_every_loss_before_it_arms() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(&config, "root = \"repos\"\n").unwrap();
    overflowing_losses(&root);
    let tree = checkout_tree(&root);

    let mut refused = String::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"r".to_vec()),
            (
                vec![
                    "CONFIRM",
                    "LOSS ACKNOWLEDGMENT REQUIRED planted",
                    "  - untracked files",
                    "more lines · PageDown",
                    "j/k focus · Space acknowledge · Enter choose · Esc cancel",
                    "absent: unknown entries in .git: toolstate",
                ],
                b" ".to_vec(),
            ),
            // Space does nothing while losses are still below the fold.
            (
                vec![
                    "[ ] Also remove planted and accept its listed losses",
                    "Remove 0 checkouts",
                    "more lines · PageDown · Space works once every loss is shown",
                ],
                b"\x1b[6~\x1b[6~\x1b[6~\x1b[6~".to_vec(),
            ),
            (
                vec![
                    "  - unknown entries in .git: toolstate",
                    "absent: more lines · PageDown",
                ],
                b" ".to_vec(),
            ),
            (
                vec![
                    "[x] Also remove planted and accept its listed losses",
                    "Remove 1 checkout",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove only cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
        0,
        |action, _, frame| {
            if action == 2 {
                refused = frame.to_owned();
            }
        },
    );
    let screen = screen_text(&refused, 24, 80);
    let lines = screen.split('\n').collect::<Vec<_>>();
    // The keys that act come first, so the narrow popup cuts only the scroll keys.
    assert!(
        lines
            .iter()
            .any(|line| line.split('│').nth(1).is_some_and(|body| body
                .trim()
                .starts_with("j/k focus · Space acknowledge · Enter choose · Esc cancel"))),
        "{screen}"
    );
    assert_eq!(lines[23], "", "{screen}");
    assert!(
        styled_screen(&refused, 24, 80)[23]
            .iter()
            .all(|(_, reversed)| *reversed),
        "the empty hint bar stays a bar:\n{screen}"
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn remove_in_a_terminal_lists_one_warning_per_line() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n\n[[repositories]]\nurl = \"git@github.com:org/planted.git\"\n",
    )
    .unwrap();
    let planted = root.join("org/planted");
    tracked_repo(&planted, "git@github.com:org/planted.git");
    fs::write(planted.join("scratch"), "untracked\n").unwrap();
    fs::create_dir(planted.join(".git/toolstate")).unwrap();
    fs::write(planted.join(".git/toolstate/state"), "x\n").unwrap();
    let tree = checkout_tree(&root);

    run_inventory_pty_actions(
        support::lager(&home, &config).args([
            "remove",
            "github.com/org/planted",
            "--keep-registered",
        ]),
        &[(
            vec![
                "Unsafe local state at",
                "- untracked files",
                "- unknown entries in .git: toolstate",
                "Force removal?",
            ],
            b"\r".to_vec(),
        )],
        24,
        80,
    );
    assert_eq!(checkout_tree(&root), tree);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_confirmation_at_80_by_24_fits_file_losses_and_a_hint() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hot = linked_worktree(&core, "hot", &["-b", "hot"]);
    run_git(&hot, &["switch", "-qc", "theirs"]);
    fs::write(hot.join("README"), "theirs\n").unwrap();
    run_git(&hot, &["commit", "-qam", "theirs"]);
    let patch = temp.path().join("theirs.patch");
    fs::write(
        &patch,
        format!(
            "{}\n",
            git_stdout(&hot, &["format-patch", "--stdout", "-1"])
        ),
    )
    .unwrap();
    run_git(&hot, &["switch", "-q", "hot"]);
    fs::write(hot.join("README"), "ours\n").unwrap();
    run_git(&hot, &["commit", "-qam", "ours"]);
    let am = std::process::Command::new("git")
        .current_dir(&hot)
        .args(["am", patch.to_str().unwrap()])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(!am.status.success(), "git am did not stop");
    fs::write(hot.join("notes"), "untracked\n").unwrap();
    fs::write(hot.join("draft"), "untracked\n").unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (vec!["Worktrees / core", "Git state"], b"jr".to_vec()),
            (
                vec![
                    "Remove worktree / hot",
                    "2 untracked files",
                    "stopped git am (in the worktree: git am --continue or",
                    "git am --abort)",
                    "absent: default",
                    "Accept these losses and remove this worktree",
                ],
                b"j\r".to_vec(),
            ),
            // The removal inspects again with the popup's hint prefix and must match.
            (
                vec![
                    "Removed worktree",
                    "Worktrees / core",
                    "absent: Remove worktree /",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        24,
        80,
    );
    assert!(!hot.exists());
}

#[cfg(unix)]
#[test]
fn inventory_paths_with_spaces_wrap_by_cell_and_keep_every_character() {
    let temp = temp_dir();
    let root = "Shared Repository Folder With A Rather Long Name Off Many Words";
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::write(&config, format!("root = \"{root}\"\n")).unwrap();
    let core = home.join(root).join("org/core");
    tracked_repo(&core, "git@github.com:org/core.git");
    linked_worktree(&core, "hot", &["-b", "hot"]);

    // Each path continues mid-word on its next line, which a wrap at spaces never does.
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"\r".to_vec()),
            (
                vec![
                    "Inspect / core",
                    "Found path         ~/Shared Repository Folder With A Rather Long Name Of  │",
                    "│                     f Many Words/org/core ",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Inspect /"], b"w".to_vec()),
            (vec!["Worktrees / core"], b"jr".to_vec()),
            (
                vec![
                    "Remove worktree / hot",
                    "path               ~/Shared Repository Folder With A Rather Long Name Of  │",
                    "│                     f Many Words/org/core.worktrees/hot ",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["Worktrees / core"], b"\x1b".to_vec()),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /"],
                b" r".to_vec(),
            ),
            (
                vec![
                    "BLOCKED core",
                    "│  ~/Shared Repository Folder With A Rather Long Name Off Many Words/org/co  │",
                    "│  re · github.com/org/core · dependent worktree metadata prevents primary",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Remove checkouts"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_never_calls_a_worktree_with_named_state_clean() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    linked_worktree(&core, "hot", &["-b", "hot"]);
    fs::write(core.join(".git/worktrees/hot/planted"), "state\n").unwrap();

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (vec!["Worktrees / core", "1 Git state"], b"\x1b".to_vec()),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_remove_only_continues_past_blocked_targets_and_keeps_registration() {
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "~/repos/org/shifty",
                    "  - untracked files",
                    "github.com/org/shifty",
                    "[ ] Also remove 2 checkouts and accept their listed losses",
                    "Remove 0 checkouts",
                ],
                b" ".to_vec(),
            ),
            (
                vec![
                    "[x] Also remove 2 checkouts and accept their listed losses",
                    "Remove 2 checkouts",
                    "pair: checkouts cannot",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                        "ELIGIBLE alpha",
                        "ELIGIBLE alpha (copy)",
                        "Remove + unregister 2 targets",
                    ],
                    b"j\r".to_vec(),
                ),
                (
                    vec![
                        "Batch / Remove + unregister",
                        "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                // Rows run archive/beta, keep/beta, org/beta, org/gone: mark all but keep/beta.
                b"kk jj j ".to_vec(),
            ),
            (vec!["3 marked · 0 hidden"], b"m\rjjjjj\r".to_vec()),
            (
                vec![
                    "Remove checkouts / unregister",
                    "ELIGIBLE beta",
                    "~/repos/org/beta · github.com/org/beta · clean",
                    "~/repos/keep/beta · also unregistered · not selected, its files stay",
                    "ELIGIBLE gone",
                    "~/repos/org/gone · github.com/org/gone · known missing",
                    "ELIGIBLE beta (copy)",
                    "~/repos/archive/beta · github.com/org/beta · clean",
                    "Remove + unregister 3 targets",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove + unregister",
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                    "known missing",
                    "absent: rechecked",
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
    let temp = temp_dir();
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
                    "absent: default",
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
                vec!["Unregister / preview", "ELIGIBLE alpha", "absent: default"],
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
                    "ELIGIBLE alpha (copy)",
                    "~/repos/org/alpha · github.com/org/alpha · clean",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Remove only",
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
                vec!["Scanning", "clean", "selected: org/alpha"],
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
                    "ELIGIBLE alpha",
                    "~/repos/zeta/alpha · also unregistered · not selected, its files stay",
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
    let temp = temp_dir();
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
            (vec!["Scanning", "clean"], b"kkU".to_vec()),
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
    let temp = temp_dir();
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
    let temp = temp_dir();
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
        "Batch: 1 / 3 processed",
        "absent: cancel batch",
        "j/k focus · PageUp/PageDown scroll · Enter choose · Esc return · q quit",
        "absent: Enter choose  Esc return",
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
                "j/k scroll · Esc return",
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
    // Sixteen keys typed while the batch is held: all are read, in order, before gamma starts.
    // Esc directly before `m` reaches the inventory as one Alt+m and still is both keys. The
    // extra `j` and the `k` end on Cancel active batch in the menu only: in the batch popup
    // they would end on Return to browsing.
    let (_temp, root, _) =
        removal_held_at_its_second_target(b"\x1bmjjjjjjjjjjjjjk\r", cancelled_batch_outcomes());
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_batch_popup_cancels_the_remaining_targets() {
    let (_temp, root, _) = removal_held_at_its_second_target(b"j\r", cancelled_batch_outcomes());
    assert!(root.join("org/gamma/.git").is_dir());
}

#[cfg(unix)]
const QUIT_PROMPT: [&str; 9] = [
    "CONFIRM",
    "A batch is running",
    "pair: processed 2",
    "pair: unattempted gamma",
    "Return to inventory",
    "pair: quit completed",
    "j/k focus · Enter choose · Esc return",
    "absent: Cancelling leaves",
    "absent: Enter choose  Esc return",
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
                    "j/k scroll · Esc return",
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
    let temp = temp_dir();
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
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    let original = "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n";
    fs::write(&config, original).unwrap();
    fs::create_dir_all(&root).unwrap();
    // Opening Add asks the provider: a stub answers with an empty catalog.
    let tools = temp.path().join("tools");
    desktop_stubs(
        &tools,
        &["gh"],
        r#"echo '[{"data":{"viewer":{"repositories":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]'"#,
    );

    run_inventory_pty_actions(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
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
    let directory = temp_dir();
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

/// macOS assesses an executable the first time it runs, and under suite load a freshly written
/// script waits seconds for that. Running the clipboard stub once first keeps that wait out of
/// the product's 2 s clipboard bound.
#[cfg(unix)]
fn warm_clipboard_stub(tools: &std::path::Path) {
    if cfg!(target_os = "macos") {
        let status = Command::new(tools.join("pbcopy"))
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
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

/// Five checkouts: owners `black`, `imfusion` and none; `api` and `theme` declared; `api` and
/// `lager` with changes. In grid order: lager, theme, api, web, scratch. `extra` follows the
/// declarations.
#[cfg(unix)]
fn filter_fixture(extra: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &config,
        format!(
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n\n\
             [[repositories]]\nurl = \"github.com/imfusion/api\"\n\n\
             [[repositories]]\nurl = \"github.com/black/theme\"\n\n{extra}"
        ),
    )
    .unwrap();
    for (path, origin, clean) in [
        ("imfusion/api", "git@github.com:imfusion/api.git", false),
        ("imfusion/web", "git@github.com:imfusion/web.git", true),
        ("black/lager", "git@github.com:black/lager.git", false),
        ("black/theme", "git@github.com:black/theme.git", true),
    ] {
        init_repo(&root.join(path), origin, clean);
    }
    init_repo_without_origin(&root.join("scratch"));
    (temp, home, config)
}

#[cfg(unix)]
#[test]
fn inventory_filter_lists_every_value_with_its_count_and_narrows_the_table_live() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 5 rows", "absent: Scanning", "pair: f filter"],
                b"f".to_vec(),
            ),
            (
                vec![
                    "FILTER · 5 rows",
                    "Filter",
                    "Changes",
                    "pair: dirty 2",
                    "pair: clean 3",
                    "Owner",
                    "pair: black 2",
                    "pair: imfusion 2",
                    "pair: — 1",
                    "Registration",
                    "pair: explicit 2",
                    "pair: unregistered 3",
                    "absent: wildcard",
                    "absent: excluded",
                    "absent: Session",
                    "j/k move · Space toggle · c clear · Enter/Esc close",
                    "absent: f filter",
                ],
                b" ".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "[x] dirty", "filter: dirty"],
                b"\x1b".to_vec(),
            ),
            // Esc closes the popup and the filter stays.
            (
                vec![
                    "NORMAL · 2 of 5 rows",
                    "filter: dirty",
                    "absent: Filter",
                    "absent: scratch",
                    "absent: web",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        140,
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_ors_within_a_group_ands_across_groups_and_clears() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 5 rows", "absent: Scanning"],
                b"fjjj ".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "[x] imfusion", "filter: imfusion"],
                b"k ".to_vec(),
            ),
            (
                vec!["FILTER · 4 of 5 rows", "filter: black · imfusion"],
                b"kk ".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "filter: dirty · black · imfusion"],
                b"c".to_vec(),
            ),
            (
                vec!["FILTER · 5 rows", "absent: filter:", "[ ] dirty"],
                b"jjjjj ".to_vec(),
            ),
            // Enter closes the popup like Esc, ticks nothing, and the filter stays.
            (
                vec!["FILTER · 2 of 5 rows", "filter: explicit"],
                b"\r".to_vec(),
            ),
            (
                vec!["NORMAL · 2 of 5 rows", "filter: explicit", "absent: Filter"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_lists_only_the_registrations_present() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n\n\
         [[repositories]]\nurl = \"github.com/team/*\"\nexclude = [\"gamma\"]\n",
    )
    .unwrap();
    for name in ["delta", "gamma"] {
        init_repo(
            &root.join("team").join(name),
            &format!("git@github.com:team/{name}.git"),
            true,
        );
    }

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 2 rows", "absent: Scanning"], b"f".to_vec()),
            (
                vec![
                    "pair: wildcard 1",
                    "pair: excluded 1",
                    "absent: explicit",
                    "absent: unregistered",
                ],
                b"jjj ".to_vec(),
            ),
            (
                vec!["FILTER · 1 of 2 rows", "filter: excluded"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 of 2 rows", "gamma", "absent: delta"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_by_owner_works_in_the_narrow_column_set() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec![
                    "NORMAL · 5 rows",
                    "absent: Scanning",
                    "absent: owner",
                    "absent: f filter",
                ],
                b"fjjj ".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "filter: imfusion"],
                b"q".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 2 of 5 rows",
                    "api",
                    "web",
                    "absent: lager",
                    "absent: scratch",
                    "absent: owner",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
}

#[cfg(unix)]
#[test]
fn inventory_search_applies_on_top_of_the_filter() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 5 rows", "absent: Scanning"],
                b"fjjj ".to_vec(),
            ),
            (vec!["FILTER · 2 of 5 rows"], b"\x1b".to_vec()),
            (vec!["NORMAL · 2 of 5 rows"], b"/api\r".to_vec()),
            (
                vec!["NORMAL · 1 of 5 rows", "filter: imfusion", "absent: web"],
                b"\x7f".to_vec(),
            ),
            // The row search finds is hidden by the filter.
            (vec!["NORMAL · 2 of 5 rows"], b"/scratch\r".to_vec()),
            (
                vec!["NORMAL · 0 of 5 rows", "No matching repositories."],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 2 of 5 rows", "api"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_marks_on_rows_the_filter_hides_count_as_hidden_in_a_batch() {
    let (_temp, home, config) = filter_fixture("[inventory.keys.normal]\nregister = [\"g\"]\n");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b" ".to_vec()),
            (vec!["1 marked · 0 hidden"], b"fj ".to_vec()),
            (
                vec!["FILTER · 3 of 5 rows", "1 marked · 1 hidden"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 3 of 5 rows"], b"g".to_vec()),
            (vec!["Register / preview", "1 hidden"], b"\x1b".to_vec()),
            (vec!["NORMAL", "absent: Register /"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_keys_can_be_rebound_and_a_taken_f_keeps_the_config_loading() {
    let (_temp, home, config) = filter_fixture(
        "[inventory.keys.normal]\nfilter = [\"F\"]\n\n\
         [inventory.keys.filter]\ntoggle = [\"t\"]\nclear_filter = [\"x\"]\n",
    );
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b"?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "F / Ctrl+d",
                    "pair: Ctrl+d filter",
                    "filter = [\"F\"]",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"F".to_vec()),
            (
                vec!["FILTER · 5 rows", "t toggle · x clear"],
                b"jjjt".to_vec(),
            ),
            (vec!["FILTER · 2 of 5 rows", "[x] imfusion"], b"x".to_vec()),
            (vec!["FILTER · 5 rows", "absent: [x]"], b"\x1b".to_vec()),
            (vec!["NORMAL · 5 rows", "absent: Filter"], b"q".to_vec()),
        ],
        32,
        132,
    );

    // A configuration that already binds `f` keeps its binding; the filter has no key.
    let (_temp, home, config) = filter_fixture("[inventory.keys.normal]\nrefresh = [\"f\"]\n");
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b"?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "pair: f refresh",
                    "No key in normal mode: filter",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"q".to_vec()),
        ],
        44,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_menu_opens_the_filter() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b"m".to_vec()),
            (
                vec!["All actions", "pair: Filter… f"],
                format!("{}\r", "j".repeat(8)).into_bytes(),
            ),
            (
                vec!["FILTER · 5 rows", "absent: All actions"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Filter"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

/// Two checkouts, `core` with an open session and `other` without, and the fake Rex that
/// answers the session list. `extra` follows the `[session]` section.
#[cfg(unix)]
fn open_session_fixture(
    extra: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), &format!("{SESSION_SECTION}\n{extra}"));
    tracked_repo(
        &home.join("repos/org/other"),
        "git@github.com:org/other.git",
    );
    // Opening the session of `core` leaves it open for the next run.
    let (mut command, _) = session_tui(temp.path(), &home, &config);
    run_inventory_pty_with_exit(
        &mut command,
        &[(
            vec!["NORMAL · 2 rows", "absent: Scanning", "selected: core"],
            b"s".to_vec(),
        )],
        32,
        132,
        0,
        |_| {},
    );
    (temp, home, config)
}

#[cfg(unix)]
#[test]
fn inventory_filter_offers_the_session_group_only_with_a_session_host_and_filters_by_it() {
    let (temp, home, config) = open_session_fixture("");
    let (mut command, _) = session_tui(temp.path(), &home, &config);
    run_inventory_pty_actions(
        &mut command,
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "pair: ● core"],
                b"f".to_vec(),
            ),
            (
                vec!["Session", "pair: open 1", "pair: none 1"],
                b"jjj ".to_vec(),
            ),
            (
                vec!["FILTER · 1 of 2 rows", "filter: session open"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 of 2 rows", "pair: ● core", "absent: other"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_keeps_the_last_known_changes_of_a_stale_row_during_a_refresh() {
    let (temp, home, config) = filter_fixture("");
    let slow_marker = temp.path().join("slow-refresh");
    let tools = temp.path().join("delayed-tools");
    fs::create_dir_all(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e {} ]; then /bin/sleep 2; fi\nexec {} \"$@\"\n",
            shell_word(&slow_marker),
            shell_word(&support::real_tool("git"))
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b"f ".to_vec()),
            (vec!["FILTER · 2 of 5 rows"], b"\x1b".to_vec()),
            (vec!["NORMAL · 2 of 5 rows"], b"R".to_vec()),
            // The two dirty rows stay while their changes read stale.
            (
                vec!["NORMAL · 2 of 5 rows", "pair: stale: 1", "api", "lager"],
                b"f".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "pair: dirty 2", "[x] dirty"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 2 of 5 rows", "absent: Filter"],
                b"q".to_vec(),
            ),
        ],
        32,
        140,
        |action| {
            if action == 0 {
                fs::write(&slow_marker, "slow").unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_filter_lists_ticked_owners_in_the_grid_order() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&root).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    for (path, origin) in [
        ("Beta/pear", "git@github.com:x/pear.git"),
        ("alpha/mango", "git@github.com:y/mango.git"),
    ] {
        init_repo(&root.join(path), origin, true);
    }

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"fj j ".to_vec(),
            ),
            // Case-sensitively `Beta` would come first.
            (
                vec!["FILTER · 2 of 2 rows", "filter: alpha · Beta"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 2 of 2 rows", "absent: Filter"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_o_toggles_the_open_sessions_filter() {
    let (temp, home, config) = open_session_fixture("");
    let (mut command, _) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_actions(
        &mut command,
        &[
            (
                vec![
                    "NORMAL · 2 rows",
                    "absent: Scanning",
                    "pair: ● core",
                    "pair: Ctrl+o sessions",
                ],
                b"\x0f".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 of 2 rows",
                    "filter: session open",
                    "pair: ● core",
                    "absent: other",
                ],
                b"f".to_vec(),
            ),
            // The popup shows the same state.
            (
                vec!["FILTER · 1 of 2 rows", "[x] open", "[ ] none"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 of 2 rows", "absent: Filter"],
                b"\x0f".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "absent: filter:", "other"],
                b"q".to_vec(),
            ),
        ],
        32,
        170,
    );
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_d_toggles_the_dirty_filter() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 5 rows", "absent: Scanning"],
                b"\x04".to_vec(),
            ),
            (
                vec!["NORMAL · 2 of 5 rows", "filter: dirty", "absent: theme"],
                b"f".to_vec(),
            ),
            (
                vec!["FILTER · 2 of 5 rows", "[x] dirty", "[ ] clean"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 2 of 5 rows", "absent: Filter"],
                b"\x04".to_vec(),
            ),
            (
                vec!["NORMAL · 5 rows", "absent: filter:", "theme"],
                b"q".to_vec(),
            ),
        ],
        32,
        170,
    );
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_o_says_why_it_does_nothing_without_a_session_host() {
    let (_temp, home, config) = filter_fixture("");

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 5 rows", "absent: Scanning", "absent: sessions"],
                b"\x0f".to_vec(),
            ),
            (
                vec![
                    "Open sessions unavailable: no [session] section in the configuration",
                    "NORMAL · 5 rows",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        140,
    );
}

#[cfg(unix)]
#[test]
fn inventory_open_sessions_key_can_be_rebound_and_a_taken_ctrl_o_keeps_the_config_loading() {
    let (temp, home, config) =
        open_session_fixture("[inventory.keys.normal]\nopen_sessions = [\"O\"]\n");
    let (mut command, _) = session_tui(temp.path(), &home, &config);
    // The default key is dead: only `O` filters.
    run_inventory_pty_actions(
        &mut command,
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "pair: ● core"],
                b"\x0fO".to_vec(),
            ),
            (
                vec!["NORMAL · 1 of 2 rows", "filter: session open"],
                b"q".to_vec(),
            ),
        ],
        32,
        140,
    );

    let (_temp, home, config) = filter_fixture("[inventory.keys.normal]\nrefresh = [\"Ctrl+o\"]\n");
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 5 rows", "absent: Scanning"], b"?".to_vec()),
            (
                vec!["Keyboard / effective bindings", "pair: Ctrl+o refresh"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Keyboard /"], b"q".to_vec()),
        ],
        32,
        140,
    );
}

fn output(command: &mut Command) -> std::process::Output {
    command.output().unwrap()
}

#[cfg(unix)]
static TRANSCRIPT_ID: AtomicU64 = AtomicU64::new(1);
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh directory named by digits under a fixed base. A path search sees a path outside home
/// in full, so a fixture path must not spell a test's query by chance, whatever TMPDIR or a random
/// name holds.
fn temp_dir() -> tempfile::TempDir {
    let name = format!(
        "{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    );
    tempfile::Builder::new()
        .prefix(&name)
        .rand_bytes(0)
        .tempdir_in("/tmp")
        .unwrap()
}

/// Gives a directory made unreadable by a test its mode back, so the tempdir can drop it.
#[cfg(unix)]
struct RestoreMode(std::path::PathBuf);

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

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

/// The table's rows without the owner column, so a row reads repository, local, branch,
/// status, registration at the full grid; hidden columns are missing from the row.
#[cfg(unix)]
fn inventory_rows(screen: &str) -> Vec<Vec<String>> {
    let (headings, rows) = inventory_grid(screen);
    let owner = headings.iter().position(|heading| heading == "owner");
    rows.into_iter()
        .map(|mut row| {
            if let Some(owner) = owner {
                row.remove(owner);
            }
            row
        })
        .collect()
}

/// Splits table rows at the column header offsets and returns the headings with the rows;
/// rows end at the first line without a mark slot or with a popup border. The repository
/// cell starts after the mark slot.
#[cfg(unix)]
fn inventory_grid(screen: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let lines = screen.lines().collect::<Vec<_>>();
    let Some(header) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("repository") && line.contains("status"))
    else {
        return (Vec::new(), Vec::new());
    };
    let headings = lines[header]
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut offsets = Vec::new();
    let mut from = 0;
    for heading in lines[header].split_whitespace() {
        let start = lines[header][from..].find(heading).unwrap() + from;
        offsets.push(start);
        from = start + heading.len();
    }
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
    let rows = lines[header + 1..]
        .iter()
        .take_while(|line| {
            let characters = line.chars().collect::<Vec<_>>();
            characters.len() > 6
                && MARK_SLOTS.contains(&characters[2..6].iter().collect::<String>().as_str())
                && characters[6..].iter().any(|character| *character != ' ')
                && !line.contains(['│', '┌', '└', '├'])
        })
        .map(|line| {
            (0..offsets.len())
                .map(|column| cell(line, column))
                .collect()
        })
        .collect();
    (headings, rows)
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
            let text = |from: usize, to: usize| {
                line[from..to]
                    .iter()
                    .map(|(character, _)| *character)
                    .collect::<String>()
            };
            // The mark slot, then the session slot when sessions are shown.
            let slot = [4, 6].into_iter().find(|&width| {
                start >= width
                    && MARK_SLOTS.contains(&text(start - width, start - width + 4).as_str())
                    && (width == 4 || ["  ", "● "].contains(&text(start - 2, start).as_str()))
            });
            let fixed_mark_slot = slot.is_some_and(|width| {
                line[start - width..start]
                    .iter()
                    .all(|(_, reversed)| *reversed)
            });
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

/// `home/repos/org/core` with an origin and no linked worktree; returns home, config, core.
#[cfg(unix)]
fn worktree_fixture(
    temp: &std::path::Path,
    settings: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let home = temp.join("home");
    let config = temp.join("config.toml");
    fs::create_dir_all(temp).unwrap();
    fs::write(&config, format!("root = \"repos\"\n{settings}")).unwrap();
    let core = home.join("repos/org/core");
    tracked_repo(&core, "git@github.com:org/core.git");
    (home, config, core)
}

/// Adds `core.worktrees/<name>` beside the primary through Git itself.
#[cfg(unix)]
fn linked_worktree(core: &std::path::Path, name: &str, args: &[&str]) -> std::path::PathBuf {
    let path = core.parent().unwrap().join("core.worktrees").join(name);
    let mut command = vec!["worktree", "add", "-q"];
    command.extend(args);
    command.push(path.to_str().unwrap());
    run_git(core, &command);
    path
}

#[cfg(unix)]
fn git_stdout(directory: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(unix)]
const POPUP_READY: [&str; 3] = ["NORMAL · 1 row", "absent: Scanning", "selected: core"];
#[cfg(unix)]
const POPUP_FOOTER: &str = "j/k select · a add · r remove · c copy · Esc close";

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_lists_every_working_tree_and_copies_the_selected_path() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hotfix = linked_worktree(&core, "hotfix", &["-b", "hotfix"]);
    // Created outside Lager with a control character in its directory name.
    let odd = linked_worktree(&core, "odd\tname", &["-b", "odd"]);
    let pinned = linked_worktree(&core, "pinned", &["--detach"]);
    run_git(
        &core,
        &[
            "worktree",
            "lock",
            "--reason",
            "reviewing on\tlaptop",
            pinned.to_str().unwrap(),
        ],
    );
    let review = linked_worktree(&core, "review-41", &["-b", "feature/login"]);
    fs::write(review.join("README"), "modified\n").unwrap();
    fs::write(review.join("notes"), "untracked\n").unwrap();
    let spike = home.join("scratch/core-spike");
    run_git(
        &core,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "dev/spike",
            spike.to_str().unwrap(),
        ],
    );
    fs::remove_dir_all(&spike).unwrap();
    let detached = format!(
        "detached {}",
        git_stdout(&core, &["rev-parse", "--short=7", "HEAD"])
    );
    let tools = temp.path().join("tools");
    let copied = temp.path().join("copied");
    desktop_stubs(
        &tools,
        &["pbcopy", "wl-copy", "xclip", "xsel", "clip"],
        &format!("exec cat > {}", shell_word(&copied)),
    );
    warm_clipboard_stub(&tools);

    let mut values = Vec::new();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (
                vec![
                    "WORKTREES · 1 row",
                    "Worktrees / core",
                    "absent: linked worktrees",
                    "pair: ~/repos/org/core main",
                    "primary · clean",
                    "pair: ~/repos/org/core.worktrees/hotfix hotfix",
                    "pair: hotfix clean",
                    "pair: ~/repos/org/core.worktrees/odd\\tname odd",
                    "pair: ~/repos/org/core.worktrees/pinned detached",
                    &detached,
                    "locked: reviewing on\\tlaptop",
                    "pair: ~/repos/org/core.worktrees/review-41 feature/login",
                    "1 modified, 1 untracked",
                    "pair: ~/scratch/core-spike dev/spike",
                    "prunable: gitdir file points",
                    "absent: primary checkout",
                    POPUP_FOOTER,
                    "No active operation",
                ],
                b"r".to_vec(),
            ),
            (
                vec![
                    "Remove unavailable: the primary checkout is removed from the table, not here",
                    "Worktrees / core",
                    "absent: Remove worktree",
                ],
                b"jc".to_vec(),
            ),
            (
                vec!["Copied path of hotfix", "absent: removes it after"],
                b"jc".to_vec(),
            ),
            (vec!["Copied path of odd\\tname"], b"\x1b".to_vec()),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if matches!(action, 3 | 4) {
                values.push(fs::read(&copied).unwrap());
            }
        },
    );
    // The clipboard gets the original bytes, the tab included.
    assert_eq!(
        values,
        [&hotfix, &odd].map(|path| fs::canonicalize(path)
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
            .to_vec())
    );

    // 80×24: every working tree, the hints and the status line stay on screen.
    let mut list = String::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (
                vec![
                    "Worktrees / core",
                    "primary · clean",
                    "odd\\tname",
                    "pair: hotfix clean",
                    &detached,
                    "feature/login",
                    "1 modified, 1 untracked",
                    "core-spike",
                    "prunable",
                    POPUP_FOOTER,
                    "No active operation",
                ],
                b"r".to_vec(),
            ),
            (
                vec![
                    "Remove unavailable: the primary checkout is removed from the table, not here",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
        0,
        |action, _, frame| {
            if action == 1 {
                list = frame.to_owned();
            }
        },
    );
    let screen = screen_text(&list, 24, 80);
    let lines = screen.split('\n').collect::<Vec<_>>();
    let body = lines
        .iter()
        .map(|line| line.split('│').nth(1).unwrap_or_default().trim())
        .collect::<Vec<_>>();
    assert!(body.contains(&POPUP_FOOTER), "{screen}");
    // The column heading follows the title directly: no sentence above the list.
    let title = body
        .iter()
        .position(|line| line.starts_with("Worktrees / core"))
        .unwrap();
    assert!(body[title + 2].starts_with("path "), "{screen}");
    assert_eq!(lines[23], "", "{screen}");
    assert!(
        styled_screen(&list, 24, 80)[23]
            .iter()
            .all(|(_, reversed)| *reversed),
        "the empty hint bar stays a bar:\n{screen}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_adds_a_worktree_through_input_and_plan() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let created = core.parent().unwrap().join("core.worktrees/fix");

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (vec!["Worktrees / core", "primary · clean"], b"a".to_vec()),
            (
                vec![
                    "INPUT · 1 row",
                    "Add worktree / core",
                    "Enter preview · Esc back",
                    "No active operation",
                    "absent: Worktree name",
                    "absent: Letters, digits",
                    "absent: Nothing is created yet",
                    "absent: Preview plan",
                    "absent: Cancel",
                ],
                b"a/b\r".to_vec(),
            ),
            (
                vec!["Rejected: NAME is one path component", "absent: / plan"],
                b"\x7f\x7f\x7fa..b\r".to_vec(),
            ),
            // Git's own answer to a name it refuses never reaches the screen.
            (
                vec!["Rejected: invalid branch name: a..b", "absent: / plan"],
                b"\x7f\x7f\x7f\x7ffix\r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM · 1 row",
                    "Add worktree / plan",
                    "pair: path ~/repos/org/core.worktrees/fix",
                    "pair: branch fix",
                    "fix · new, at ",
                    "Create worktree",
                    "absent: default",
                    "Back to name",
                    "absent: copies no files",
                    "absent: Open session",
                    "j/k select · Enter choose · Esc back",
                    "No active operation",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Created worktree ~/repos/org/core.worktrees/fix on new branch fix",
                    "WORKTREES · 1 row",
                    "pair: ~/repos/org/core.worktrees/fix fix",
                    "absent: Add worktree",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert!(
        transcript.contains("Preparing worktree (new branch 'fix')"),
        "{transcript}"
    );
    assert!(!transcript.contains("fatal:"), "{transcript}");
    assert_screen_left(&transcript);
    assert_eq!(
        git_stdout(&created, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "fix"
    );

    // New refuses a name whose branch exists; Local checks that branch out, and
    // `worktree_path` places it.
    let (home, config, core) = worktree_fixture(
        &temp.path().join("placed"),
        "worktree_path = \"~/wt/{relative}.worktrees/{name}\"\n",
    );
    run_git(&core, &["branch", "topic"]);
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa".to_vec()),
            (vec!["Add worktree / core"], b"topic\r".to_vec()),
            (
                vec![
                    "Rejected: branch topic exists; pick it on Local",
                    "[New]",
                    "absent: / plan",
                ],
                b"]".to_vec(),
            ),
            (
                vec!["[Local]", "2 branches", "primary", "topic"],
                b"/top".to_vec(),
            ),
            (vec!["/ top█", "absent: primary"], b"\r\r".to_vec()),
            (
                vec![
                    "Add worktree / plan",
                    "pair: name topic",
                    "pair: path ~/wt/org/core.worktrees/topic",
                    "pair: branch topic",
                    "existing",
                    "Change name",
                    "Back to branches",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Created worktree ~/wt/org/core.worktrees/topic on branch topic",
                    "pair: ~/wt/org/core.worktrees/topic topic",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
    );
    assert_eq!(
        git_stdout(
            &home.join("wt/org/core.worktrees/topic"),
            &["rev-parse", "--abbrev-ref", "HEAD"]
        ),
        "topic"
    );
}

/// Points `refs/remotes/<remote_branch>` at a new commit that no local branch holds.
#[cfg(unix)]
fn remote_only_branch(core: &std::path::Path, remote_branch: &str) -> String {
    run_git(core, &["checkout", "-qb", "scratch"]);
    fs::write(core.join("remote-only"), remote_branch).unwrap();
    run_git(core, &["add", "remote-only"]);
    run_git(core, &["commit", "-qm", remote_branch]);
    let commit = git_stdout(core, &["rev-parse", "HEAD"]);
    run_git(
        core,
        &[
            "update-ref",
            &format!("refs/remotes/{remote_branch}"),
            &commit,
        ],
    );
    run_git(core, &["checkout", "-q", "main"]);
    run_git(core, &["branch", "-qD", "scratch"]);
    commit
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_switches_tabs_and_refuses_a_checked_out_branch() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    linked_worktree(&core, "wildcards", &["-b", "fix/wildcards"]);
    run_git(&core, &["branch", "spike/tabs"]);
    run_git(
        &core,
        &[
            "update-ref",
            "refs/remotes/origin/fix/wildcards",
            "fix/wildcards",
        ],
    );

    let mut command = support::lager(&home, &config);
    command.arg("tui").env("NO_COLOR", "1");
    run_inventory_pty_actions(
        &mut command,
        &[
            (POPUP_READY.to_vec(), b"wa".to_vec()),
            (
                vec![
                    "Add worktree / core",
                    "[New]",
                    " Local ",
                    " Remote ",
                    "[ ] tab · Enter preview · Esc back",
                ],
                b"]".to_vec(),
            ),
            (
                vec![
                    "WORKTREES · 1 row",
                    "[Local]",
                    "absent: [New]",
                    "3 branches",
                    "pair: checked out",
                    "primary",
                    "core.worktrees/wildcards",
                    "spike/tabs",
                    "/ Filter branches…",
                    "[ ] tab · j/k select · / filter · Enter plan · Esc back",
                ],
                b"]".to_vec(),
            ),
            (
                vec![
                    "[Remote]",
                    "absent: [Local]",
                    "never fetched",
                    "origin/main",
                    // Where the local branch is checked out is text, not only a dimmed row.
                    "main · same commit · primary",
                    "fix/wildcards · same commit · in core.worktrees/wildcards",
                    "[ ] tab · j/k select · / filter · f fetch · Enter plan · Esc back",
                ],
                b"]".to_vec(),
            ),
            (vec!["[New]", "absent: [Remote]"], b"[".to_vec()),
            (vec!["[Remote]"], b"[/wild\r\r".to_vec()),
            (
                vec![
                    "! fix/wildcards is checked out in core.worktrees/wildcards",
                    "[Local]",
                    "/ wild",
                    "absent: spike/tabs",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "Add worktree cancelled · nothing changed",
                    "Worktrees / core",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_tab_key_rebound_to_a_letter_stays_text_on_new() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        "[inventory.keys.worktrees]\nnext_tab = [\"n\"]\n",
    );

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa".to_vec()),
            (
                vec!["[New]", "Enter preview · Esc back", "absent: tab ·"],
                b"n".to_vec(),
            ),
            // `n` can be part of a NAME, so on New it is text.
            (vec!["[New]", "n█"], b"[".to_vec()),
            (vec!["[Remote]"], b"[".to_vec()),
            (vec!["[Local]", "[ n tab · j/k select"], b"n".to_vec()),
            (vec!["[Remote]", "absent: [Local]"], b"\x1b".to_vec()),
            (
                vec!["Add worktree cancelled · nothing changed"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_creates_a_tracking_worktree_from_a_remote_branch() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let commit = remote_only_branch(&core, "origin/feat/session-tabs");
    let created = core.parent().unwrap().join("core.worktrees/tabs");

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa]]".to_vec()),
            (
                vec!["[Remote]", "origin/feat/session-tabs", "origin/main"],
                b"/session\r\r".to_vec(),
            ),
            (
                vec![
                    "CONFIRM · 1 row",
                    "Add worktree / plan",
                    "pair: name session-tabs",
                    "pair: path ~/repos/org/core.worktrees/session-tabs",
                    &format!(
                        "feat/session-tabs · new, at {} · tracking origin/feat/session-tabs",
                        &commit[..7]
                    ),
                    "Create worktree",
                    "Change name",
                    "Back to branches",
                ],
                b"jj\r".to_vec(),
            ),
            (vec!["[Remote]", "/ session"], b"\r".to_vec()),
            (vec!["Add worktree / plan"], b"j\r".to_vec()),
            (
                vec!["INPUT · 1 row", "session-tabs█"],
                [&b"\x7f".repeat("session-tabs".len())[..], b"tabs\r"].concat(),
            ),
            (
                vec![
                    "Add worktree / plan",
                    "pair: name tabs",
                    "pair: path ~/repos/org/core.worktrees/tabs",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Created worktree ~/repos/org/core.worktrees/tabs on new branch feat/session-tabs, tracking origin/feat/session-tabs",
                    "WORKTREES · 1 row",
                    "pair: ~/repos/org/core.worktrees/tabs feat/session-tabs",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert!(!transcript.contains("fatal:"), "{transcript}");
    assert_eq!(
        git_stdout(&created, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/feat/session-tabs"
    );
    assert_eq!(git_stdout(&created, &["rev-parse", "HEAD"]), commit);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_fetches_each_remote_and_keeps_the_list_when_one_fails() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    // Every remote is a local path, so the fetch never reaches the network.
    let bare = temp.path().join("upstream.git");
    run_git(
        temp.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    run_git(&core, &["remote", "remove", "origin"]);
    run_git(
        &core,
        &["remote", "add", "upstream", bare.to_str().unwrap()],
    );
    let missing = temp.path().join("missing.git");
    run_git(
        &core,
        &["remote", "add", "broken", missing.to_str().unwrap()],
    );
    run_git(&core, &["push", "-q", "upstream", "main"]);
    let other = temp.path().join("other");
    run_git(
        temp.path(),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    run_git(&other, &["config", "user.email", "lager@example.invalid"]);
    run_git(&other, &["config", "user.name", "lager"]);
    run_git(&other, &["checkout", "-qb", "feat/fetched"]);
    fs::write(other.join("fetched"), "fetched\n").unwrap();
    run_git(&other, &["add", "fetched"]);
    run_git(&other, &["commit", "-qm", "fetched"]);
    run_git(&other, &["push", "-q", "origin", "feat/fetched"]);

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa]]".to_vec()),
            (
                vec![
                    "[Remote]",
                    "never fetched",
                    "upstream/main",
                    "absent: upstream/feat/fetched",
                ],
                b"f".to_vec(),
            ),
            (
                vec![
                    "! fetch broken failed: ",
                    "upstream/feat/fetched",
                    "upstream/main",
                    "last fetch ",
                    "fetch failed",
                    "absent: never fetched",
                    "[Remote]",
                ],
                b"\r".to_vec(),
            ),
            // The fetched branch sorts first, and the selection stays on upstream/main.
            (
                vec![
                    "! main is checked out in core",
                    "absent: ! fetch broken",
                    "upstream/feat/fetched",
                    "absent: Add worktree / plan",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add worktree cancelled · nothing changed"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    // Git's own fetch output never reaches the terminal the inventory draws on.
    assert!(!transcript.contains("[new branch]"), "{transcript}");
    assert_eq!(
        git_stdout(&core, &["rev-parse", "refs/remotes/upstream/feat/fetched"]),
        git_stdout(&other, &["rev-parse", "HEAD"])
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_says_a_repository_without_remotes_has_nothing_to_fetch() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    run_git(&core, &["remote", "remove", "origin"]);

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa]]".to_vec()),
            (
                vec!["[Remote]", "no remotes", "No remote branches"],
                b"f".to_vec(),
            ),
            (
                vec!["! core has no remote to fetch", "No remote branches"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add worktree cancelled · nothing changed"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
}

/// `core.worktrees/draft` on branch `draft` with one modified and one untracked file.
#[cfg(unix)]
fn dirty_worktree(core: &std::path::Path) -> std::path::PathBuf {
    let draft = linked_worktree(core, "draft", &["-b", "draft"]);
    fs::write(draft.join("README"), "modified\n").unwrap();
    fs::write(draft.join("notes"), "untracked\n").unwrap();
    draft
}

#[cfg(unix)]
const LOSS_CONFIRMATION: [&str; 10] = [
    "CONFIRM · 1 row",
    "absent: Close session",
    "Remove worktree / draft",
    "pair: path ~/repos/org/core.worktrees/draft",
    "draft · kept",
    "1 modified file, 1 untracked file",
    "absent: default",
    "Accept these losses and remove this worktree",
    "j/k focus · Enter choose · Esc cancel",
    "absent: Worktrees / core",
];

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_removes_a_worktree_with_losses_only_after_yes() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let draft = dirty_worktree(&core);

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (
                vec!["pair: ~/repos/org/core.worktrees/draft draft"],
                b"jr".to_vec(),
            ),
            // Enter on the default keeps the worktree.
            (LOSS_CONFIRMATION.to_vec(), b"\r".to_vec()),
            (
                vec![
                    "Remove cancelled · nothing changed",
                    "Worktrees / core",
                    "absent: Remove worktree /",
                ],
                b"r".to_vec(),
            ),
            (LOSS_CONFIRMATION.to_vec(), b"j\r".to_vec()),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept",
                    "WORKTREES · 1 row",
                    "primary · clean",
                    "absent: Remove worktree /",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 3 || action == 4 {
                assert_eq!(
                    fs::read_to_string(draft.join("notes")).unwrap(),
                    "untracked\n"
                );
            }
        },
    );
    assert!(!draft.exists());
    assert_eq!(git_stdout(&core, &["branch", "--list", "draft"]), "draft");
    assert_eq!(
        fs::read_to_string(core.join("README")).unwrap(),
        "initial\n"
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_shows_blockers_without_offering_a_confirmation() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let locked = linked_worktree(&core, "locked", &["-b", "locked"]);
    run_git(
        &core,
        &[
            "worktree",
            "lock",
            "--reason",
            "on loan",
            locked.to_str().unwrap(),
        ],
    );
    let nested = linked_worktree(&core, "nested", &["-b", "nested"]);
    init_repo_without_origin(&nested.join("vendor/lib"));
    fs::write(nested.join("notes"), "untracked\n").unwrap();
    let tree = checkout_tree(&home);
    let no_confirmation = [
        "absent: Cancel",
        "absent: remove this worktree",
        "absent: CONFIRM",
        "absent: Nothing was removed",
    ];

    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (
                vec!["locked: on loan", "absent: reports the blocker"],
                b"jr".to_vec(),
            ),
            (
                [
                    &[
                        "Remove worktree / blocked",
                        "pair: blocked worktree",
                        "worktree is locked: on loan",
                        "pair: hint git",
                        "worktree unlock",
                        "j/k scroll · Esc back",
                    ][..],
                    &no_confirmation,
                ]
                .concat(),
                b"\r".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Remove worktree /"],
                b"jr".to_vec(),
            ),
            // Neither an answer nor a move to one does anything here.
            (
                [
                    &[
                        "Remove worktree / blocked",
                        "pair: blocked nested",
                        "nested repository vendor/lib",
                    ][..],
                    &no_confirmation,
                ]
                .concat(),
                b"jy\r".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Remove worktree /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
    );
    assert_eq!(checkout_tree(&home), tree);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_is_unavailable_on_rows_without_a_checkout() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        "\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/ghost\"\n\n[[repositories]]\nurl = \"github.com/org/sealed\"\n",
    );
    let sealed = home.join("repos/org/sealed");
    fs::create_dir_all(&sealed).unwrap();
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 3 rows", "absent: Scanning"],
                // Rows run core, ghost, sealed: the top, then one down.
                b"kkjw".to_vec(),
            ),
            (
                vec![
                    "Worktrees unavailable: no checkout at the destination",
                    "absent: Worktrees /",
                ],
                b"jw".to_vec(),
            ),
            (
                vec![
                    "selected: sealed",
                    "Worktrees unavailable: the checkout is unreadable",
                    "absent: Worktrees /",
                ],
                b"k".to_vec(),
            ),
            // The menu entry is disabled with the same reason.
            (vec!["selected: ghost"], b"mjjjj\r".to_vec()),
            (
                vec![
                    "All actions",
                    "pair: Worktrees… no",
                    "checkout at the destination",
                    "absent: Worktrees /",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 3 rows", "absent: All actions"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |_| {},
    );
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_keys_typed_before_the_confirmation_is_drawn_never_answer_it() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let draft = dirty_worktree(&core);

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wj".to_vec()),
            // Open, move to the removing choice, choose: all in one burst.
            (
                vec!["pair: ~/repos/org/core.worktrees/draft draft"],
                b"rj\r".to_vec(),
            ),
            // The cursor still sits on Cancel, and Enter there keeps the worktree.
            (LOSS_CONFIRMATION.to_vec(), b"\r".to_vec()),
            (
                vec!["Remove cancelled · nothing changed", "Worktrees / core"],
                b"a".to_vec(),
            ),
            // The same holds for the plan of a new worktree.
            (vec!["Add worktree / core"], b"fix\r\r".to_vec()),
            (
                vec!["Add worktree / plan", "Create worktree", "absent: default"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add worktree / core", "absent: / plan"],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "Add worktree cancelled · nothing changed",
                    "Worktrees / core",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Worktrees /",
                    "absent: Add worktree",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action >= 2 {
                assert!(draft.join("notes").exists(), "removed at action {action}");
            }
        },
    );
    assert!(draft.join("notes").exists());
    assert!(!core.parent().unwrap().join("core.worktrees/fix").exists());
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_aborts_when_the_worktree_changed_after_the_confirmation() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hotfix = linked_worktree(&core, "hotfix", &["-b", "hotfix"]);

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wjr".to_vec()),
            (
                vec![
                    "Remove worktree / hotfix",
                    "pair: losses none",
                    "absent: default",
                    "pair: worktree cannot",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "the worktree changed after it was inspected; nothing removed",
                    "Worktrees / core",
                    "1 untracked",
                    "absent: Remove worktree /",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 1 {
                fs::write(hotfix.join("late"), "written after the summary\n").unwrap();
            }
        },
    );
    assert_eq!(
        fs::read_to_string(hotfix.join("late")).unwrap(),
        "written after the summary\n"
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_says_nothing_was_removed_when_a_lock_lands_after_the_confirmation() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hotfix = linked_worktree(&core, "hotfix", &["-b", "hotfix"]);

    run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wjr".to_vec()),
            (
                vec!["Remove worktree / hotfix", "pair: worktree cannot"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "the worktree changed after it was inspected; nothing removed",
                    "Worktrees / core",
                    "pair: hotfix locked:",
                    "absent: Remove worktree /",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        |action| {
            if action == 1 {
                run_git(
                    &core,
                    &[
                        "worktree",
                        "lock",
                        "--reason",
                        "on loan",
                        hotfix.to_str().unwrap(),
                    ],
                );
            }
        },
    );
    assert!(hotfix.is_dir());
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_reports_a_failed_creation_and_returns_after_enter() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let destination = core.parent().unwrap().join("core.worktrees/fix");

    let transcript = run_inventory_pty_actions_with_hook(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa".to_vec()),
            (vec!["Add worktree / core"], b"fix\r".to_vec()),
            (vec!["Add worktree / plan"], b"\r".to_vec()),
            (
                vec!["lager: Enter returns to the inventory. Ctrl-C exits 130."],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Worktree was not created",
                    "Worktrees / core",
                    "primary · clean",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Worktrees /", "selected: core"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                fs::create_dir_all(&destination).unwrap();
            }
        },
    );
    assert!(
        transcript.contains("lager: failed · worktree was not created: destination exists: "),
        "{transcript}"
    );
    assert!(
        transcript.contains("lager: remaining: nothing"),
        "{transcript}"
    );
    assert_screen_left(&transcript);
    assert_eq!(git_stdout(&core, &["branch", "--list", "fix"]), "");
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_ctrl_c_during_git_exits_130_with_the_terminal_restored() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hook = core.join(".git/hooks/post-checkout");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        "#!/bin/sh\nprintf 'hook-started\\033[0m\\n'\nexec /bin/sleep 30\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    let transcript = run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wa".to_vec()),
            (vec!["Add worktree / core"], b"fix\r".to_vec()),
            (vec!["Add worktree / plan"], b"\r".to_vec()),
            (vec!["hook-started"], b"\x03".to_vec()),
        ],
        32,
        132,
        130,
        |_| {},
    );
    assert!(
        transcript.contains(
            "lager: Git owns this terminal until it returns. Ctrl-C interrupts it and exits 130."
        ),
        "{transcript}"
    );
    assert_eq!(transcript.matches("\x1b[?1049h").count(), 1);
    assert_screen_left(&transcript);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_keys_follow_the_configured_maps() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        "\n[inventory.keys.normal]\nworktrees = [\"W\"]\n\n[inventory.keys.worktrees]\nadd = [\"n\"]\nremove = [\"x\"]\n",
    );
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            // The default key is dead: Help opens from normal mode, not from the popup.
            (POPUP_READY.to_vec(), b"w?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "Normal mode",
                    "pair: W Worktrees",
                    "Worktrees of the current repository (config: worktrees = [\"W\"])",
                    "pair: worktrees j/k",
                    "j/k select · n add · x remove · c copy path · Esc close",
                    "absent: Worktrees /",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Keyboard /"], b"W".to_vec()),
            (
                vec![
                    "Worktrees / core",
                    "j/k select · n add · x remove · c copy · Esc close",
                ],
                b"ar".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Add worktree", "absent: Remove"],
                b"?".to_vec(),
            ),
            (
                vec![
                    "Worktrees mode",
                    "add a worktree… (config: add = [\"n\"])",
                    "remove the selected worktree (config: remove = [\"x\"])",
                    "pair: c copy",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Keyboard /"],
                b"n".to_vec(),
            ),
            (vec!["Add worktree / core"], b"\x1b".to_vec()),
            (
                vec!["Add worktree cancelled · nothing changed"],
                b"\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Worktrees /",
                    "absent: Add worktree",
                ],
                b"q".to_vec(),
            ),
        ],
        40,
        132,
    );

    // A key the configuration gives to another action wins over the later default; the menu
    // still reaches the popup, and Help names the action left without a key.
    fs::write(
        &config,
        "root = \"repos\"\n\n[inventory.keys.normal]\nmark = [\"w\"]\n\n[inventory.keys.worktrees]\ndown = [\"a\"]\n",
    )
    .unwrap();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (
                vec!["1 marked · 0 hidden", "absent: Worktrees /"],
                b"w?".to_vec(),
            ),
            (
                vec![
                    "Normal mode",
                    "No key in normal mode: worktrees.",
                    "No key in worktrees mode: add.",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Keyboard /"],
                b"mjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Worktrees / core",
                    "a/k select · r remove · c copy · Esc close",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        40,
        132,
    );

    for (settings, expected) in [
        (
            "[inventory.keys.worktrees]\ncancel = []",
            "inventory worktrees.cancel must remain reachable",
        ),
        (
            "[inventory.keys.worktrees]\nadd = [\"x\"]\nremove = [\"x\"]",
            "[inventory.keys.worktrees] binds x to add and to remove",
        ),
        (
            "[inventory.keys.worktrees]\nmark = [\"x\"]",
            "unknown inventory action `worktrees.mark`",
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
        assert!(!transcript.contains("\x1b[?1049h"), "{settings}");
    }
}

#[cfg(unix)]
const TOO_SMALL: &str = "Too small to choose here · enlarge the terminal";

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_never_removes_while_losses_and_choices_are_off_screen() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let draft = dirty_worktree(&core);

    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"wjr".to_vec()),
            // 80×10: moving to the removing choice and choosing it does nothing.
            (
                vec!["CONFIRM · 1 row", "Remove worktree / draft", TOO_SMALL],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Remove unavailable: enlarge the terminal to show the whole summary",
                    "Remove worktree / draft",
                ],
                b"k".to_vec(),
            ),
            // At 80×24 the same confirmation shows everything, the losses first.
            (
                vec![
                    "pair: losses 1",
                    "1 modified file, 1 untracked file",
                    "pair: path ~/repos/org/core.worktrees/draft",
                    "absent: default",
                    "pair: worktree cannot",
                    "cannot be undone",
                    "absent: Too small",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Remove cancelled · nothing changed", "Worktrees / core"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        10,
        80,
        0,
        |action, terminal, _| {
            assert!(draft.join("notes").exists(), "removed at action {action}");
            if action == 2 {
                resize_pty(terminal, 24, 80);
            }
        },
    );
    assert_eq!(
        fs::read_to_string(draft.join("notes")).unwrap(),
        "untracked\n"
    );
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_acts_at_80x12_only_once_the_choices_and_fields_fit() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let hotfix = linked_worktree(&core, "hotfix", &["-b", "hotfix"]);
    let planned = core.parent().unwrap().join("core.worktrees/fix");

    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"wjr".to_vec()),
            // 80×12: the choices are pinned, the losses line is not in view.
            (
                vec![
                    "Remove worktree / hotfix",
                    "absent: default",
                    "Remove this worktree",
                    TOO_SMALL,
                    "absent: losses",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec!["Remove unavailable: enlarge the terminal to show the whole summary"],
                b"j".to_vec(),
            ),
            // 80×14: losses and path are in view, the kept branch is not; still inert.
            (
                vec![
                    "pair: losses none",
                    "pair: path ~/repos/org/core.worktrees/hotfix",
                    "absent: hotfix · kept",
                    TOO_SMALL,
                    "absent: Remove unavailable",
                ],
                b"\r".to_vec(),
            ),
            (
                vec!["Remove unavailable: enlarge the terminal to show the whole summary"],
                b"j".to_vec(),
            ),
            // 80×15: path, branch, losses and both choices fit, and the focused choice removes.
            (
                vec![
                    "pair: losses none",
                    "pair: path ~/repos/org/core.worktrees/hotfix",
                    "hotfix · kept",
                    "absent: default",
                    "absent: Too small",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/hotfix · branch hotfix kept",
                    "primary · clean",
                ],
                b"a".to_vec(),
            ),
            (vec!["Add worktree / core"], b"fix\r".to_vec()),
            (
                vec![
                    "Add worktree / plan",
                    "Create worktree",
                    "absent: default",
                    "absent: Too small",
                ],
                b"k".to_vec(),
            ),
            // Shrunk again, the plan is cut and its default does not create.
            (
                vec!["Add worktree / plan", "Create worktree", TOO_SMALL],
                b"\r".to_vec(),
            ),
            (
                vec!["Add unavailable: enlarge the terminal to show the whole plan"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add worktree / core", "absent: / plan"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add worktree cancelled · nothing changed"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        12,
        80,
        0,
        |action, terminal, _| match action {
            2 => resize_pty(terminal, 14, 80),
            4 => {
                assert!(hotfix.join("README").exists());
                resize_pty(terminal, 15, 80);
            }
            8 => resize_pty(terminal, 12, 80),
            _ => {}
        },
    );
    assert!(!hotfix.exists());
    assert_eq!(git_stdout(&core, &["branch", "--list", "hotfix"]), "hotfix");
    assert!(!planned.exists());
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_reports_what_a_failed_removal_left() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let draft = dirty_worktree(&core);
    // Git can empty the worktree but not delete its directory.
    let container = draft.parent().unwrap().to_path_buf();
    fs::set_permissions(&container, fs::Permissions::from_mode(0o555)).unwrap();

    let transcript = run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"wjr".to_vec()),
            (LOSS_CONFIRMATION.to_vec(), b"j\r".to_vec()),
            (
                vec!["lager: Enter returns to the inventory. Ctrl-C exits 130."],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Worktree removal failed: git worktree remove failed:",
                    "Worktrees / core",
                    "primary · clean",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
    );
    fs::set_permissions(&container, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        transcript.contains("lager: failed · worktree removal failed: git worktree remove failed:"),
        "{transcript}"
    );
    assert!(
        transcript.contains("lager: remaining: directory "),
        "{transcript}"
    );
    assert!(
        !transcript.contains("remaining: worktree metadata"),
        "{transcript}"
    );
    assert!(!transcript.contains("was not removed"), "{transcript}");
    assert!(!draft.join("notes").exists());
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_starts_on_the_primary_and_scrolls_every_blocker_into_view() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    // Sorts before the primary and carries three blockers, with hints long enough to scroll.
    let first = home.join("aaa").join("0".repeat(200)).join("first");
    run_git(
        &core,
        &["worktree", "add", "-q", "--detach", first.to_str().unwrap()],
    );
    run_git(
        &first,
        &["commit", "-q", "--allow-empty", "-m", "unreachable"],
    );
    run_git(
        &core,
        &[
            "worktree",
            "lock",
            "--reason",
            "on loan",
            first.to_str().unwrap(),
        ],
    );
    init_repo_without_origin(&first.join("vendor/lib"));

    let mut scrolled = String::new();
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[
            (POPUP_READY.to_vec(), b"w".to_vec()),
            (vec!["Worktrees / core", "primary · clean"], b"kr".to_vec()),
            (
                vec![
                    "Remove worktree / blocked",
                    "worktree is locked: on loan",
                    "scroll",
                ],
                vec![b'j'; 30],
            ),
            (
                vec![
                    "Remove worktree / blocked",
                    "nested repository vendor/lib",
                    "j/k scroll · Esc back",
                    "absent: Nothing was removed",
                    "absent: path ",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Remove worktree /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        24,
        80,
        0,
        |action, _, frame| {
            if action == 3 {
                scrolled = screen_text(frame, 24, 80);
            }
        },
    );
    // The last blocker is the last body line: the scroll reaches the end of the body.
    let body = scrolled
        .lines()
        .filter_map(|line| line.split('│').nth(1).map(str::trim))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let hints = body
        .iter()
        .position(|line| *line == "j/k scroll · Esc back")
        .unwrap_or_else(|| panic!("{scrolled}"));
    assert!(
        body[hints - 1].ends_with("nested repository vendor/lib"),
        "{scrolled}"
    );
    assert!(first.join("vendor/lib/README").exists());
}

/// A Bitbucket Cloud stub with one workspace: `team/tool` and the archived `team/attic`. It
/// numbers its requests and answers those at or past `open_from`; the others wait.
#[cfg(unix)]
struct BitbucketStub {
    base: String,
    open_from: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    answered: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(unix)]
impl BitbucketStub {
    fn start() -> Self {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        use std::sync::{Arc, Mutex};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stub = Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            open_from: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
            requests: Arc::new(Mutex::new(Vec::new())),
            answered: Arc::new(AtomicUsize::new(0)),
        };
        let (open_from, fail, requests, answered) = (
            Arc::clone(&stub.open_from),
            Arc::clone(&stub.fail),
            Arc::clone(&stub.requests),
            Arc::clone(&stub.answered),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (open_from, fail, requests, answered) = (
                    Arc::clone(&open_from),
                    Arc::clone(&fail),
                    Arc::clone(&requests),
                    Arc::clone(&answered),
                );
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 1024];
                    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(count) if count > 0 => bytes.extend_from_slice(&buffer[..count]),
                            _ => break,
                        }
                    }
                    let path = String::from_utf8_lossy(&bytes)
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .to_owned();
                    let sequence = {
                        let mut requests = requests.lock().unwrap();
                        requests.push(path.clone());
                        requests.len() - 1
                    };
                    while sequence < open_from.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let repository = |name: &str, archived: bool| {
                        format!(
                            r#"{{"is_archived":{archived},"links":{{"clone":[{{"name":"ssh","href":"ssh://git@bitbucket.org/team/{name}.git"}}]}}}}"#
                        )
                    };
                    let (status, body) = match path.as_str() {
                        _ if fail.load(Ordering::Relaxed) => {
                            ("500 Internal Server Error", "{}".to_owned())
                        }
                        "/2.0/user/permissions/workspaces" => (
                            "200 OK",
                            r#"{"values":[{"workspace":{"slug":"team"}}],"next":null}"#.to_owned(),
                        ),
                        "/2.0/repositories/team" => (
                            "200 OK",
                            format!(
                                r#"{{"values":[{},{}],"next":null}}"#,
                                repository("tool", false),
                                repository("attic", true)
                            ),
                        ),
                        _ => ("404 Not Found", "{}".to_owned()),
                    };
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                    answered.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        stub
    }

    fn hold(&self) {
        self.open_from.store(usize::MAX, Ordering::Relaxed);
    }

    fn open_from(&self, sequence: usize) {
        self.open_from.store(sequence, Ordering::Relaxed);
    }

    fn requests(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn wait_for_answers(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.answered.load(Ordering::Relaxed) < count {
            assert!(Instant::now() < deadline, "{:?}", self.requests.lock());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Two providers and three local rows. A `gh` stub answers the viewer catalog with `me/zebra`
/// and `org/local`, and the wildcard `github.com/org/*` with `active`, the archived `old`, the
/// excluded `skip`, the explicitly declared `explicit` and the cloned `local`. With every
/// provider ready the inventory shows six rows, eight with archives. A repository lookup
/// fails, so it leaves the remote state unknown and the catalog request count alone.
#[cfg(unix)]
struct RemoteFixture {
    _temp: tempfile::TempDir,
    home: std::path::PathBuf,
    root: std::path::PathBuf,
    config: std::path::PathBuf,
    tools: std::path::PathBuf,
    state: std::path::PathBuf,
    bitbucket: BitbucketStub,
}

#[cfg(unix)]
const REMOTE_CONFIG: &str = r#"root = "repos"

[providers."github.com"]
preset = "github"

[providers."bitbucket.org"]
preset = "bitbucket-cloud"
api_url = "{BASE}"

[[repositories]]
url = "github.com/org/explicit"

[[repositories]]
url = "github.com/org/*"
exclude = ["skip"]
"#;

#[cfg(unix)]
impl RemoteFixture {
    fn new() -> Self {
        let temp = temp_dir();
        let home = temp.path().join("home");
        let root = home.join("repos");
        let config = temp.path().join("config.toml");
        let tools = temp.path().join("tools");
        let state = temp.path().join("state");
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(&state).unwrap();
        let bitbucket = BitbucketStub::start();
        fs::write(&config, REMOTE_CONFIG.replace("{BASE}", &bitbucket.base)).unwrap();
        init_repo(
            &root.join("org/local"),
            "git@github.com:org/local.git",
            true,
        );
        let node = |owner: &str, name: &str, archived: bool| {
            format!(
                r#"{{"name":"{name}","isArchived":{archived},"sshUrl":"git@github.com:{owner}/{name}.git","owner":{{"login":"{owner}"}}}}"#
            )
        };
        let page = |field: &str, nodes: &[String]| {
            format!(
                r#"[{{"data":{{"{field}":{{"repositories":{{"nodes":[{}],"pageInfo":{{"hasNextPage":false,"endCursor":null}}}}}}}}}}]"#,
                nodes.join(",")
            )
        };
        fs::write(
            state.join("viewer.json"),
            page(
                "viewer",
                &[node("me", "zebra", false), node("org", "local", false)],
            ),
        )
        .unwrap();
        fs::write(
            state.join("organization.json"),
            page(
                "organization",
                &[
                    node("org", "active", false),
                    node("org", "old", true),
                    node("org", "skip", false),
                    node("org", "explicit", false),
                    node("org", "local", false),
                ],
            ),
        )
        .unwrap();
        let directory = shell_word(&state);
        let gh = tools.join("gh");
        fs::write(
            &gh,
            format!(
                "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then exit 1; fi\n\
                 case \"$*\" in *viewer*) kind=viewer ;; *) kind=organization ;; esac\n\
                 echo \"$kind\" >> {directory}/gh.log\n\
                 echo $$ >> {directory}/gh.pids\n\
                 while [ -e {directory}/gh.hold ]; do /bin/sleep 0.02; done\n\
                 if [ -e {directory}/gh.fail-$kind ]; then echo 'gh: rate limited' >&2; exit 1; fi\n\
                 exec cat {directory}/$kind.json\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _temp: temp,
            home,
            root,
            config,
            tools,
            state,
            bitbucket,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = support::lager_with_path(&self.home, &self.config, Some(&self.tools));
        command.args(args);
        command
    }

    fn hold_gh(&self) {
        fs::write(self.state.join("gh.hold"), "").unwrap();
    }

    fn release_gh(&self) {
        fs::remove_file(self.state.join("gh.hold")).unwrap();
    }

    /// How often `gh` ran; one fetch of github.com runs it twice.
    fn gh_requests(&self) -> usize {
        fs::read_to_string(self.state.join("gh.log"))
            .map(|log| log.lines().count())
            .unwrap_or(0)
    }
}

/// Opens "Remote discovery…" from the action menu.
#[cfg(unix)]
const REMOTE_MENU: &[u8] = b"mjjjjjjjjj\r";

#[cfg(unix)]
const REMOTE_READY: [&str; 3] = [
    "NORMAL · 6 rows",
    "bitbucket.org: ready, 1 candidate",
    "github.com: ready, 3 candidates",
];

/// The regions every remote board draws, top to bottom: header bar, source line, search row,
/// table, status line and hint footer, which is empty under a popup.
#[cfg(unix)]
fn assert_remote_regions(screen: &str, header: &str, source: &str, status: &str, footer: &str) {
    let lines = screen.split('\n').collect::<Vec<_>>();
    assert!(
        lines[0].starts_with("  LAGER / inventory") && lines[0].ends_with(header),
        "{screen}"
    );
    assert_eq!(lines[1], format!("  {source}"), "{screen}");
    assert!(lines[2].starts_with("  / "), "{screen}");
    assert_eq!(lines[3], "", "{screen}");
    assert!(lines[4].trim_start().starts_with("repository"), "{screen}");
    let last = lines.len() - 1;
    assert!(lines[last].contains(footer), "{screen}");
    assert_eq!(lines[last].is_empty(), footer.is_empty(), "{screen}");
    assert_eq!(lines[last - 1], format!("  {status}"), "{screen}");
}

#[cfg(unix)]
#[test]
fn inventory_remote_keeps_local_rows_usable_while_providers_load_and_keeps_rows_beside_a_failure() {
    let fixture = RemoteFixture::new();
    fixture.hold_gh();
    fixture.bitbucket.hold();
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec![
                    "NORMAL · 2 rows",
                    "Local: ready",
                    "bitbucket.org: loading…",
                    "github.com: loading…",
                    "Remote on · local rows stay usable while providers load",
                ],
                b"kk/explicit\r".to_vec(),
            ),
            // Search, navigation and marks work on the local rows while both providers hang.
            (
                vec!["NORMAL · 1 row", "github.com: loading…"],
                b"\x7fjk ".to_vec(),
            ),
            (
                vec!["NORMAL · 2 rows", "1 marked · 0 hidden", "loading…"],
                b"j".to_vec(),
            ),
            // The first provider's rows arrive while the second still loads.
            (
                vec![
                    "NORMAL · 5 rows",
                    "github.com: ready, 3 candidates",
                    "bitbucket.org: loading…",
                    "1 marked · 0 hidden",
                    "remote only",
                ],
                b"j".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 5 rows · 1 problem",
                    "bitbucket.org: failed",
                    "! bitbucket.org failed: bitbucket request failed with status 500",
                    "github.com: ready, 3 candidates",
                    "1 marked · 0 hidden",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action, _, frame| {
            frames.push(screen_text(frame, 32, 132));
            match action {
                2 => fixture.release_gh(),
                3 => {
                    fixture.bitbucket.fail.store(true, Ordering::Relaxed);
                    fixture.bitbucket.open_from(0);
                }
                _ => {}
            }
        },
    );
    // RemoteLoading: local rows under a source line with per-provider progress.
    let loading = &frames[0];
    let source = loading.lines().nth(1).unwrap();
    assert!(
        source.starts_with("  Local: ready · bitbucket.org: loading… ")
            && source.contains(" s · github.com: loading… ")
            && source.ends_with(" s"),
        "{loading}"
    );
    assert_remote_regions(
        loading,
        "NORMAL · 2 rows",
        source.trim_start(),
        "Remote on · local rows stay usable while providers load · R refreshes local/config state only",
        "q quit",
    );
    assert_eq!(inventory_rows(loading).len(), 2, "{loading}");
    let partial = inventory_rows(&frames[3]);
    assert_eq!(
        partial
            .iter()
            .filter(|row| row[1] == "remote only")
            .map(|row| (row[0].as_str(), row[4].as_str()))
            .collect::<Vec<_>>(),
        [
            ("zebra", "unregistered"),
            ("active", "wildcard"),
            ("skip", "excluded")
        ],
        "{}",
        frames[3]
    );
    assert_eq!(inventory_rows(&frames[4]), partial, "{}", frames[4]);
}

#[cfg(unix)]
#[test]
fn inventory_remote_menu_retries_only_the_failed_provider() {
    let fixture = RemoteFixture::new();
    fixture.bitbucket.fail.store(true, Ordering::Relaxed);
    let mut frames = Vec::new();
    let mut before_retry = (0, 0);
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec![
                    "NORMAL · 5 rows · 1 problem",
                    "github.com: ready, 3 candidates",
                    "bitbucket.org: failed",
                ],
                [&b"kk"[..], REMOTE_MENU].concat(),
            ),
            (
                vec![
                    "MENU · 5 rows · 1 problem",
                    "Remote discovery",
                    "pair: providers bitbucket.org",
                    "absent: bitbucket.org only",
                    "Disable remote",
                    "j/k select · Enter choose · Esc back",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 6 rows · 1 problem",
                    "bitbucket.org: ready, 1 candidate",
                    "github.com: ready, 3 candidates",
                    "absent: Remote discovery",
                ],
                REMOTE_MENU.to_vec(),
            ),
            // Nothing failed any more: retrying is unavailable and asks no provider.
            // `q` closes a menu level; two ESC bytes in a row would read as one Alt chord.
            (
                vec!["Retry failed providers", "none failed"],
                b"\rqqq".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action, _, frame| {
            frames.push(screen_text(frame, 32, 132));
            if action == 1 {
                fixture.bitbucket.fail.store(false, Ordering::Relaxed);
                before_retry = (fixture.gh_requests(), fixture.bitbucket.requests());
            }
        },
    );
    assert_eq!(before_retry, (2, 1));
    assert_eq!(fixture.gh_requests(), 2, "retry asked the ready provider");
    assert_eq!(fixture.bitbucket.requests(), 3);

    // RemoteControls: the menu over the table, the failure as the current state.
    let controls = &frames[1];
    assert_remote_regions(
        controls,
        "MENU · 5 rows · 1 problem",
        "Local: ready · bitbucket.org: failed · github.com: ready, 3 candidates",
        "! bitbucket.org failed: bitbucket request failed with status 500 Internal Server Error",
        "",
    );
    let position = |text: &str| {
        controls
            .lines()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("{text}: {controls}"))
    };
    let order = [
        position("Remote discovery"),
        position("failed · bitbucket request failed"),
        position("ready · 3 candidates · "),
        position("Retry failed providers"),
        position("Refresh all enabled providers"),
        position("[ ] Include archived"),
        position("Disable remote"),
        position("j/k select · Enter choose · Esc back"),
    ];
    assert!(order.is_sorted(), "{order:?}: {controls}");
    for absent in [
        "fetched",
        "shown stale",
        "refetches",
        "refreshes local/config only",
        "never registers",
    ] {
        assert!(!controls.contains(absent), "{absent}: {controls}");
    }
    assert!(
        controls
            .lines()
            .any(|line| line.contains("Disable remote") && line.contains("hides 3 rows")),
        "{controls}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_add_shows_a_failed_provider_as_its_status() {
    let fixture = RemoteFixture::new();
    fixture.bitbucket.fail.store(true, Ordering::Relaxed);
    run_inventory_pty_with_exit(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec!["NORMAL · 5 rows · 1 problem", "bitbucket.org: failed"],
                b"a".to_vec(),
            ),
            (
                vec![
                    "Enter preview · Tab focus · Esc back",
                    "! bitbucket.org failed: bitbucket request failed",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Enter preview"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_remote_refresh_keeps_cached_rows_stale_while_every_provider_reloads() {
    let fixture = RemoteFixture::new();
    let mut rows = Vec::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (REMOTE_READY.to_vec(), [&b"kk"[..], REMOTE_MENU].concat()),
            (
                vec!["Refresh all enabled providers", "absent: cached rows stay"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 6 rows",
                    "bitbucket.org: refreshing, 1 cached (stale)",
                    "github.com: refreshing, 3 cached (stale)",
                ],
                b"j".to_vec(),
            ),
            (REMOTE_READY.to_vec(), b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action, _, frame| {
            rows.push(inventory_rows(&screen_text(frame, 32, 132)));
            match action {
                1 => {
                    assert_eq!(
                        (fixture.gh_requests(), fixture.bitbucket.requests()),
                        (2, 2)
                    );
                    fixture.hold_gh();
                    fixture.bitbucket.hold();
                }
                2 => {
                    fixture.release_gh();
                    fixture.bitbucket.open_from(0);
                }
                _ => {}
            }
        },
    );
    assert_eq!(rows[0].len(), 6);
    // Staleness shows in the source line only: the cached rows stay as they were.
    assert_eq!(rows[2], rows[0]);
    assert_eq!(rows[3], rows[0]);
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (4, 4)
    );
}

#[cfg(unix)]
#[test]
fn inventory_remote_disable_hides_candidates_removes_their_marks_and_keeps_the_cache() {
    let fixture = RemoteFixture::new();
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (REMOTE_READY.to_vec(), b"kkmjjjjjj\r".to_vec()),
            (vec!["Mark visible concrete rows"], b"j\r".to_vec()),
            (vec!["6 marked · 0 hidden"], REMOTE_MENU.to_vec()),
            (
                vec!["Disable remote", "hides 4 rows, removes 4 marks"],
                b"jjj\r".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 2 rows",
                    "Remote off · 4 remote-only rows hidden · 4 marks removed: ",
                    "2 marked · 0 hidden",
                    "remote off · 4 candidates cached for this run",
                    "absent: vanished",
                    "absent: remote only",
                ],
                REMOTE_MENU.to_vec(),
            ),
            // Enabling again brings the rows back; the marks stay removed.
            (vec!["Enable remote", "fetches 2 providers"], b"\r".to_vec()),
            (
                [&REMOTE_READY[..], &["2 marked · 0 hidden"][..]].concat(),
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |_, _, frame| frames.push(screen_text(frame, 32, 132)),
    );
    // RemoteOff: local and declaration rows, the cache named in the source line.
    let off = &frames[4];
    let status = off
        .lines()
        .find(|line| line.contains("Remote off · "))
        .unwrap();
    for name in ["tool", "zebra", "active", "skip"] {
        assert!(status.contains(name), "{off}");
    }
    assert_remote_regions(
        off,
        "NORMAL · 2 rows",
        "Local + declarations · remote off · 4 candidates cached for this run",
        status.trim_start(),
        "q quit",
    );
    let names = |frame: &str| {
        inventory_rows(frame)
            .into_iter()
            .map(|row| row[0].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(off), ["explicit", "local"], "{off}");
    assert_eq!(names(&frames[6]).len(), 6, "{}", frames[6]);
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (4, 4)
    );
}

#[cfg(unix)]
#[test]
fn inventory_opening_add_enables_remote_and_cancelling_add_leaves_it_on() {
    let fixture = RemoteFixture::new();
    let original = fs::read_to_string(&fixture.config).unwrap();
    fixture.hold_gh();
    fixture.bitbucket.hold();
    let wait_for_requests = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while fixture.gh_requests() < 1 || fixture.bitbucket.requests() < 1 {
            assert!(Instant::now() < deadline, "opening Add asked no provider");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let mut frames = Vec::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui"]),
        &[
            (
                vec!["NORMAL · 2 rows", "Local + declarations · remote off"],
                b"kka".to_vec(),
            ),
            // AddInput: the form is open, the providers load, typing works meanwhile.
            (
                vec![
                    "INPUT · 2 rows",
                    "Add / URL or reference",
                    "bitbucket.org: loading…",
                    "github.com: loading…",
                    "Remote enabled by Add · providers loading",
                    "absent: typed input works",
                ],
                b"org/thing".to_vec(),
            ),
            (vec!["org/thing█", "github.com: loading…"], b"\x1b".to_vec()),
            // Cancelling Add leaves remote discovery on.
            (
                vec![
                    "NORMAL · 2 rows",
                    "Add cancelled · nothing changed",
                    "Local: ready",
                    "bitbucket.org: loading…",
                    "github.com: loading…",
                    "absent: Add / URL",
                ],
                b"j".to_vec(),
            ),
            (REMOTE_READY.to_vec(), b"a".to_vec()),
            // Opening Add again asks no provider; "Browse remote candidates" closes the form.
            (
                [
                    &REMOTE_READY[1..],
                    &[
                        "Add / URL or reference",
                        "Browse remote candidates",
                        "absent: closes this form",
                        "absent: Remote enabled by Add",
                    ][..],
                ]
                .concat(),
                b"\t\r".to_vec(),
            ),
            (
                [&REMOTE_READY[..], &["absent: Add / URL"][..]].concat(),
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |action, _, frame| {
            frames.push(screen_text(frame, 32, 132));
            match action {
                0 => assert_eq!(
                    (fixture.gh_requests(), fixture.bitbucket.requests()),
                    (0, 0)
                ),
                2 => wait_for_requests(),
                3 => {
                    fixture.release_gh();
                    fixture.bitbucket.open_from(0);
                }
                _ => {}
            }
        },
    );
    let lines = frames[1].lines().collect::<Vec<_>>();
    assert!(
        lines[1].starts_with("  Local: ready · bitbucket.org: loading… "),
        "{}",
        frames[1]
    );
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (2, 2)
    );
    // Discovery only reads: nothing was registered or cloned.
    assert_eq!(fs::read_to_string(&fixture.config).unwrap(), original);
    assert!(!fixture.root.join("org/active").exists());
}

#[cfg(unix)]
#[test]
fn inventory_opening_add_from_the_menu_enables_remote_and_without_providers_asks_nothing() {
    let fixture = RemoteFixture::new();
    run_inventory_pty_actions(
        &mut fixture.command(&["tui"]),
        &[
            (
                vec!["NORMAL · 2 rows", "Local + declarations · remote off"],
                b"kkmjjjjjjj\r".to_vec(),
            ),
            (
                [&REMOTE_READY[1..], &["Add / URL or reference"][..]].concat(),
                b"\x1b".to_vec(),
            ),
            (
                [&REMOTE_READY[..], &["Add cancelled · nothing changed"][..]].concat(),
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (2, 2)
    );

    // No provider configured: remote stays off, nothing is asked, nothing is a problem.
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    support::clear_outbound_requests();
    run_inventory_pty_actions(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["NORMAL · 0 rows", "absent: Scanning"], b"a".to_vec()),
            (
                vec![
                    "Add / URL or reference",
                    "Scan complete · 0 repositories · 0 declarations",
                    "absent: nothing is saved yet",
                    "Local + declarations · remote off",
                    "absent: problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Add cancelled · nothing changed", "absent: problem"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert!(support::outbound_requests().is_empty());
}

#[cfg(unix)]
#[test]
fn inventory_remote_archive_toggle_refetches_and_survives_disabling() {
    let fixture = RemoteFixture::new();
    let with_archives = [
        "NORMAL · 8 rows",
        "attic",
        "bitbucket.org: ready, 2 candidates",
        "github.com: ready, 4 candidates",
    ];
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                [&REMOTE_READY[..], &["absent: attic"][..]].concat(),
                [&b"kk"[..], REMOTE_MENU].concat(),
            ),
            (
                vec!["[ ] Include archived", "absent: refetches"],
                b"jj\r".to_vec(),
            ),
            (with_archives.to_vec(), REMOTE_MENU.to_vec()),
            (
                vec!["[x] Include archived", "Disable remote"],
                b"jjj\r".to_vec(),
            ),
            (vec!["NORMAL · 2 rows", "remote off"], REMOTE_MENU.to_vec()),
            // The preference outlives disabling and applies to the next fetch.
            (
                vec!["[x] Include archived", "Enable remote"],
                b"\r".to_vec(),
            ),
            (with_archives.to_vec(), b"q".to_vec()),
        ],
        32,
        132,
        |action| {
            if action == 1 {
                assert_eq!(
                    (fixture.gh_requests(), fixture.bitbucket.requests()),
                    (2, 2)
                );
            }
        },
    );
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (6, 6)
    );

    run_inventory_pty_actions(
        &mut fixture.command(&["tui", "--remote", "--include-archived"]),
        &[
            (with_archives.to_vec(), b"/attic\r\r".to_vec()),
            (
                vec!["Inspect / attic", "pair: Remote archived"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL ·", "absent: Inspect /"], b"q".to_vec()),
        ],
        32,
        132,
    );
}

#[cfg(unix)]
#[test]
fn inventory_local_refresh_merges_the_cache_into_the_new_config_without_a_provider_request() {
    let fixture = RemoteFixture::new();
    let mut rows = Vec::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (REMOTE_READY.to_vec(), b"kkj".to_vec()),
            (REMOTE_READY.to_vec(), b"R".to_vec()),
            (
                // The failed lookup of the new `added` row is a session problem, so it exits 1.
                vec![
                    "NORMAL · 7 rows · 1 problem",
                    "No active operation",
                    "bitbucket.org: ready, 1 candidate",
                    "github.com: ready, 3 candidates",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |action, _, frame| {
            rows.push(inventory_rows(&screen_text(frame, 32, 132)));
            if action == 1 {
                let config = REMOTE_CONFIG
                    .replace("{BASE}", &fixture.bitbucket.base)
                    .replace("exclude = [\"skip\"]\n", "")
                    + "\n[[repositories]]\nurl = \"github.com/org/added\"\n";
                fs::write(&fixture.config, config).unwrap();
            }
        },
    );
    let registration = |rows: &[Vec<String>], name: &str| {
        rows.iter()
            .find(|row| row[0] == name)
            .map(|row| (row[4].clone(), row[1].clone()))
    };
    assert_eq!(
        registration(&rows[1], "skip"),
        Some(("excluded".to_owned(), "remote only".to_owned()))
    );
    assert_eq!(
        registration(&rows[2], "skip"),
        Some(("wildcard".to_owned(), "remote only".to_owned()))
    );
    assert_eq!(
        registration(&rows[2], "added"),
        Some(("explicit".to_owned(), "missing".to_owned()))
    );
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (2, 2)
    );
}

#[cfg(unix)]
#[test]
fn inventory_remote_ignores_answers_to_requests_that_were_invalidated() {
    // Disabled while loading: the late answer neither shows rows nor joins the cache.
    let fixture = RemoteFixture::new();
    fixture.bitbucket.hold();
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec![
                    "NORMAL · 5 rows",
                    "github.com: ready, 3 candidates",
                    "bitbucket.org: loading…",
                ],
                [&b"kk"[..], REMOTE_MENU].concat(),
            ),
            (vec!["Disable remote", "hides 3 rows"], b"jjj\r".to_vec()),
            (
                vec![
                    "NORMAL · 2 rows",
                    "remote off · 3 candidates cached for this run",
                ],
                b"j".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 2 rows",
                    "remote off · 3 candidates cached for this run",
                ],
                REMOTE_MENU.to_vec(),
            ),
            (
                vec!["Enable remote", "pair: bitbucket.org not", "off · fetched"],
                b"qqq".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                fixture.bitbucket.open_from(0);
                fixture.bitbucket.wait_for_answers(2);
                std::thread::sleep(Duration::from_millis(200));
            }
        },
    );
    assert_eq!(fixture.bitbucket.requests(), 2);

    // An archive toggle supersedes the running request; the request without archives is
    // answered last and must not replace the answer with them.
    let fixture = RemoteFixture::new();
    fixture.bitbucket.hold();
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec!["github.com: ready, 3 candidates", "bitbucket.org: loading…"],
                [&b"kk"[..], REMOTE_MENU].concat(),
            ),
            (vec!["[ ] Include archived"], b"jj\r".to_vec()),
            (
                vec!["github.com: ready, 4 candidates", "bitbucket.org: loading…"],
                b"j".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 8 rows",
                    "attic",
                    "bitbucket.org: ready, 2 candidates",
                ],
                b"j".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 8 rows",
                    "attic",
                    "bitbucket.org: ready, 2 candidates",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| match action {
            // The second request, with archives, is answered first.
            2 => {
                let deadline = Instant::now() + Duration::from_secs(10);
                while fixture.bitbucket.requests() < 2 {
                    assert!(Instant::now() < deadline, "the toggle sent no request");
                    std::thread::sleep(Duration::from_millis(5));
                }
                fixture.bitbucket.open_from(1);
            }
            3 => {
                fixture.bitbucket.open_from(0);
                fixture.bitbucket.wait_for_answers(4);
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => {}
        },
    );
    assert_eq!(fixture.bitbucket.requests(), 4);
}

#[cfg(unix)]
#[test]
fn inventory_remote_rows_agree_with_status_remote() {
    let fixture = RemoteFixture::new();
    for (flags, ready, extras) in [
        (&["--remote"][..], "NORMAL · 6 rows", 3),
        (
            &["--remote", "--include-archived"][..],
            "NORMAL · 8 rows",
            4,
        ),
    ] {
        let transcript = run_inventory_pty_with_size(
            fixture.command(&["tui"]).args(flags),
            &[ready, "absent: loading…", "No active operation"],
            b"q",
            40,
            220,
        );
        let (_, screen) = inventory_grid(&screen_text(&transcript, 40, 220));
        let status = output(fixture.command(&["status"]).args(flags));
        assert!(status.status.success(), "{status:?}");
        let printed = status_rows(&String::from_utf8(status.stdout).unwrap());
        assert!(
            printed.iter().any(|row| row[2] == "remote only"),
            "{printed:?}"
        );
        // Every status row is on the screen with the same cells, in the same order.
        assert_eq!(
            screen
                .iter()
                .filter(|row| printed.contains(row))
                .collect::<Vec<_>>(),
            printed.iter().collect::<Vec<_>>(),
            "{flags:?}"
        );
        // What the screen adds are catalog candidates: repositories no wildcard registers.
        let candidates = screen
            .iter()
            .filter(|row| !printed.contains(row))
            .collect::<Vec<_>>();
        assert_eq!(candidates.len(), extras, "{candidates:?}");
        for row in candidates {
            assert!(
                row[2] == "remote only" && ["unregistered", "excluded"].contains(&row[5].as_str()),
                "{row:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn inventory_remote_without_providers_starts_and_says_so() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    support::clear_outbound_requests();
    run_inventory_pty_with_size(
        support::lager(&home, &config).args(["tui", "--remote"]),
        &[
            "Local: ready · no providers configured",
            "No repositories yet.",
        ],
        b"q",
        24,
        132,
    );
    assert!(support::outbound_requests().is_empty());
}

#[cfg(unix)]
#[test]
fn inventory_registers_a_remote_candidate_without_asking_a_provider_again() {
    let fixture = RemoteFixture::new();
    run_inventory_pty_actions(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            // The candidate `tool` has the owner `team`, the last of the rows.
            (REMOTE_READY.to_vec(), b"jjjjjjjm\r".to_vec()),
            (
                vec!["Repository actions / tool", "Register"],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "RESULTS · 6 rows",
                    "Register: 1 succeeded",
                    "bitbucket.org: ready, 0 candidates",
                    "github.com: ready, 3 candidates",
                ],
                b"qq".to_vec(),
            ),
        ],
        32,
        132,
    );
    let config = fs::read_to_string(&fixture.config).unwrap();
    assert!(config.contains("bitbucket.org:team/tool.git"), "{config}");
    assert!(!fixture.root.join("team/tool").exists());
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (2, 2)
    );
}

#[cfg(unix)]
#[test]
fn inventory_quit_ends_a_hanging_provider_request() {
    let fixture = RemoteFixture::new();
    fixture.hold_gh();
    let mut quit_sent = Instant::now();
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(&["tui", "--remote"]),
        &[(
            vec![
                "NORMAL · 3 rows",
                "github.com: loading…",
                "bitbucket.org: ready, 1 candidate",
            ],
            b"q".to_vec(),
        )],
        32,
        132,
        |_| {
            wait_for_file(&fixture.state.join("gh.pids"));
            quit_sent = Instant::now();
        },
    );
    // The cancellation bound is 2 s; the margin covers a loaded machine reaping the process.
    let quit_took = quit_sent.elapsed();
    assert!(quit_took < Duration::from_secs(6), "{quit_took:?}");
    let pids = fs::read_to_string(fixture.state.join("gh.pids")).unwrap_or_else(|error| {
        panic!(
            "{error}: {:?}",
            fs::read_dir(&fixture.state)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>()
        )
    });
    assert_eq!(pids.lines().count(), 1, "{pids}");
    let pid: libc::pid_t = pids.trim().parse().unwrap();
    let deadline = quit_sent + Duration::from_secs(6);
    // SAFETY: signal 0 only asks whether the process exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "gh {pid} survived the quit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn inventory_remote_keeps_a_provider_catalog_when_one_of_its_wildcards_fails() {
    let fixture = RemoteFixture::new();
    let failing = fixture.state.join("gh.fail-organization");
    fs::write(&failing, "").unwrap();
    run_inventory_pty_with_exit(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            // The viewer catalog arrived; the wildcard's members did not.
            (
                vec![
                    "NORMAL · 4 rows · 1 problem",
                    "zebra",
                    "bitbucket.org: ready, 1 candidate",
                    "github.com: failed, 1 cached (stale)",
                    "! github.com failed: github.com/org/*: gh: rate limited",
                ],
                [&b"kk"[..], REMOTE_MENU].concat(),
            ),
            (
                vec!["pair: providers github.com", "absent: github.com only"],
                b"\r".to_vec(),
            ),
            (REMOTE_READY.to_vec(), b"q".to_vec()),
        ],
        32,
        132,
        1,
        |action| {
            if action == 1 {
                fs::remove_file(&failing).unwrap();
            }
        },
    );
    assert_eq!(
        (fixture.gh_requests(), fixture.bitbucket.requests()),
        (4, 2)
    );
}

/// A `.git/config` that is a FIFO holds the scan's traversal in `open`, as an unresponsive
/// network mount under the root does.
#[cfg(unix)]
fn blocked_scan_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::ffi::OsStrExt;
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let git = home.join("repos/stalled/.git");
    fs::create_dir_all(&git).unwrap();
    let fifo = std::ffi::CString::new(git.join("config").as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    fs::write(&config, "root = \"repos\"\n").unwrap();
    (temp, home, config)
}

/// Reads the PTY until the alternate screen is left, and returns how long that took.
#[cfg(unix)]
fn time_until_screen_left(terminal: &mut fs::File, since: Instant) -> Duration {
    let mut bytes = Vec::new();
    while !String::from_utf8_lossy(&bytes).contains("\x1b[?1049l")
        && since.elapsed() < Duration::from_secs(10)
    {
        read_pty(terminal, &mut bytes);
        std::thread::sleep(Duration::from_millis(5));
    }
    since.elapsed()
}

/// Exits through `key` or `signal` while the scan is blocked: the terminal is restored at once
/// and the process ends within the cancellation bound.
#[cfg(unix)]
fn exit_while_the_scan_is_blocked(key: &[u8], signal: Option<i32>, code: i32) {
    use std::os::fd::AsRawFd;
    let (_temp, home, config) = blocked_scan_fixture();
    let mut asked = None;
    let mut restored = Duration::ZERO;
    run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[(vec!["Scanning local repositories"], Vec::new())],
        24,
        132,
        code,
        |_, terminal, _| {
            let since = Instant::now();
            asked = Some(since);
            terminal.write_all(key).unwrap();
            if let Some(signal) = signal {
                // SAFETY: the PTY descriptor is live; the foreground PID belongs to this fixture.
                unsafe {
                    let foreground = libc::tcgetpgrp(terminal.as_raw_fd());
                    assert!(foreground > 0);
                    assert_eq!(libc::kill(foreground, signal), 0);
                }
            }
            restored = time_until_screen_left(terminal, since);
        },
    );
    let exited = asked.unwrap().elapsed();
    assert!(
        restored < Duration::from_secs(1),
        "the terminal was restored after {restored:?}"
    );
    // The 2 s cancellation bound, plus scheduling slack on a loaded machine.
    assert!(
        exited < Duration::from_millis(3000),
        "exited after {exited:?}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_quit_while_the_scan_is_blocked_restores_the_terminal_within_the_bound() {
    exit_while_the_scan_is_blocked(b"q", None, 0);
}

#[cfg(unix)]
#[test]
fn inventory_ctrl_c_while_the_scan_is_blocked_restores_the_terminal_within_the_bound() {
    exit_while_the_scan_is_blocked(b"\x03", None, 130);
}

#[cfg(unix)]
#[test]
fn inventory_sigint_while_the_scan_is_blocked_restores_the_terminal_within_the_bound() {
    exit_while_the_scan_is_blocked(b"", Some(libc::SIGINT), 130);
}

#[cfg(unix)]
#[test]
fn inventory_sigterm_while_the_scan_is_blocked_restores_the_terminal_within_the_bound() {
    exit_while_the_scan_is_blocked(b"", Some(libc::SIGTERM), 143);
}

#[cfg(unix)]
#[test]
fn inventory_refresh_keeps_drawing_while_the_previous_scan_is_blocked() {
    let (_temp, home, config) = blocked_scan_fixture();
    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (vec!["Scanning local repositories"], b"R?".to_vec()),
            (vec!["Keyboard / effective bindings"], b"\x03".to_vec()),
        ],
        24,
        132,
        130,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_sighup_restores_the_terminal_and_exits_129() {
    use std::os::fd::AsRawFd;
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    let transcript = run_inventory_pty_with_exit_observing(
        support::lager(&home, &config).arg("tui"),
        &[(vec!["Local + declarations"], Vec::new())],
        24,
        132,
        129,
        |_, terminal, _| {
            // SAFETY: the PTY descriptor is live; the foreground PID belongs to this fixture.
            unsafe {
                let foreground = libc::tcgetpgrp(terminal.as_raw_fd());
                assert!(foreground > 0);
                assert_eq!(libc::kill(foreground, libc::SIGHUP), 0);
            }
        },
    );
    assert!(transcript.contains("\x1b[?1049l"));
    assert!(transcript.contains("\x1b[?25h"));
    // Nothing was left running, so nothing is waited for.
    assert!(!transcript.contains("waiting up to"), "{transcript}");
}

/// A checkout `slow` whose `git status` writes `started` into the returned state directory and
/// then waits while `slow` exists there.
#[cfg(unix)]
fn slow_probe_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let tools = temp.path().join("tools");
    let state = temp.path().join("state");
    fs::create_dir_all(&tools).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("slow"), "").unwrap();
    fs::write(&config, "root = \"repos\"\n").unwrap();
    init_repo(
        &home.join("repos/slow"),
        "git@github.com:org/slow.git",
        true,
    );
    let state_word = shell_word(&state);
    fs::write(
        tools.join("git"),
        format!(
            "#!/bin/sh\ncase \"$2:$3\" in */slow:status) : > {state_word}/started; while [ -e {state_word}/slow ]; do /bin/sleep 0.05; done ;; esac\nexec {} \"$@\"\n",
            shell_word(&support::real_tool("git"))
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    (temp, home, config, tools, state)
}

#[cfg(unix)]
#[test]
fn inventory_config_touched_during_a_scan_still_reaches_a_complete_scan() {
    let (_temp, home, config, tools, state) = slow_probe_fixture();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["slow", "pending"], Vec::new()),
            (
                vec![
                    "Local + declarations",
                    "clean",
                    "Config changed on disk · reloaded",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 0 {
                // The write lands while the first probe is provably running.
                wait_for_file(&state.join("started"));
                // A registration from another shell, an editor save or a dotfile sync.
                fs::write(&config, "root = \"repos\"\n# touched\n").unwrap();
                fs::remove_file(state.join("slow")).unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_half_saved_config_during_a_scan_is_a_notice_and_the_repaired_save_completes() {
    let (_temp, home, config, tools, state) = slow_probe_fixture();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["slow", "pending"], Vec::new()),
            (
                vec![
                    "R retries · config changed on disk but did not load:",
                    "· previous configuration stays",
                    "absent: problem",
                ],
                Vec::new(),
            ),
            (
                vec![
                    "Local + declarations",
                    "clean",
                    "Config changed on disk · reloaded",
                    "absent: ! Config changed on disk · reloaded",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| match action {
            0 => {
                wait_for_file(&state.join("started"));
                fs::write(&config, "root = [\n").unwrap();
            }
            1 => {
                fs::remove_file(state.join("slow")).unwrap();
                fs::write(&config, "root = \"repos\"\n# repaired\n").unwrap();
            }
            _ => {}
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_broken_config_saved_during_a_scan_completes_the_scan_with_the_previous_one() {
    let (_temp, home, config, tools, state) = slow_probe_fixture();
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["slow", "pending"], Vec::new()),
            (
                vec![
                    "R retries · config changed on disk but did not load:",
                    "· previous configuration stays",
                ],
                Vec::new(),
            ),
            (
                vec![
                    "clean",
                    "absent: pending",
                    "absent: Scanning local repositories",
                    "R retries · config changed on disk but did not load:",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| match action {
            0 => {
                wait_for_file(&state.join("started"));
                fs::write(&config, "root = [\n").unwrap();
            }
            1 => fs::remove_file(state.join("slow")).unwrap(),
            _ => {}
        },
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), "root = [\n");
}

#[cfg(unix)]
#[test]
fn inventory_config_reload_waits_until_an_open_confirmation_closes() {
    let (_temp, home, config, tools, state) = slow_probe_fixture();
    tracked_repo(&home.join("repos/ok"), "git@github.com:org/ok.git");
    run_inventory_pty_actions_with_hook(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (vec!["ok", "clean", "pending"], b"k".to_vec()),
            (vec!["selected: ok", "pending"], b"r".to_vec()),
            // `x` is the new cancel key; with the old bindings still active it does nothing,
            // and Esc closes the dialog. A reload under the dialog would make `x` cancel and
            // Esc then quit, so the reload notice never shows.
            (
                vec!["CONFIRM", "pair: Esc cancel", "Remove 1 checkout"],
                b"x\x1b".to_vec(),
            ),
            (
                vec![
                    "NORMAL ·",
                    "Config changed on disk · reloaded",
                    "clean",
                    "absent: stale",
                    "selected: ok",
                ],
                b"r".to_vec(),
            ),
            (vec!["CONFIRM", "Remove 1 checkout"], b"x".to_vec()),
            (
                vec!["NORMAL ·", "Remove only cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        24,
        132,
        |action| {
            if action == 2 {
                wait_for_file(&state.join("started"));
                let mut text = fs::read_to_string(&config).unwrap();
                text.push_str("[inventory.keys.confirmation]\ncancel = [\"x\"]\n");
                fs::write(&config, text).unwrap();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn inventory_exit_shares_one_bound_between_a_blocked_scan_and_a_stuck_provider() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = RemoteFixture::new();
    fixture.bitbucket.hold();
    let git = fixture.root.join("stalled/.git");
    fs::create_dir_all(&git).unwrap();
    let fifo = std::ffi::CString::new(git.join("config").as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let mut asked = None;
    let transcript = run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[(
            vec!["Scanning local repositories", "bitbucket.org: loading"],
            b"q".to_vec(),
        )],
        32,
        132,
        0,
        |_, _, _| asked = Some(Instant::now()),
    );
    let exited = asked.unwrap().elapsed();
    // One 2 s budget for both waits, plus load headroom; two budgets in a row take 4 s.
    assert!(
        exited < Duration::from_millis(3000),
        "exited after {exited:?}"
    );
    assert!(
        transcript.contains("lager: waiting up to 2 s for background work to stop…"),
        "{transcript}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_refresh_drops_a_provider_answer_fetched_with_the_previous_wildcards() {
    let fixture = RemoteFixture::new();
    fixture.hold_gh();
    let pids = fixture.state.join("gh.pids");
    let mut screen = String::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(&["tui", "--remote"]),
        &[
            (
                vec!["bitbucket.org: ready", "github.com: loading"],
                b"R".to_vec(),
            ),
            // Checked while the old request is still held: `R` alone invalidates it.
            (
                vec!["NORMAL · 4 rows", "github.com: config changed, not fetched"],
                b"j".to_vec(),
            ),
            (
                vec!["github.com: config changed, not fetched"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |action, _, frame| match action {
            0 => {
                // The request is running with the previous wildcards when `R` arrives.
                wait_for_file(&pids);
                let config = REMOTE_CONFIG
                    .replace("{BASE}", &fixture.bitbucket.base)
                    .replace(
                        "[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"skip\"]\n",
                        "[[repositories]]\nurl = \"github.com/org/added\"\n",
                    );
                assert!(!config.contains("org/*"));
                fs::write(&fixture.config, config).unwrap();
            }
            1 => {
                fixture.release_gh();
                let deadline = Instant::now() + Duration::from_secs(10);
                while fs::read_to_string(&pids)
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|pid| pid.parse().ok())
                    // SAFETY: probing for existence sends no signal.
                    .any(|pid: i32| unsafe { libc::kill(pid, 0) } == 0)
                {
                    assert!(Instant::now() < deadline, "gh did not end");
                    std::thread::sleep(Duration::from_millis(10));
                }
                // The request thread hands any answer over right after `gh` ends.
                std::thread::sleep(Duration::from_secs(1));
            }
            _ => screen = screen_text(frame, 32, 132),
        },
    );
    assert!(
        !screen.contains("github.com: ready"),
        "an answer for the previous wildcards was accepted:\n{screen}"
    );
}

/// A home with a fake `gh` on PATH that logs its calls and keeps the repository's state.
#[cfg(unix)]
struct GhFixture {
    temp: tempfile::TempDir,
    home: std::path::PathBuf,
    config: std::path::PathBuf,
    bin: std::path::PathBuf,
    state: std::path::PathBuf,
    log: std::path::PathBuf,
}

#[cfg(unix)]
impl GhFixture {
    fn new(config: &str) -> Self {
        let temp = temp_dir();
        let home = temp.path().join("home");
        fs::create_dir_all(home.join("repos")).unwrap();
        let (bin, state) = support::install_fake_gh(temp.path());
        let path = temp.path().join("config.toml");
        fs::write(&path, format!("root = \"repos\"\n\n{config}")).unwrap();
        Self {
            log: temp.path().join("gh-args"),
            temp,
            home,
            config: path,
            bin,
            state,
        }
    }

    fn command(&self) -> Command {
        let mut command = support::lager_with_path(&self.home, &self.config, Some(&self.bin));
        command
            .env("GH_ARGS_LOG", &self.log)
            .env("GH_STATE", &self.state)
            .arg("tui");
        command
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The calls apart from `repo view`, which also looks up declared rows without a checkout.
    fn actions(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| !call.contains(" repo view "))
            .collect()
    }

    fn config_text(&self) -> String {
        fs::read_to_string(&self.config).unwrap()
    }

    /// A clean checkout of `org/<name>` whose branch is level with its remote-tracking ref.
    fn checkout(&self, name: &str) -> std::path::PathBuf {
        let path = self.home.join("repos/org").join(name);
        init_repo(&path, &format!("git@github.com:org/{name}.git"), true);
        run_git(&path, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run_git(&path, &["branch", "--set-upstream-to=origin/main"]);
        path
    }
}

#[cfg(unix)]
#[test]
fn inventory_github_rows_show_the_remote_group_and_bitbucket_rows_do_not() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n\n\
         [providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n\n\
         [[repositories]]\nurl = \"bitbucket.org/team/tool\"\n",
    );
    run_inventory_pty_actions(
        &mut fixture.command(),
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning"],
                b"/demo\r".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "/ demo"], b"m".to_vec()),
            (vec!["All actions"], b"\r".to_vec()),
            (
                vec![
                    "Repository actions / demo",
                    "On github.com",
                    "pair: Rename… not",
                    "pair: Delete everywhere…",
                    "Change visibility…",
                    "not in remote_actions",
                    "Archive",
                    "absent: Unarchive",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: All actions"], b"\x7f".to_vec()),
            (vec!["NORMAL · 2 rows"], b"/tool\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ tool"], b"m".to_vec()),
            (vec!["All actions"], b"\r".to_vec()),
            (
                vec![
                    "Repository actions / tool",
                    "Register",
                    "absent: On bitbucket.org",
                    "absent: Delete everywhere",
                ],
                vec![17],
            ),
        ],
        32,
        132,
    );
    assert!(fixture.actions().is_empty(), "{:?}", fixture.calls());
}

#[cfg(unix)]
#[test]
fn inventory_remote_states_come_from_the_catalog_and_lookups_and_missing_blocks_clone() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\n\n\
         [[repositories]]\nurl = \"github.com/org/listed\"\n\n\
         [[repositories]]\nurl = \"github.com/org/gone\"\n\n\
         [[repositories]]\nurl = \"github.com/org/old\"\n\n\
         [[repositories]]\nurl = \"github.com/other/broken\"\n\n\
         [[repositories]]\nurl = \"github.com/other/kept\"\n",
    );
    // The catalog lists only org/listed; other/kept exists beyond what the account lists.
    fs::write(
        fixture.state.join("catalog"),
        r#"[{"data":{"viewer":{"repositories":{"nodes":[{"name":"listed","isArchived":false,"isPrivate":false,"sshUrl":"git@github.com:org/listed.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]"#,
    )
    .unwrap();
    fs::write(fixture.state.join("free"), "org/gone").unwrap();
    // org/old is a former name GitHub redirects; other/broken fails the lookup otherwise.
    fs::write(fixture.state.join("redirects"), "org/old org/new\n").unwrap();
    fs::write(fixture.state.join("broken"), "other/broken").unwrap();
    let mut command = fixture.command();
    command.arg("--remote");
    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec!["NORMAL · 5 rows · 1 problem", "github.com: ready"],
                b"/gone\r".to_vec(),
            ),
            (
                vec![
                    "NORMAL · 1 row",
                    "gone",
                    "pair: org missing",
                    "absent: github.com/org/gone",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Inspect / gone",
                    "pair: Remote missing",
                    "absent: not found",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Inspect /"], b"m".to_vec()),
            (vec!["All actions"], b"\r".to_vec()),
            (
                vec![
                    "Repository actions / gone",
                    "pair: only not",
                    "pair: register not",
                    "not found on github.com",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["All actions", "absent: Repository actions /"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: All actions"], b"\x7f".to_vec()),
            (vec!["NORMAL · 5 rows"], b"/kept\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ kept"], b"\r".to_vec()),
            (
                vec!["Inspect / kept", "pair: Remote private", "absent: not on"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /"],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 5 rows"], b"/listed\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ listed"], b"\r".to_vec()),
            (
                vec!["Inspect / listed", "pair: Remote public"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /"],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 5 rows"], b"/org/old\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ org/old"], b"\r".to_vec()),
            (
                vec!["Inspect / old", "pair: Remote private", "absent: not on"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /"],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 5 rows"], b"/broken\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ broken"], b"\r".to_vec()),
            // A failed lookup leaves the state unknown, never missing.
            (
                vec!["Inspect / broken", "absent: Remote  ", "absent: not on"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Inspect /"], b"q".to_vec()),
        ],
        32,
        132,
        // The failed lookup is a session problem.
        1,
        |_| {},
    );
    // Only the repositories the catalog did not list are looked up.
    let mut views = fixture
        .calls()
        .into_iter()
        .filter(|call| call.contains("repo view"))
        .collect::<Vec<_>>();
    views.sort();
    assert_eq!(
        views,
        [
            "github.com repo view org/gone --json nameWithOwner,isArchived,visibility",
            "github.com repo view org/old --json nameWithOwner,isArchived,visibility",
            "github.com repo view other/broken --json nameWithOwner,isArchived,visibility",
            "github.com repo view other/kept --json nameWithOwner,isArchived,visibility",
        ]
    );
}

#[cfg(unix)]
#[test]
fn inventory_looks_up_declared_repositories_without_a_checkout_while_discovery_is_off() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\n\n\
         [[repositories]]\nurl = \"github.com/org/gone\"\n\n\
         [[repositories]]\nurl = \"github.com/org/kept\"\n\n\
         [[repositories]]\nurl = \"github.com/other/broken\"\n",
    );
    fs::write(fixture.state.join("free"), "org/gone").unwrap();
    fs::write(fixture.state.join("broken"), "other/broken").unwrap();
    run_inventory_pty_with_exit(
        &mut fixture.command(),
        &[
            (
                vec![
                    "NORMAL · 3 rows",
                    "gone",
                    "pair: org missing",
                    "broken",
                    "kept",
                ],
                b"/gone\r".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "/ gone"], b"\r".to_vec()),
            (
                vec![
                    "Inspect / gone",
                    "pair: Remote missing",
                    "absent: not found",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /"],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 3 rows"], b"/kept\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ kept"], b"\r".to_vec()),
            (
                vec!["Inspect / kept", "pair: Remote private", "absent: not on"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /"],
                b"\x7f".to_vec(),
            ),
            (vec!["NORMAL · 3 rows"], b"/broken\r".to_vec()),
            (vec!["NORMAL · 1 row", "/ broken"], b"\r".to_vec()),
            // A failed lookup leaves the state unknown, never missing.
            (
                vec!["Inspect / broken", "absent: Remote  ", "absent: not on"],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Inspect /", "absent: problem"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        // A failed lookup with discovery off is no session problem.
        0,
        |_| {},
    );
    let mut calls = fixture.calls();
    calls.sort();
    assert_eq!(
        calls,
        [
            "github.com repo view org/gone --json nameWithOwner,isArchived,visibility",
            "github.com repo view org/kept --json nameWithOwner,isArchived,visibility",
            "github.com repo view other/broken --json nameWithOwner,isArchived,visibility",
        ]
    );
}

#[cfg(unix)]
#[test]
fn inventory_creates_a_github_repository_and_clones_it_into_a_new_row() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"create\"]\n\n\
         [inventory.keys.normal]\ncreate_remote = [\"N\"]\n",
    );
    fs::write(fixture.state.join("missing"), "").unwrap();
    let remotes = fixture.temp.path().join("remotes");
    let remote = remotes.join("org/demo.git");
    fs::create_dir_all(&remote).unwrap();
    run_git(&remote, &["init", "--bare", "-q"]);
    let gitconfig = fixture.temp.path().join("gitconfig");
    fs::write(
        &gitconfig,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = git@github.com:\n",
            remotes.display()
        ),
    )
    .unwrap();
    let mut command = fixture.command();
    command.env("GIT_CONFIG_GLOBAL", &gitconfig);
    run_inventory_pty_actions(
        &mut command,
        &[
            (vec!["NORMAL", "absent: Scanning"], b"m".to_vec()),
            (
                vec!["All actions", "New repository on GitHub…"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: All actions"], b"N".to_vec()),
            (
                vec![
                    "New repository on GitHub",
                    "github.com/",
                    "[x] Private",
                    "[x] Register",
                    "absent: after cloning",
                    "absent: Lager creates",
                    "absent: Cancel",
                    "Tab focus · Space toggle · Enter choose · Esc back",
                    "Scan complete · 0 repositories · 0 declarations",
                ],
                b"org/demo\tA demo".to_vec(),
            ),
            (
                vec![
                    "github.com/org/demo",
                    "A demo█",
                    "pair: to ~/repos/org/demo",
                    "absent: Space toggles",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Batch / Clone + register",
                    "pair: SUCCEEDED github.com/org/demo",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Batch /", "explicit"], b"q".to_vec()),
        ],
        32,
        132,
    );
    assert_eq!(
        fixture.calls(),
        [
            "github.com auth status --hostname github.com",
            "github.com repo view org/demo --json nameWithOwner,isArchived,visibility",
            "github.com repo create org/demo --private --description=A demo",
        ]
    );
    assert!(fixture.home.join("repos/org/demo/.git").is_dir());
    assert!(fixture.config_text().contains("org/demo"));
}

#[cfg(unix)]
#[test]
fn inventory_renames_a_github_repository_and_moves_its_row() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"rename\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    fs::write(fixture.state.join("free"), "org/demo-cli").unwrap();
    let checkout = fixture.checkout("demo");
    run_inventory_pty_actions(
        &mut fixture.command(),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: demo"],
                b"m\rjjjjjj\r".to_vec(),
            ),
            (
                vec!["Rename on github.com", "absent: same owner"],
                b"-cli".to_vec(),
            ),
            (
                vec![
                    "org/demo → org/demo-cli",
                    "~/repos/org/demo → ~/repos/org/demo-cli",
                    "repointed to github.com/org/demo-cli",
                    "declaration rewritten",
                ],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "renamed to org/demo-cli, moved, redeclared",
                    "absent: Rename on",
                ],
                Vec::new(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: demo-cli"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
    );
    assert!(
        fixture
            .calls()
            .contains(&"github.com repo rename demo-cli --repo org/demo --yes".to_owned())
    );
    assert!(!checkout.exists());
    assert!(fixture.home.join("repos/org/demo-cli/.git").is_dir());
    assert!(
        fixture
            .config_text()
            .contains("\"github.com/org/demo-cli\"")
    );
}

#[cfg(unix)]
#[test]
fn inventory_delete_stays_disabled_until_the_typed_name_matches() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"delete\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/jqk\"\n",
    );
    fs::write(fixture.state.join("scopes"), "repo, delete_repo").unwrap();
    let checkout = fixture.checkout("jqk");
    let mut before_final = Vec::new();
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: jqk"],
                b"m\rjjjjjjjjj\r".to_vec(),
            ),
            (
                vec![
                    "Delete everywhere / org/jqk",
                    "deleted permanently, with issues, PRs and releases",
                    "nothing local would be lost",
                    "declaration removed",
                    "Type org/jqk to confirm",
                    "name does not match yet",
                    "absent: first",
                    "absent: default",
                    "Tab focus · Enter choose · Esc cancel",
                    "No active operation",
                ],
                b"org/jq".to_vec(),
            ),
            (
                vec![
                    "Delete everywhere / org/jqk",
                    "org/jq█",
                    "name does not match yet",
                ],
                b"k\r".to_vec(),
            ),
            (vec!["org/jqk█", "cannot be undone"], b"\r".to_vec()),
            (
                vec![
                    "org/jqk: deleted remote, removed, unregistered",
                    "absent: Delete everywhere /",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        |action| {
            if action == 3 {
                before_final = fixture.calls();
            }
        },
    );
    assert!(before_final.is_empty(), "{before_final:?}");
    assert!(
        fixture
            .calls()
            .contains(&"github.com repo delete org/jqk --yes".to_owned())
    );
    assert!(!checkout.exists());
    assert!(!fixture.config_text().contains("org/jqk"));
}

#[cfg(unix)]
#[test]
fn inventory_bound_archive_key_archives_the_current_row() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n\n\
         [inventory.keys.normal]\narchive = [\"A\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    run_inventory_pty_actions(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"A".to_vec()),
            (vec!["org/demo: archived"], b"m\r".to_vec()),
            (vec!["Repository actions / demo", "Unarchive"], vec![17]),
        ],
        32,
        132,
    );
    assert_eq!(
        fixture.actions(),
        [
            "github.com auth status --hostname github.com",
            "github.com repo archive org/demo --yes",
        ]
    );
    let calls = fixture.calls();
    let archive = calls.iter().position(|call| call.contains("repo archive"));
    assert!(
        calls[..archive.unwrap()]
            .iter()
            .any(|call| call.contains("repo view org/demo")),
        "{calls:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.state.join("archived")).unwrap(),
        "true"
    );
}

#[cfg(unix)]
#[test]
fn inventory_github_failure_names_the_fix_in_session_problems_and_exits_one() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n\n\
         [inventory.keys.normal]\narchive = [\"A\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    fs::write(fixture.state.join("logged-out"), "").unwrap();
    run_inventory_pty_with_exit(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"A".to_vec()),
            (vec!["org/demo: failed, see Problems"], b"m".to_vec()),
            (vec!["All actions", "1 problem"], b"jjjjjjjjjjj\r".to_vec()),
            (
                vec!["Problems / 1", "gh auth login --hostname github.com"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Problems /"], b"q".to_vec()),
        ],
        24,
        80,
        1,
        |_| {},
    );
    assert_eq!(
        fixture.actions(),
        ["github.com auth status --hostname github.com"]
    );
}

#[cfg(unix)]
#[test]
fn inventory_batches_wait_while_a_github_action_runs() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n\n\
         [inventory.keys.normal]\narchive = [\"A\"]\nunregister = [\"u\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    let hold = fixture.state.join("hold");
    fs::write(&hold, "").unwrap();
    run_inventory_pty_actions_with_hook(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"A".to_vec()),
            (vec!["Archiving org/demo on github.com…"], b"u".to_vec()),
            (
                vec!["A GitHub action is running", "absent: Batch /"],
                Vec::new(),
            ),
            (vec!["org/demo: archived"], b"q".to_vec()),
        ],
        32,
        132,
        |action| {
            if action == 2 {
                fs::remove_file(&hold).unwrap();
            }
        },
    );
    assert!(fixture.config_text().contains("org/demo"));
}

#[cfg(unix)]
#[test]
fn inventory_quit_waits_for_a_running_github_action() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n\n\
         [inventory.keys.normal]\narchive = [\"A\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    let hold = fixture.state.join("hold");
    fs::write(&hold, "").unwrap();
    let release = hold.clone();
    let transcript = run_inventory_pty_actions_with_hook(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"A".to_vec()),
            (vec!["Archiving org/demo on github.com…"], b"q".to_vec()),
        ],
        32,
        132,
        move |action| {
            if action == 1 {
                let release = release.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(500));
                    fs::remove_file(release).unwrap();
                });
            }
        },
    );
    assert!(
        transcript.contains("waiting for the GitHub action on github.com/org/demo to finish"),
        "{transcript}"
    );
    assert!(transcript.contains("lager: github.com/org/demo: archived"));
    assert_eq!(
        fs::read_to_string(fixture.state.join("archived")).unwrap(),
        "true"
    );
}

#[cfg(unix)]
#[test]
fn inventory_make_private_asks_first_with_its_consequences() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"visibility\"]\n\n\
         [inventory.keys.normal]\nvisibility = [\"V\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    fs::write(fixture.state.join("visibility"), "PUBLIC").unwrap();
    run_inventory_pty_actions(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"V".to_vec()),
            (
                vec!["Change visibility / demo", "Make private…", "absent: asks"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Make private / org/demo",
                    "its stars and watchers",
                    "Cancel",
                    "absent: default",
                    "j/k focus · Enter choose · Esc cancel",
                    "No active operation",
                ],
                b"j".to_vec(),
            ),
            (vec!["Make private / org/demo"], b"\r".to_vec()),
            (vec!["org/demo: made private"], b"m\r".to_vec()),
            (
                vec!["Repository actions / demo", "pair: public… private"],
                vec![17],
            ),
        ],
        32,
        132,
    );
    assert_eq!(
        fs::read_to_string(fixture.state.join("visibility")).unwrap(),
        "PRIVATE"
    );
}

#[cfg(unix)]
#[test]
fn inventory_a_refused_rename_reopens_its_form_with_the_reason() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"rename\"]\n\n\
         [inventory.keys.normal]\nrename = [\"N\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    run_inventory_pty_with_exit(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"N".to_vec()),
            (vec!["Rename on github.com", "demo█"], b"-cli\r".to_vec()),
            (
                vec![
                    "Rename on github.com",
                    "demo-cli█",
                    "already exists on github.com",
                    "org/demo: failed, the form shows why",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL", "absent: Rename on"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |_| {},
    );
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|call| call.contains("repo rename"))
    );
}

#[cfg(unix)]
#[test]
fn inventory_rename_form_holds_one_hint_row_over_an_empty_hint_bar_and_refuses_a_bad_name() {
    let fixture = GhFixture::new(
        "[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"rename\"]\n\n\
         [inventory.keys.normal]\nrename = [\"N\"]\n\n\
         [[repositories]]\nurl = \"github.com/org/demo\"\n",
    );
    let mut refused = String::new();
    run_inventory_pty_with_exit_observing(
        &mut fixture.command(),
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"N".to_vec()),
            (
                vec![
                    "Rename on github.com",
                    "demo█",
                    "pair: GitHub org/demo",
                    "Enter choose · Esc back",
                    "No active operation",
                    "absent: same owner",
                    "absent: redirects",
                    "absent: Cancel",
                ],
                b"/x\r".to_vec(),
            ),
            (
                vec![
                    "Rejected: use letters, digits, -, _ and . without a leading -",
                    "Enter choose · Esc back",
                    "absent: q quit",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL", "Rename cancelled · nothing changed"],
                b"q".to_vec(),
            ),
        ],
        24,
        80,
        0,
        |action, _, frame| {
            if action == 2 {
                refused = frame.to_owned();
            }
        },
    );
    let screen = screen_text(&refused, 24, 80);
    let lines = screen.split('\n').collect::<Vec<_>>();
    assert!(
        lines.iter().any(|line| line
            .split('│')
            .nth(1)
            .is_some_and(|body| body.trim() == "Enter choose · Esc back")),
        "{screen}"
    );
    assert_eq!(lines[23], "", "{screen}");
    assert!(
        styled_screen(&refused, 24, 80)[23]
            .iter()
            .all(|(_, reversed)| *reversed),
        "the empty hint bar stays a bar:\n{screen}"
    );
    assert!(fixture.calls().iter().all(|call| !call.contains("rename")));
}

/// A `lazygit` that logs its working directory, modifies a file of the checkout and fails.
#[cfg(unix)]
fn lazygit_stub(tools: &std::path::Path, log: &std::path::Path) {
    desktop_stubs(
        tools,
        &["lazygit"],
        &format!(
            "pwd -P >> {}\nprintf 'changed\\n' > README\nexit 3",
            shell_word(log)
        ),
    );
}

#[cfg(unix)]
#[test]
fn inventory_g_runs_lazygit_in_the_checkout_and_probes_the_row_again() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), "");
    let tools = temp.path().join("tools");
    let log = temp.path().join("lazygit.log");
    lazygit_stub(&tools, &log);

    let transcript = run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Scanning",
                    "selected: core",
                    "clean",
                    "pair: g lazygit",
                ],
                b"?".to_vec(),
            ),
            (vec!["lazygit in the current repository"], b"\x1b".to_vec()),
            (vec!["NORMAL · 1 row", "absent: Keyboard /"], b"g".to_vec()),
            (
                vec![
                    "NORMAL · 1 row",
                    "selected: core",
                    "1 modified",
                    "absent: clean",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        format!("{}\n", fs::canonicalize(&core).unwrap().display())
    );
    assert_eq!(
        fs::read_to_string(core.join("README")).unwrap(),
        "changed\n"
    );
    // The terminal left the alternate screen for lazygit and came back to it.
    assert!(
        transcript.matches("\x1b[?1049l").count() >= 2,
        "{transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn inventory_g_on_a_row_without_a_checkout_names_the_reason_and_spawns_nothing() {
    let temp = temp_dir();
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(home.join("repos")).unwrap();
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/alpha\"\n",
    )
    .unwrap();
    let tools = temp.path().join("tools");
    let log = temp.path().join("lazygit.log");
    lazygit_stub(&tools, &log);

    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: alpha"],
                b"g".to_vec(),
            ),
            (
                vec!["Open lazygit unavailable: no checkout at the destination"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert!(
        !log.exists(),
        "lazygit was started for a row without a checkout"
    );
}

#[cfg(unix)]
#[test]
fn inventory_g_without_lazygit_on_the_path_says_so_and_keeps_running() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), "");

    run_inventory_pty_with_exit(
        support::lager(&home, &config).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: core"],
                b"g".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "lazygit not found on PATH"],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_lazygit_default_yields_to_a_configured_g() {
    let temp = temp_dir();
    let (home, config, _) =
        worktree_fixture(temp.path(), "\n[inventory.keys.normal]\nsearch = [\"g\"]\n");
    let tools = temp.path().join("tools");
    let log = temp.path().join("lazygit.log");
    lazygit_stub(&tools, &log);

    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: core"],
                b"g".to_vec(),
            ),
            (vec!["SEARCH ·"], b"\x1b".to_vec()),
            (vec!["NORMAL · 1 row"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert!(
        !log.exists(),
        "lazygit took a key the configuration gave to search"
    );
}

#[cfg(unix)]
#[test]
fn inventory_lazygit_follows_a_rebound_key() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        "\n[inventory.keys.normal]\nlazygit = [\"L\"]\n",
    );
    let tools = temp.path().join("tools");
    let log = temp.path().join("lazygit.log");
    lazygit_stub(&tools, &log);

    run_inventory_pty_with_exit(
        support::lager_with_path(&home, &config, Some(&tools)).arg("tui"),
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "selected: core"],
                b"g".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "selected: core", "absent: 1 modified"],
                b"L".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "1 modified"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[cfg(unix)]
const SESSION_SECTION: &str = "\n[session]\nhost = \"rex\"\n\n[session.layouts.default.tabs.Shells]\nroot = { pane = \"Shell I\" }\n";

/// `lager tui` with the fake `rex` first on `PATH` and its state under `<temp>/rex/state`. No
/// call can reach a real Rex: the environment is cleared, `HOME` is the sandbox, and
/// `REX_SERVER` names a socket that does not exist.
#[cfg(unix)]
fn session_tui(
    temp: &std::path::Path,
    home: &std::path::Path,
    config: &std::path::Path,
) -> (Command, std::path::PathBuf) {
    let (mut command, state) = session_lager(temp, home, config);
    command.arg("tui");
    (command, state)
}

/// `lager` in the sandbox `session_tui` describes, for any command.
#[cfg(unix)]
fn session_lager(
    temp: &std::path::Path,
    home: &std::path::Path,
    config: &std::path::Path,
) -> (Command, std::path::PathBuf) {
    let (bin, state) = support::install_fake_rex(&temp.join("rex"));
    let mut command = support::lager_with_path(home, config, Some(&bin));
    command
        .env("REX_ARGS_LOG", state.join("args.log"))
        .env("REX_STATE", &state)
        .env(
            "REX_SERVER",
            format!("unix://{}", temp.join("none.sock").display()),
        )
        .env("REX_RUNTIME_DIR", temp.join("rex-runtime"));
    (command, state)
}

/// The fake's calls that contain `part`.
#[cfg(unix)]
fn rex_calls(state: &std::path::Path, part: &str) -> usize {
    fs::read_to_string(state.join("args.log"))
        .unwrap_or_default()
        .lines()
        .filter(|call| call.contains(part))
        .count()
}

/// What the terminal received after the inventory left the alternate screen.
#[cfg(unix)]
fn after_restore(transcript: &str) -> &str {
    transcript
        .rsplit_once("\x1b[?1049l")
        .map(|(_, after)| after)
        .unwrap_or_default()
}

#[cfg(unix)]
#[test]
fn inventory_s_opens_the_session_closes_and_names_it() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Scanning",
                    "pair: s session",
                    "absent: ●",
                ],
                b"?".to_vec(),
            ),
            (
                vec![
                    "pair: s Session",
                    "Sessions",
                    "pair: ● open",
                    "opens the session or shows the open one, then the inventory closes",
                    "pair: s plan:",
                    "plan: open the session after · removal: close it after",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Keyboard /"], b"m".to_vec()),
            (vec!["All actions", "pair: session s"], b"\x1b".to_vec()),
            (vec!["NORMAL · 1 row", "absent: All actions"], b"s".to_vec()),
        ],
        33,
        132,
        0,
        |_| {},
    );
    assert!(
        after_restore(&transcript)
            .trim_end()
            .ends_with("Opened session core"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 1);

    // The next run marks the row and switches to the same session.
    let (mut command, _) = session_tui(temp.path(), &home, &config);
    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[(
            vec!["NORMAL · 1 row", "absent: Scanning", "pair: ● core"],
            b"s".to_vec(),
        )],
        32,
        132,
        0,
        |_| {},
    );
    assert!(
        after_restore(&transcript)
            .trim_end()
            .ends_with("Switched to session core"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 2);
}

#[cfg(unix)]
#[test]
fn inventory_s_stays_open_with_the_reason_or_the_failure() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        &format!("\n[[repositories]]\nurl = \"github.com/org/ghost\"\n{SESSION_SECTION}"),
    );
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("create-fails"), "").unwrap();

    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "pair: s session"],
                b"/ghost\r".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "selected: ghost"], b"s".to_vec()),
            (
                vec!["Open session unavailable: no checkout at the destination"],
                b"m".to_vec(),
            ),
            (
                vec![
                    "All actions",
                    "pair: session no",
                    "checkout at the destination",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row", "absent: All actions"],
                b"\x7f/core\r".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "selected: core"], b"s".to_vec()),
            (
                vec![
                    "! Open session failed: stdin:4: session.create",
                    "NORMAL · 1 row · 1 problem",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        1,
        |_| {},
    );
    assert!(
        !after_restore(&transcript).contains("session core"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 0);
}

#[cfg(unix)]
#[test]
fn inventory_says_once_that_sessions_are_unavailable_and_s_repeats_it() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("offline"), "").unwrap();

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec![
                    "NORMAL · 1 row",
                    "absent: Scanning",
                    "Sessions unavailable: no Rex server is running",
                    "absent: ! Sessions",
                    "absent: No active operation",
                    "absent: s session",
                    "absent: ●",
                ],
                b"s".to_vec(),
            ),
            (
                vec!["Open session unavailable: no Rex server is running"],
                b"R".to_vec(),
            ),
            // Sent once the list after the refresh has been answered.
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"j".to_vec()),
            (
                vec![
                    "NORMAL · 1 row",
                    "No active operation",
                    "absent: Sessions unavailable",
                ],
                b"s".to_vec(),
            ),
            (
                vec![
                    "Open session unavailable: no Rex server is running",
                    "absent: Sessions unavailable",
                    "absent: problem",
                ],
                b"q".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |action| {
            if action == 2 {
                wait_for_answers(&state, "session.list", 2);
            }
        },
    );
    assert_eq!(rex_calls(&state, "do - --json"), 0);
}

/// Waits until the offline fake answered `count` calls containing `part`, and lager had time
/// to read them.
#[cfg(unix)]
fn wait_for_answers(state: &std::path::Path, part: &str, count: usize) {
    let answered = || {
        fs::read_to_string(state.join("answered"))
            .unwrap_or_default()
            .lines()
            .filter(|call| call.contains(part))
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while answered() < count {
        assert!(Instant::now() < deadline, "no {count} calls of {part}");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
}

#[cfg(unix)]
#[test]
fn inventory_records_a_failed_session_list_as_a_problem() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("bad-list"), "").unwrap();

    run_inventory_pty_with_exit(
        &mut command,
        &[(
            vec![
                "NORMAL · 1 row · 1 problem",
                "absent: Scanning",
                "absent: Sessions unavailable",
                "absent: s session",
            ],
            b"q".to_vec(),
        )],
        32,
        132,
        1,
        |_| {},
    );
}

#[cfg(unix)]
#[test]
fn inventory_answers_keys_while_a_session_opens() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    tracked_repo(&home.join("repos/org/dots"), "git@github.com:org/dots.git");
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("hold-create"), "").unwrap();

    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec!["NORMAL · 2 rows", "absent: Scanning", "pair: s session"],
                b"/core\r\x7f".to_vec(),
            ),
            (vec!["NORMAL · 2 rows", "selected: core"], b"s".to_vec()),
            (vec!["Opening session core…"], b"j".to_vec()),
            (
                vec!["selected: dots", "Opening session core…"],
                b"s".to_vec(),
            ),
            (
                vec!["Open session unavailable: a session is opening"],
                Vec::new(),
            ),
        ],
        32,
        132,
        0,
        |action| {
            if action == 4 {
                fs::write(state.join("release"), "").unwrap();
            }
        },
    );
    assert!(
        after_restore(&transcript)
            .trim_end()
            .ends_with("Opened session core"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
}

#[cfg(unix)]
#[test]
fn inventory_explicit_s_wins_over_the_session_key() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(
        temp.path(),
        &format!("{SESSION_SECTION}\n[inventory.keys.normal]\nsearch = [\"s\"]\n"),
    );
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "pair: s search"],
                b"s".to_vec(),
            ),
            (vec!["SEARCH · 1 row"], b"\x1b".to_vec()),
            (
                vec!["NORMAL · 1 row", "absent: Enter accept"],
                b"m".to_vec(),
            ),
            (
                vec!["All actions", "Open session", "absent: session s"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: All actions"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert_eq!(rex_calls(&state, "do - --json"), 0);
}

#[cfg(unix)]
#[test]
fn inventory_without_a_session_section_shows_and_asks_nothing_about_sessions() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), "");
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    let mut frames = Vec::new();

    run_inventory_pty_with_exit_observing(
        &mut command,
        &[
            (vec!["NORMAL · 1 row", "absent: Scanning"], b"s".to_vec()),
            // Gives `s` time to act before the next frame is compared.
            (vec!["NORMAL · 1 row"], Vec::new()),
            (vec!["NORMAL · 1 row"], b"?".to_vec()),
            (
                vec![
                    "Keyboard / effective bindings",
                    "absent: Session of the current repository",
                    "absent: Sessions",
                    "absent: mode: session",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Keyboard /"], b"m".to_vec()),
            (
                vec!["All actions", "absent: Open session"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: All actions"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action, _, frame| {
            if action == 1 {
                std::thread::sleep(Duration::from_millis(300));
            }
            if matches!(action, 0 | 2) {
                frames.push(screen_text(frame, 32, 132));
            }
        },
    );
    let [before, after] = frames.as_slice() else {
        panic!("expected two frames: {frames:?}");
    };
    assert_eq!(before, after);
    assert!(!before.contains("session"), "{before}");
    assert!(!state.join("args.log").exists());
}

#[cfg(unix)]
#[test]
fn inventory_quit_waits_for_an_open_in_flight_and_records_it() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("hold-create"), "").unwrap();

    let transcript = run_inventory_pty_with_exit_observing(
        &mut command,
        &[
            (
                vec!["NORMAL · 1 row", "absent: Scanning", "pair: s session"],
                b"s".to_vec(),
            ),
            (vec!["Opening session core…"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action, terminal, _| {
            if action == 1 {
                // Rex answers once the inventory gave the terminal back.
                let (terminal, release) = (terminal.try_clone().unwrap(), state.join("release"));
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while Instant::now() < deadline
                        && !rustix_openpty::rustix::termios::tcgetattr(&terminal)
                            .unwrap()
                            .local_modes
                            .contains(rustix_openpty::rustix::termios::LocalModes::ICANON)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    fs::write(release, "").unwrap();
                });
            }
        },
    );
    let after = after_restore(&transcript);
    assert!(
        after.contains("lager: waiting for session core to open…"),
        "{transcript:?}"
    );
    assert!(
        after.trim_end().ends_with("Opened session core"),
        "{transcript:?}"
    );
    let records = fs::read_to_string(temp.path().join("lager-test-cache/sessions.json")).unwrap();
    assert!(records.contains("org/core"), "{records}");
}

/// Opens the session of each worktree in `paths` through `lager session create`, as the
/// inventory finds them later.
#[cfg(unix)]
fn create_sessions(
    temp: &std::path::Path,
    home: &std::path::Path,
    config: &std::path::Path,
    paths: &[&std::path::Path],
) {
    for path in paths {
        let (mut command, _) = session_lager(temp, home, config);
        let output = command
            .args(["session", "create"])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_opens_the_session_of_a_worktree_it_creates() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"w".to_vec(),
            ),
            (
                vec![
                    "Worktrees / core",
                    "j/k select · s session · a add · r remove · c copy · Esc close",
                ],
                b"a".to_vec(),
            ),
            (vec!["Add worktree / core"], b"skip\r".to_vec()),
            (
                vec![
                    "Add worktree / plan",
                    "pair: session core",
                    "core · Skip",
                    "[x] Open session",
                    "j/k select · s session · Enter choose · Esc back",
                ],
                b"s".to_vec(),
            ),
            (vec!["[ ] Open session", "absent: [x]"], b"\r".to_vec()),
            (
                vec![
                    "Created worktree ~/repos/org/core.worktrees/skip on new branch skip",
                    "WORKTREES · 1 row",
                ],
                b"a".to_vec(),
            ),
            (vec!["Add worktree / core"], b"fix\r".to_vec()),
            (
                vec!["Add worktree / plan", "core · Fix", "[x] Open session"],
                b"\r".to_vec(),
            ),
            (
                vec!["Opening session core · Fix…", "WORKTREES · 1 row"],
                Vec::new(),
            ),
        ],
        32,
        132,
        0,
        |action| {
            if action == 5 {
                assert_eq!(rex_calls(&state, "do - --json"), 0);
                assert_eq!(rex_calls(&state, "client.perform"), 0);
                fs::write(state.join("hold-create"), "").unwrap();
            }
            if action == 8 {
                fs::write(state.join("release"), "").unwrap();
            }
        },
    );
    fs::remove_file(state.join("hold-create")).unwrap();
    let after = after_restore(&transcript);
    let created = after
        .find("Created worktree ~/repos/org/core.worktrees/fix on new branch fix")
        .unwrap_or_else(|| panic!("{transcript:?}"));
    assert!(
        after.trim_end().ends_with("Opened session core · Fix")
            && created < after.find("Opened session").unwrap(),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 1);

    // The next run marks the new worktree, and `s` there switches to its session.
    let (mut command, _) = session_tui(temp.path(), &home, &config);
    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"w".to_vec(),
            ),
            (
                vec![
                    "pair: ● ~/repos/org/core.worktrees/fix",
                    "absent: ● ~/repos/org/core.worktrees/skip",
                ],
                b"?".to_vec(),
            ),
            (
                vec![
                    "Worktrees mode",
                    "open the session of the selected worktree",
                    "s session · Enter choose",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Keyboard /"],
                b"js".to_vec(),
            ),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert!(
        after_restore(&transcript)
            .trim_end()
            .ends_with("Switched to session core · Fix"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 2);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_closes_a_session_only_when_ticked_and_after_the_removal() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let draft = linked_worktree(&core, "draft", &["-b", "draft"]);
    let spare = linked_worktree(&core, "spare", &["-b", "spare"]);
    create_sessions(temp.path(), &home, &config, &[&draft, &spare]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"w".to_vec(),
            ),
            (
                vec![
                    "pair: ● ~/repos/org/core.worktrees/draft",
                    "pair: ● ~/repos/org/core.worktrees/spare",
                ],
                b"jr".to_vec(),
            ),
            (
                vec![
                    "Remove worktree / draft",
                    "core · Draft · open",
                    "[x] Close session",
                    "ends its programs",
                    "j/k focus · s session · Enter choose · Esc cancel",
                ],
                b"s".to_vec(),
            ),
            (vec!["[ ] Close session", "absent: [x]"], b"j\r".to_vec()),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept",
                    "WORKTREES · 1 row",
                    "absent: Closed session",
                ],
                b"r".to_vec(),
            ),
            (
                vec![
                    "Remove worktree / spare",
                    "core · Spare · open",
                    "[x] Close session",
                ],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/spare · branch spare kept · Closed session core · Spare",
                    "WORKTREES · 1 row",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action| {
            if action == 4 {
                assert!(!draft.exists());
                assert_eq!(rex_calls(&state, "session.destroy"), 0);
            }
        },
    );
    assert!(!spare.exists());
    assert_eq!(rex_calls(&state, "session.destroy"), 1);
    assert_eq!(rex_calls(&state, "client.perform"), 0);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_sees_a_session_created_after_the_inventory_started() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let draft = linked_worktree(&core, "draft", &["-b", "draft"]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            // `s session` shows once the first lookup has been answered.
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"w".to_vec(),
            ),
            (
                vec!["pair: ● ~/repos/org/core.worktrees/draft"],
                b"jr".to_vec(),
            ),
            (
                vec!["Remove worktree / draft", "[x] Close session"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept · Closed session core · Draft",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action| {
            if action == 0 {
                create_sessions(temp.path(), &home, &config, &[&draft]);
            }
        },
    );
    assert!(!draft.exists());
    assert_eq!(rex_calls(&state, "session.destroy"), 1);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_keeps_the_session_of_a_blocked_worktree_open() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let locked = linked_worktree(&core, "locked", &["-b", "locked"]);
    linked_worktree(&core, "plain", &["-b", "plain"]);
    create_sessions(temp.path(), &home, &config, &[&locked]);
    run_git(&core, &["worktree", "lock", locked.to_str().unwrap()]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"w".to_vec(),
            ),
            (
                vec!["pair: ● ~/repos/org/core.worktrees/locked"],
                b"jr".to_vec(),
            ),
            (
                vec![
                    "Remove worktree / blocked",
                    "pair: session core",
                    "core · Locked · open",
                    "pair: blocked worktree",
                    "absent: Close session",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Worktrees / core", "absent: Remove worktree /"],
                b"jr".to_vec(),
            ),
            // A worktree without a session offers no session line.
            (
                vec![
                    "Remove worktree / plain",
                    "absent: Close session",
                    "absent: · open",
                    "j/k focus · Enter choose · Esc cancel",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["Remove cancelled · nothing changed", "Worktrees / core"],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert!(locked.exists());
    assert_eq!(rex_calls(&state, "session.destroy"), 0);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_keeps_the_session_when_the_removal_is_refused_after_the_summary() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let hotfix = linked_worktree(&core, "hotfix", &["-b", "hotfix"]);
    create_sessions(temp.path(), &home, &config, &[&hotfix]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"wjr".to_vec(),
            ),
            (
                vec!["Remove worktree / hotfix", "[x] Close session"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "the worktree changed after it was inspected; nothing removed · the session stays open",
                    "pair: ● ~/repos/org/core.worktrees/hotfix",
                    "pair: hotfix locked",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action| {
            if action == 1 {
                run_git(&core, &["worktree", "lock", hotfix.to_str().unwrap()]);
            }
        },
    );
    assert!(hotfix.is_dir());
    assert_eq!(rex_calls(&state, "session.destroy"), 0);
}

/// The menu keys that open Problems… from a row with a checkout and sessions configured.
#[cfg(unix)]
const OPEN_PROBLEMS: &[u8] = b"mjjjjjjjjjjjj\r";

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_keeps_the_removal_when_the_close_fails() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let draft = linked_worktree(&core, "draft", &["-b", "draft"]);
    create_sessions(temp.path(), &home, &config, &[&draft]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("destroy-fails"), "").unwrap();

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"wjr".to_vec(),
            ),
            (
                vec!["Remove worktree / draft", "[x] Close session"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "! Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept · Close session failed:",
                    "WORKTREES · 1 row · 1 problem",
                ],
                b"\x1b".to_vec(),
            ),
            (
                vec!["NORMAL · 1 row · 1 problem", "absent: Worktrees /"],
                OPEN_PROBLEMS.to_vec(),
            ),
            (
                vec![
                    "Problems / 1",
                    "pair: session Close",
                    "Close session failed:",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Problems /"], b"q".to_vec()),
        ],
        32,
        132,
        1,
        |_| {},
    );
    assert!(!draft.exists());
    assert_eq!(rex_calls(&state, "session.destroy"), 1);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_removes_the_worktree_while_rex_is_unavailable_at_the_close() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(temp.path(), SESSION_SECTION);
    let draft = linked_worktree(&core, "draft", &["-b", "draft"]);
    create_sessions(temp.path(), &home, &config, &[&draft]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"wjr".to_vec(),
            ),
            (
                vec!["Remove worktree / draft", "[x] Close session"],
                b"j\r".to_vec(),
            ),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept · Close session unavailable: no Rex server is running",
                    "absent: problem",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |action| {
            if action == 1 {
                fs::write(state.join("offline"), "").unwrap();
            }
        },
    );
    assert!(!draft.exists());
    assert_eq!(rex_calls(&state, "session.destroy"), 0);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_explicit_s_in_the_confirmation_wins_over_the_session_key() {
    let temp = temp_dir();
    let (home, config, core) = worktree_fixture(
        temp.path(),
        &format!("{SESSION_SECTION}\n[inventory.keys.confirmation]\ndown = [\"s\"]\n"),
    );
    let draft = linked_worktree(&core, "draft", &["-b", "draft"]);
    create_sessions(temp.path(), &home, &config, &[&draft]);
    let (mut command, state) = session_tui(temp.path(), &home, &config);

    run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"wjr".to_vec(),
            ),
            (
                vec![
                    "Remove worktree / draft",
                    "[x] Close session",
                    "absent: s session",
                ],
                // `s` moves to the removing choice; the tick stays.
                b"s\r".to_vec(),
            ),
            (
                vec![
                    "Removed worktree ~/repos/org/core.worktrees/draft · branch draft kept · Closed session core · Draft",
                ],
                b"\x1b".to_vec(),
            ),
            (vec!["NORMAL · 1 row", "absent: Worktrees /"], b"q".to_vec()),
        ],
        32,
        132,
        0,
        |_| {},
    );
    assert_eq!(rex_calls(&state, "session.destroy"), 1);
}

#[cfg(unix)]
#[test]
fn inventory_worktree_popup_add_starts_no_second_open() {
    let temp = temp_dir();
    let (home, config, _) = worktree_fixture(temp.path(), SESSION_SECTION);
    let (mut command, state) = session_tui(temp.path(), &home, &config);
    fs::write(state.join("hold-create"), "").unwrap();

    let transcript = run_inventory_pty_with_exit(
        &mut command,
        &[
            (
                [&POPUP_READY[..], &["pair: s session"]].concat(),
                b"ws".to_vec(),
            ),
            (vec!["Opening session core…"], b"a".to_vec()),
            (vec!["Add worktree / core"], b"fix\r".to_vec()),
            (
                vec!["Add worktree / plan", "[x] Open session"],
                b"\r".to_vec(),
            ),
            (
                vec![
                    "Created worktree ~/repos/org/core.worktrees/fix on new branch fix · Open session unavailable: a session is opening",
                ],
                Vec::new(),
            ),
        ],
        32,
        132,
        0,
        |action| {
            if action == 4 {
                fs::write(state.join("release"), "").unwrap();
            }
        },
    );
    assert!(
        after_restore(&transcript)
            .trim_end()
            .ends_with("Opened session core"),
        "{transcript:?}"
    );
    assert_eq!(rex_calls(&state, "do - --json"), 1);
}
