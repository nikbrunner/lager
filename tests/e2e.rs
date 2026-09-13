mod support;

use std::fs;
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn clone_explicit_local_remote_uses_exact_destination() -> TestResult {
    let temp = tempfile::tempdir()?;
    let remote = temp.path().join("remote.git");
    let source = temp.path().join("source");
    let home = temp.path().join("home");
    let root = home.join("repos");
    let config = temp.path().join("config.toml");

    run_git(temp.path(), ["init", "--bare", remote.to_str().unwrap()])?;
    run_git(temp.path(), ["init", source.to_str().unwrap()])?;
    fs::write(source.join("README.md"), "lager\n")?;
    run_git(&source, ["add", "README.md"])?;
    run_git_env(
        &source,
        [
            "-c",
            "user.name=Lager Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    )?;
    run_git(&source, ["branch", "-M", "main"])?;
    run_git(
        &source,
        ["remote", "add", "origin", remote.to_str().unwrap()],
    )?;
    run_git(&source, ["push", "origin", "main"])?;
    run_git(&remote, ["symbolic-ref", "HEAD", "refs/heads/main"])?;
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"~/repos\"\n")?;

    let output = support::lager(&home, &config)
        .args([
            "add",
            &format!("file://{}", remote.display()),
            "--no-register",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let destination = root.join("remote");
    assert!(destination.join("README.md").is_file());
    assert_eq!(
        git_output(&destination, ["remote", "get-url", "origin"])?,
        format!("file://{}", remote.display())
    );
    Ok(())
}

fn run_git<I, S>(cwd: &Path, args: I) -> TestResult
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run_git_env(cwd, args)
}

fn run_git_env<I, S>(cwd: &Path, args: I) -> TestResult
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = support::external(cwd, "git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(())
}

fn git_output<I, S>(cwd: &Path, args: I) -> Result<String, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = support::external(cwd, "git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
