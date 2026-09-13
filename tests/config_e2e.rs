mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn run(home: &Path, _cache: &Path, config: &Path, args: &[&str]) -> Output {
    support::lager(home, config)
        .args(args)
        .output()
        .expect("lager process")
}

#[test]
fn unknown_keys_warn_and_survive_binary_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let cache = temp.path().join("cache");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\nfuture_key = \"keep me\"\n")?;

    let output = run(&home, &cache, &config, &["register", "github.com/org/repo"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown config key `future_key`"));
    assert!(fs::read_to_string(config)?.contains("future_key = \"keep me\""));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_chains_and_dangling_targets_are_safe() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&home)?;

    let direct_target = temp.path().join("direct.toml");
    fs::write(&direct_target, "root = \"repos\"\n")?;
    let direct = temp.path().join("direct-link.toml");
    symlink(&direct_target, &direct)?;
    assert!(
        run(
            &home,
            &cache,
            &direct,
            &["register", "github.com/org/direct"]
        )
        .status
        .success()
    );
    assert!(direct.is_symlink());
    assert!(fs::read_to_string(&direct_target)?.contains("org/direct"));

    let relative_target = temp.path().join("relative.toml");
    fs::write(&relative_target, "root = \"repos\"\n")?;
    let relative_dir = temp.path().join("links");
    fs::create_dir_all(&relative_dir)?;
    let relative = relative_dir.join("config.toml");
    symlink(PathBuf::from("../relative.toml"), &relative)?;
    assert!(
        run(
            &home,
            &cache,
            &relative,
            &["register", "github.com/org/relative"]
        )
        .status
        .success()
    );
    assert!(relative.is_symlink());
    assert!(fs::read_to_string(&relative_target)?.contains("org/relative"));

    let multi_target = temp.path().join("multi.toml");
    fs::write(&multi_target, "root = \"repos\"\n")?;
    let hop_one = temp.path().join("hop-one.toml");
    let hop_two = temp.path().join("hop-two.toml");
    symlink(&multi_target, &hop_one)?;
    symlink(&hop_one, &hop_two)?;
    assert!(
        run(
            &home,
            &cache,
            &hop_two,
            &["register", "github.com/org/multi"]
        )
        .status
        .success()
    );
    assert!(hop_two.is_symlink() && hop_one.is_symlink());
    assert!(fs::read_to_string(&multi_target)?.contains("org/multi"));

    let dangling = temp.path().join("dangling.toml");
    symlink("missing.toml", &dangling)?;
    let output = run(
        &home,
        &cache,
        &dangling,
        &["register", "github.com/org/dangling"],
    );
    assert!(!output.status.success());
    assert!(dangling.is_symlink());
    Ok(())
}

#[cfg(unix)]
#[test]
fn mutation_preserves_mode_symlink_and_atomic_target() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let cache = temp.path().join("cache");
    let target = temp.path().join("target.toml");
    let alias = temp.path().join("alias.toml");
    fs::create_dir_all(&home)?;
    fs::write(&target, "root = \"repos\"\n")?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640))?;
    symlink(&target, &alias)?;

    let output = run(&home, &cache, &alias, &["register", "github.com/org/repo"]);
    assert!(output.status.success());
    assert_eq!(fs::metadata(&target)?.permissions().mode() & 0o777, 0o640);
    assert!(alias.is_symlink());
    assert!(fs::read_dir(temp.path())?.all(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .is_some_and(|name| !name.contains(".lager-"))
    }));
    Ok(())
}

#[test]
fn remove_missing_member_is_an_idempotent_binary_noop() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let cache = temp.path().join("cache");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;

    let first = run(
        &home,
        &cache,
        &config,
        &["unregister", "github.com/org/repo"],
    );
    let before = fs::read(&config)?;
    let second = run(
        &home,
        &cache,
        &config,
        &["unregister", "github.com/org/repo"],
    );
    assert!(first.status.success() && second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already unmanaged"));
    assert_eq!(before, fs::read(&config)?);
    Ok(())
}

#[test]
fn clone_config_failure_keeps_fresh_clone() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let cache = temp.path().join("cache");
    let source = temp.path().join("source");
    let remote = temp.path().join("remote.git");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    git(temp.path(), &["init", source.to_str().unwrap()])?;
    fs::write(source.join("README.md"), "lager\n")?;
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
            "init",
        ],
    )?;
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()])?;
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )?;
    git(&source, &["push", "origin", "HEAD:main"])?;
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[[repositories]]\nurl = \"file://{}\"\npost_clone = \"old\"\n",
            remote.display()
        ),
    )?;

    let reference = format!("file://{}", remote.display());
    let output = run(
        &home,
        &cache,
        &config,
        &["add", &reference, "--register", "--post-clone", "new"],
    );
    assert!(!output.status.success());
    assert!(home.join("repos").join("remote").is_dir());
    assert!(fs::read_to_string(config)?.contains("post_clone = \"old\""));
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
