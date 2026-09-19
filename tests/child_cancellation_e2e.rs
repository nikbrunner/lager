mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(&home).unwrap();
        fs::write(&config, "root = \"repos\"\n").unwrap();
        Self { temp, home, config }
    }

    fn remote(&self, name: &str) -> String {
        let remote = self.temp.path().join(format!("{name}.git"));
        let output = support::external(self.temp.path(), "git")
            .args(["init", "--bare"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        format!("file://{}", remote.display())
    }

    fn run(&self, args: &[&str]) -> Output {
        support::lager(&self.home, &self.config)
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn hook_exit130_preserves_clone_and_registration_and_stops_add_batch() {
    let fixture = Fixture::new();
    let first = fixture.remote("first");
    let later = fixture.remote("later");
    let output = fixture.run(&[
        "add",
        &first,
        &later,
        "--register",
        "--post-clone",
        "printf native-out; printf native-err >&2; exit 130",
    ]);
    assert_eq!(output.status.code(), Some(130), "{output:?}");
    assert!(fixture.home.join("repos/first/.git").exists());
    assert!(!fixture.home.join("repos/later").exists());
    let config = fs::read_to_string(&fixture.config).unwrap();
    assert!(config.contains(&first), "{config}");
    assert!(!config.contains(&later), "{config}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("native-out"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("native-err"));
}

#[test]
fn ensure_and_hook_stop_on_self_sigint_but_continue_ordinary_failure() {
    for command in ["kill -INT $$", "exit 130", "printf cancelled >&2; exit 1"] {
        for operation in ["ensure", "hook"] {
            let fixture = Fixture::new();
            let first = fixture.remote("first");
            let middle = fixture.remote("middle");
            let later = fixture.remote("later");
            if operation == "hook" {
                let output = fixture.run(&["add", &first, &middle, &later, "--no-register"]);
                assert!(output.status.success(), "{output:?}");
            }
            fs::write(
                &fixture.config,
                format!(
                    "root = \"repos\"\n\
                 [[repositories]]\nurl = {first:?}\npost_clone = \"printf first > $HOME/first\"\n\
                 [[repositories]]\nurl = {middle:?}\npost_clone = {command:?}\n\
                 [[repositories]]\nurl = {later:?}\npost_clone = \"printf later > $HOME/later\"\n"
                ),
            )
            .unwrap();
            let args = if operation == "ensure" {
                vec!["ensure"]
            } else {
                vec!["hook", &first, &middle, &later]
            };
            let output = fixture.run(&args);
            let cancelled = command != "printf cancelled >&2; exit 1";
            assert_eq!(
                output.status.code(),
                Some(if cancelled { 130 } else { 1 }),
                "{operation}: {command}: {output:?}"
            );
            assert!(fixture.home.join("first").exists());
            assert_eq!(fixture.home.join("later").exists(), !cancelled);
            if operation == "ensure" {
                assert!(fixture.home.join("repos/middle/.git").exists());
                assert_eq!(fixture.home.join("repos/later").exists(), !cancelled);
            }
        }
    }
}

#[test]
fn clone_interruption_never_registers_or_hooks_and_preserves_earlier_success() {
    for failure in ["kill -INT $$", "exit 130", "printf cancelled >&2; exit 1"] {
        for cleanup_failure in [false, true] {
            let fixture = Fixture::new();
            let first = fixture.remote("first");
            let middle = fixture.remote("middle");
            let later = fixture.remote("later");
            let bin = fixture.temp.path().join("bin");
            fs::create_dir(&bin).unwrap();
            let cleanup = if cleanup_failure {
                "/bin/mv \"$PWD\" \"$PWD.moved\""
            } else {
                ":"
            };
            fs::write(
                bin.join("git"),
                format!(
                    "#!/bin/sh\nif [ \"$1\" = clone ] && [ \"$2\" = '{middle}' ]; then\n\
                 printf clone-native-out; printf clone-native-err >&2\n{cleanup}\n{failure}\nfi\n\
                 exec '{}' \"$@\"\n",
                    support::real_tool("git").display()
                ),
            )
            .unwrap();
            fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
            let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&bin))
                .args([
                    "add",
                    &first,
                    &middle,
                    &later,
                    "--register",
                    "--post-clone",
                    "printf hook > ran-hook",
                ])
                .output()
                .unwrap();
            let cancelled = failure != "printf cancelled >&2; exit 1";
            assert_eq!(
                output.status.code(),
                Some(if cancelled { 130 } else { 1 }),
                "{failure}, cleanup_failure={cleanup_failure}: {output:?}"
            );
            assert!(fixture.home.join("repos/first/.git").exists());
            assert!(fixture.home.join("repos/first/ran-hook").exists());
            assert!(!fixture.home.join("repos/middle").exists());
            assert!(!fixture.home.join("repos/middle.moved/ran-hook").exists());
            assert_eq!(fixture.home.join("repos/later").exists(), !cancelled);
            let config = fs::read_to_string(&fixture.config).unwrap();
            assert!(config.contains(&first), "{config}");
            assert!(!config.contains(&middle), "{config}");
            assert_eq!(config.contains(&later), !cancelled, "{config}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("clone-native-out"));
            assert!(String::from_utf8_lossy(&output.stderr).contains("clone-native-err"));
            if cleanup_failure {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("could not be safely cleaned")
                );
            }
        }
    }
}
