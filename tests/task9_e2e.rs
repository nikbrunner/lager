//! Task 9 binary-boundary coverage for automation contracts not covered by earlier slices.

mod support;

#[cfg(unix)]
mod unix {
    use super::support;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::process::Output;

    fn isolated_path(temp: &Path) -> std::io::Result<std::path::PathBuf> {
        let bin = temp.join("bin");
        fs::create_dir_all(&bin)?;
        symlink(support::real_tool("git"), bin.join("git"))?;
        Ok(bin)
    }

    fn run(
        config: &Path,
        home: &Path,
        cache: &Path,
        bin: &Path,
        args: &[&str],
    ) -> std::io::Result<Output> {
        let _ = cache;
        support::lager_with_path(home, config, Some(bin))
            .args(args)
            .output()
    }

    #[test]
    fn compiled_help_describes_public_commands_and_examples()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(&home)?;
        let top = support::lager(&home, &config).args(["--help"]).output()?;
        assert!(top.status.success());
        let top_help = String::from_utf8_lossy(&top.stdout);
        for description in [
            "Create a new portable configuration file",
            "Declare repositories for management",
            "Remove repository declarations without deleting local data",
            "Create local checkouts and optionally declare them",
            "Permanently delete guarded local checkouts",
            "Create missing declared checkouts sequentially",
            "Run configured hooks for explicit repositories",
            "Show declarations and local repository state",
            "[alias: ls]",
            "Use PATH instead of the default configuration file",
            "lager add github.com/org/project --register",
            "lager remove github.com/org/project --unregister --yes --force",
        ] {
            assert!(
                top_help.contains(description),
                "missing top-level help: {description}"
            );
        }

        let expected: &[(&str, &[&str])] = &[
            (
                "init",
                &[
                    "Portable repository root under HOME",
                    "Create ROOT when it does not exist",
                    "Do not configure GitHub discovery",
                ],
            ),
            (
                "register",
                &[
                    "Repository references; omit to choose interactively",
                    "Store a hook command with each declaration",
                    "Include archived provider repositories",
                ],
            ),
            (
                "unregister",
                &[
                    "Repository references; omit to choose interactively",
                    "Include archived provider repositories",
                ],
            ),
            (
                "add",
                &[
                    "Repository references; omit to choose interactively",
                    "Declare each successfully added repository",
                    "Do not declare added repositories",
                    "Run CMD after a fresh add; implies --register",
                    "Include archived provider repositories",
                    "lager add github.com/org/project --no-register",
                ],
            ),
            (
                "remove",
                &[
                    "Repository references; omit to choose local checkouts",
                    "Remove declarations after successful disk removal",
                    "Keep declarations after disk removal",
                    "Confirm removal without an interactive prompt",
                    "Bypass local-state warnings; safety checks still apply",
                    "--force bypasses local-state warnings only",
                ],
            ),
            ("ensure", &["Include archived provider repositories"]),
            ("hook", &["Explicit repository references"]),
            (
                "list",
                &[
                    "Query providers for expanded wildcard members",
                    "Include archived repositories; requires --remote",
                    "Render one stable JSON document",
                ],
            ),
        ];
        for (command, descriptions) in expected {
            let output = support::lager(&home, &config)
                .args([command, "--help"])
                .output()?;
            assert!(output.status.success(), "help failed for {command}");
            let help = String::from_utf8_lossy(&output.stdout);
            for description in *descriptions {
                assert!(
                    help.contains(description),
                    "missing {command} help: {description}"
                );
            }
        }
        let list = support::lager(&home, &config)
            .args(["list", "--help"])
            .output()?;
        let alias = support::lager(&home, &config)
            .args(["ls", "--help"])
            .output()?;
        assert_eq!(alias.status.code(), list.status.code());
        assert_eq!(alias.stderr, list.stderr);
        assert_eq!(
            String::from_utf8_lossy(&alias.stdout).replace("lager ls", "lager list"),
            String::from_utf8_lossy(&list.stdout)
        );
        Ok(())
    }

    #[test]
    fn json_keeps_unknown_config_warnings_on_stderr() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let cache = temp.path().join("cache");
        let config = temp.path().join("config.toml");
        let bin = isolated_path(temp.path())?;
        fs::create_dir_all(&home)?;
        fs::write(
            &config,
            "root = \"repos\"\nfuture_key = \"preserve\"\n\n[[repositories]]\nurl = \"github.com/org/repo\"\n",
        )?;

        let output = run(&config, &home, &cache, &bin, &["list", "--json"])?;
        assert!(output.status.success());
        let document: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(document["schema_version"], 1);
        assert!(document["repositories"].is_array());
        assert!(String::from_utf8_lossy(&output.stderr).contains("future_key"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("warning"));
        assert!(fs::read_to_string(config)?.contains("future_key = \"preserve\""));
        Ok(())
    }

    #[test]
    fn non_tty_add_rejects_missing_registration_before_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let cache = temp.path().join("cache");
        let config = temp.path().join("config.toml");
        let bin = isolated_path(temp.path())?;
        fs::create_dir_all(&home)?;
        fs::write(&config, "root = \"repos\"\n")?;
        let before = fs::read(&config)?;

        let output = run(
            &config,
            &home,
            &cache,
            &bin,
            &["add", "file:///does-not-exist.git"],
        )?;
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("--register"));
        assert_eq!(fs::read(&config)?, before);
        assert!(!home.join("repos").exists());
        Ok(())
    }
}
