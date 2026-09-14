mod support;

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;

use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct PtyOutput {
    status: ExitStatus,
    terminal: Vec<u8>,
}

struct Fixture {
    temp: TempDir,
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let config = temp.path().join("config.toml");
        fs::create_dir_all(&home)?;
        fs::write(&config, "root = \"repos\"\n")?;
        Ok(Self { temp, home, config })
    }

    fn run(&self, args: &[&str]) -> Output {
        support::lager(&self.home, &self.config)
            .args(args)
            .output()
            .expect("lager process")
    }

    fn run_in_pty(&self, args: &[&str]) -> io::Result<PtyOutput> {
        let mut command = support::lager(&self.home, &self.config);
        command.env("TERM", "xterm-256color").args(args);
        run_with_pty(command)
    }

    fn reference(&self, name: &str) -> String {
        format!(
            "file://{}",
            self.temp.path().join(format!("{name}.git")).display()
        )
    }

    fn destination(&self, name: &str) -> PathBuf {
        self.home.join("repos").join(name)
    }
}

fn run_with_pty(mut command: Command) -> io::Result<PtyOutput> {
    let pty = rustix_openpty::openpty(None, None)?;
    let mut controller = fs::File::from(pty.controller);
    let user = fs::File::from(pty.user);
    let stdout = user.try_clone()?;
    command
        .stderr(Stdio::from(user))
        .stdout(Stdio::from(stdout));
    let mut child = command.spawn()?;
    drop(command);
    let reader = thread::spawn(move || {
        let mut terminal = Vec::new();
        loop {
            let mut buffer = [0_u8; 4096];
            match controller.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => terminal.extend_from_slice(&buffer[..length]),
                Err(error)
                    if error.raw_os_error()
                        == Some(rustix_openpty::rustix::io::Errno::IO.raw_os_error()) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(terminal)
    });
    let status = child.wait()?;
    let terminal = reader
        .join()
        .map_err(|_| io::Error::other("PTY reader thread panicked"))??;
    Ok(PtyOutput { status, terminal })
}

fn create_remote(fixture: &Fixture, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let source = fixture.temp.path().join(format!("{name}-source"));
    let remote = fixture.temp.path().join(format!("{name}.git"));
    git(fixture.temp.path(), &["init", source.to_str().unwrap()])?;
    fs::write(source.join("README.md"), format!("{name}\n"))?;
    git(&source, &["add", "README.md"])?;
    git(
        &source,
        &[
            "-c",
            "user.name=Lager",
            "-c",
            "user.email=lager@example.com",
            "commit",
            "-m",
            "initial",
        ],
    )?;
    git(
        fixture.temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
    )?;
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )?;
    git(&source, &["push", "origin", "HEAD:main"])?;
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let output = support::external(cwd, "git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(())
}

#[test]
fn clone_success_repeated_matching_noop_honors_add_and_conflicts_are_untouched() -> TestResult {
    let fixture = Fixture::new()?;
    create_remote(&fixture, "matching")?;
    let reference = fixture.reference("matching");

    let first = fixture.run(&["add", &reference, "--no-register"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let repeated = fixture.run(&["add", &reference, "--register"]);
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert!(fixture.destination("matching").is_dir());
    assert!(fs::read_to_string(&fixture.config)?.contains("matching.git"));

    create_remote(&fixture, "conflict")?;
    let conflict_path = fixture.destination("conflict");
    fs::create_dir_all(&conflict_path)?;
    fs::write(conflict_path.join("keep.txt"), "untouched")?;
    let conflict = fixture.run(&["add", &fixture.reference("conflict"), "--no-register"]);
    assert_eq!(conflict.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(conflict_path.join("keep.txt"))?,
        "untouched"
    );
    Ok(())
}

#[test]
fn clone_repeated_inputs_continue_after_failure() -> TestResult {
    let fixture = Fixture::new()?;
    create_remote(&fixture, "continued")?;
    let missing = format!(
        "file://{}",
        fixture.temp.path().join("missing.git").display()
    );
    let good = fixture.reference("continued");
    let output = fixture.run(&["add", &missing, &good, "--no-register"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(fixture.destination("continued").is_dir());
    assert!(String::from_utf8_lossy(&output.stderr).contains(&missing));
    Ok(())
}

#[test]
fn ensure_success_is_fresh_hook_only_and_hook_reruns_explicitly() -> TestResult {
    let fixture = Fixture::new()?;
    create_remote(&fixture, "ensured")?;
    let marker = fixture.home.join("ensure-hook");
    let reference = fixture.reference("ensured");
    fs::write(
        &fixture.config,
        format!(
            "root = \"repos\"\n\n[[repositories]]\nurl = \"{reference}\"\npost_clone = \"printf x >> $HOME/ensure-hook\"\n"
        ),
    )?;

    let ensure = fixture.run(&["ensure"]);
    assert!(
        ensure.status.success(),
        "{}",
        String::from_utf8_lossy(&ensure.stderr)
    );
    let stderr = String::from_utf8_lossy(&ensure.stderr);
    assert!(
        stderr.contains(&format!(
            "Cloning {reference} into {}",
            fixture.destination("ensured").display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains(&format!("Cloned {reference}")), "{stderr}");
    assert!(stderr.contains("1 cloned"), "{stderr}");
    assert!(!stderr.contains("\u{1b}["), "{stderr}");

    let ensure_again = fixture.run(&["ensure"]);
    assert!(
        ensure_again.status.success(),
        "{}",
        String::from_utf8_lossy(&ensure_again.stderr)
    );
    let stderr = String::from_utf8_lossy(&ensure_again.stderr);
    assert!(stderr.contains("1 already present"), "{stderr}");
    assert!(
        !stderr.contains(&format!("Cloning {reference}")),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&marker)?.len(), 1);

    let hook = fixture.run(&["hook", &reference]);
    assert!(
        hook.status.success(),
        "{}",
        String::from_utf8_lossy(&hook.stderr)
    );
    assert_eq!(fs::read_to_string(&marker)?.len(), 2);
    Ok(())
}

#[test]
fn ensure_reports_each_failure_and_continues_to_successful_declarations() -> TestResult {
    let fixture = Fixture::new()?;
    create_remote(&fixture, "ensure-good")?;
    let missing = format!(
        "file://{}",
        fixture.temp.path().join("ensure-missing.git").display()
    );
    let good = fixture.reference("ensure-good");
    fs::write(
        &fixture.config,
        format!(
            "root = \"repos\"\n\n[[repositories]]\nurl = \"{missing}\"\n\n[[repositories]]\nurl = \"{good}\"\n"
        ),
    )?;

    let output = fixture.run(&["ensure"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(fixture.destination("ensure-good").is_dir());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let context = format!(
        "Cloning {missing} into {}",
        fixture.destination("ensure-missing").display()
    );
    assert!(stderr.contains(&context), "{stderr}");
    assert!(stderr.contains("Cloning into '.'"), "{stderr}");
    assert!(
        stderr.find(&context) < stderr.find("Cloning into '.'"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("Failed {missing}: git exited")),
        "{stderr}"
    );
    assert!(stderr.contains(&format!("Cloned {good}")), "{stderr}");
    assert!(stderr.contains("1 cloned, 1 failed"), "{stderr}");
    assert!(stderr.contains("fatal:"), "{stderr}");
    assert!(!stderr.contains("\u{1b}["), "{stderr}");
    Ok(())
}

#[test]
fn ensure_terminal_colors_progress_and_summaries() -> TestResult {
    let success_fixture = Fixture::new()?;
    create_remote(&success_fixture, "color-success")?;
    let success_reference = success_fixture.reference("color-success");
    fs::write(
        &success_fixture.config,
        format!("root = \"repos\"\n\n[[repositories]]\nurl = \"{success_reference}\"\n"),
    )?;

    let success = success_fixture.run_in_pty(&["ensure"])?;
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.terminal)
    );
    let success_terminal = String::from_utf8_lossy(&success.terminal);
    assert!(
        success_terminal.contains(&format!(
            "\u{1b}[34m●\u{1b}[0m  Cloning {success_reference} into "
        )),
        "{success_terminal:?}"
    );
    assert!(
        success_terminal.contains(&format!("\u{1b}[32m◆\u{1b}[0m  Cloned {success_reference}")),
        "{success_terminal:?}"
    );
    assert!(
        success_terminal.contains("\u{1b}[32m◆\u{1b}[0m  1 cloned"),
        "{success_terminal:?}"
    );
    assert!(success_terminal.contains("Cloning into '.'"));

    let failure_fixture = Fixture::new()?;
    let missing = format!(
        "file://{}",
        failure_fixture
            .temp
            .path()
            .join("color-missing.git")
            .display()
    );
    fs::write(
        &failure_fixture.config,
        format!("root = \"repos\"\n\n[[repositories]]\nurl = \"{missing}\"\n"),
    )?;

    let failure = failure_fixture.run_in_pty(&["ensure"])?;
    assert_eq!(failure.status.code(), Some(1));
    let failure_terminal = String::from_utf8_lossy(&failure.terminal);
    assert!(
        failure_terminal.contains(&format!("\u{1b}[34m●\u{1b}[0m  Cloning {missing} into ")),
        "{failure_terminal:?}"
    );
    assert!(
        failure_terminal.contains(&format!(
            "\u{1b}[31m■\u{1b}[0m  Failed {missing}: git exited"
        )),
        "{failure_terminal:?}"
    );
    assert!(
        failure_terminal.contains("\u{1b}[31m■\u{1b}[0m  1 failed"),
        "{failure_terminal:?}"
    );
    assert!(failure_terminal.contains("Cloning into '.'"));
    assert!(failure_terminal.contains("fatal:"));
    Ok(())
}

#[test]
fn post_clone_persists_before_hook_and_persistence_failure_skips_hook() -> TestResult {
    let fixture = Fixture::new()?;
    create_remote(&fixture, "persisted")?;
    let persisted = fixture.reference("persisted");
    let marker = fixture.home.join("persisted-hook");
    let command = "printf x >> $HOME/persisted-hook";
    let output = fixture.run(&["add", &persisted, "--register", "--post-clone", command]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(&fixture.config)?.contains("post_clone"));
    assert_eq!(fs::read_to_string(marker)?.len(), 1);

    create_remote(&fixture, "persist-fail")?;
    let failing = fixture.reference("persist-fail");
    let failing_marker = fixture.home.join("persist-fail-hook");
    fs::write(
        &fixture.config,
        format!(
            "root = \"repos\"\n\n[[repositories]]\nurl = \"{failing}\"\npost_clone = \"printf old >> $HOME/persist-fail-hook\"\n"
        ),
    )?;
    let failed = fixture.run(&[
        "add",
        &failing,
        "--register",
        "--post-clone",
        "printf new >> \"$HOME/persist-fail-hook\"",
    ]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(fixture.destination("persist-fail").is_dir());
    assert!(!failing_marker.exists());
    Ok(())
}

#[test]
fn hook_without_repositories_returns_usage_exit_two() -> TestResult {
    let fixture = Fixture::new()?;
    let output = fixture.run(&["hook"]);
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}
