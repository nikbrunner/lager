mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lager(home: &Path, config: &Path, args: &[&str]) -> Output {
    support::lager(home, config)
        .args(args)
        .output()
        .expect("lager process")
}

fn git(cwd: &Path, args: &[&str]) -> TestResult {
    let output = support::external(cwd, "git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    Ok(())
}

fn bare_remote(temp: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source = temp.join("source");
    let remote = temp.join("remote.git");
    git(temp, &["init", source.to_str().unwrap()])?;
    fs::write(source.join("README"), "safe\n")?;
    git(&source, &["add", "README"])?;
    git(
        &source,
        &[
            "-c",
            "user.name=Lager",
            "-c",
            "user.email=lager@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    )?;
    git(temp, &["init", "--bare", remote.to_str().unwrap()])?;
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )?;
    git(&source, &["push", "origin", "HEAD:main"])?;
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    Ok(remote)
}

#[cfg(unix)]
#[test]
fn clone_rejects_existing_symlink_ancestor_without_writing_outside_root() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let outside = temp.path().join("outside");
    let config = temp.path().join("config.toml");
    let remotes = temp.path().join("remotes/org");
    fs::create_dir_all(home.join("repos"))?;
    fs::create_dir_all(&outside)?;
    fs::create_dir_all(&remotes)?;
    let remote = bare_remote(temp.path())?;
    fs::rename(remote, remotes.join("repo.git"))?;
    let git_config = temp.path().join("gitconfig");
    fs::write(
        &git_config,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = git@github.com:\n",
            temp.path().join("remotes").display()
        ),
    )?;
    symlink(&outside, home.join("repos/org"))?;
    fs::write(&config, "root = \"repos\"\n")?;

    let output = support::lager(&home, &config)
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .args(["add", "github.com/org/repo", "--no-register"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
    assert!(!outside.join("repo").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn clone_stays_in_held_root_when_its_path_is_replaced_after_git_starts() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let root = home.join("repos");
    let moved_root = home.join("held-root");
    let outside = temp.path().join("outside");
    let config = temp.path().join("config.toml");
    let bin = temp.path().join("bin");
    let started = temp.path().join("started");
    let release = temp.path().join("release");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    fs::create_dir_all(&bin)?;
    fs::write(&config, "root = \"repos\"\n")?;
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nprintf started > \"$STARTED\"\nwhile [ ! -e \"$RELEASE\" ]; do :; done\nif [ \"$3\" = . ]; then printf held > marker; else mkdir -p \"$3\"; printf escaped > \"$3/marker\"; fi\n",
    )?;
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755))?;

    let child = support::lager_with_path(&home, &config, Some(&bin))
        .env("STARTED", &started)
        .env("RELEASE", &release)
        .args(["add", "github.com/org/repo", "--no-register"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(started.exists(), "git shim did not start");
    fs::rename(&root, &moved_root)?;
    symlink(&outside, &root)?;
    fs::write(&release, "go")?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(moved_root.join("org/repo/marker").is_file());
    assert!(!outside.join("org/repo/marker").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_clone_cleans_its_fresh_destination_via_held_parent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let destination = home.join("repos/org/repo");
    let config = temp.path().join("config.toml");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&bin)?;
    fs::write(&config, "root = \"repos\"\n")?;
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nif [ \"$3\" = . ]; then printf partial > partial; else mkdir -p \"$3\"; printf partial > \"$3/partial\"; fi\nexit 1\n",
    )?;
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755))?;
    let output = support::lager_with_path(&home, &config, Some(&bin))
        .args(["add", "github.com/org/repo", "--no-register"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        !destination.exists(),
        "failed clone destination was retained"
    );
    Ok(())
}

#[test]
fn clone_safely_creates_a_missing_root_and_parent_chain() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    let remote = bare_remote(temp.path())?;
    fs::write(&config, "root = \"repos/nested\"\n")?;

    let output = lager(
        &home,
        &config,
        &[
            "add",
            &format!("file://{}", remote.display()),
            "--no-register",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join("repos/nested/remote/README").is_file());
    Ok(())
}

#[test]
fn every_invalid_semantic_config_fails_before_a_subprocess() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    let marker = temp.path().join("subprocess-ran");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&bin)?;
    #[cfg(unix)]
    for name in ["git", "gh"] {
        let executable = bin.join(name);
        fs::write(
            &executable,
            "#!/bin/sh\nprintf invoked > \"$MARKER\"\nexit 99\n",
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }

    let cases = [
        (
            "wildcard-hook",
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/*\"\npost_clone = \"false\"\n",
        ),
        (
            "internal-wildcard",
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/*/repo\"\n",
        ),
        (
            "duplicate-conflict",
            "root = \"repos\"\n[[repositories]]\nurl = \"git@github.com:org/repo.git\"\npost_clone = \"one\"\n[[repositories]]\nurl = \"https://github.com/org/repo.git\"\npost_clone = \"two\"\n",
        ),
        (
            "prefix-escape",
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"../outside\"\n",
        ),
        (
            "exclude-escape",
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"../outside\"]\n",
        ),
        (
            "unnormalized-prefix",
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"team/./nested\"\n",
        ),
        (
            "unnormalized-exclusion",
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n[[repositories]]\nurl = \"github.com/org/*\"\nexclude = [\"team//repo\"]\n",
        ),
        (
            "wildcard-provider-required",
            "root = \"repos\"\n[[repositories]]\nurl = \"github.com/org/*\"\n",
        ),
        (
            "unknown-preset",
            "root = \"repos\"\n[providers.\"git.example.com\"]\npreset = \"unknown\"\n",
        ),
        (
            "unsafe-api-url",
            "root = \"repos\"\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"ftp://user:secret@example.com\"\n",
        ),
        (
            "anonymous-credentials",
            "root = \"repos\"\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\ntoken_env = \"TOKEN\"\n",
        ),
        (
            "bad-bearer-auth",
            "root = \"repos\"\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\nauth = \"bearer\"\nusername_env = \"USER\"\n",
        ),
        (
            "bad-basic-auth",
            "root = \"repos\"\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\nauth = \"basic\"\nusername_env = \"USER\"\n",
        ),
        (
            "dc-api-required",
            "root = \"repos\"\n[providers.\"dc.example\"]\npreset = \"bitbucket-data-center\"\n",
        ),
        (
            "unknown-canonical-host",
            "root = \"repos\"\n[[repositories]]\nurl = \"git.example.com/team/repo\"\n",
        ),
    ];

    let valid = temp.path().join("valid.toml");
    fs::write(&valid, "root = \"repos\"\n")?;
    let invoked = support::lager_with_path(&home, &valid, Some(&bin))
        .env("MARKER", &marker)
        .args([
            "add",
            "file:///guaranteed-external-operation.git",
            "--no-register",
        ])
        .output()?;
    assert_eq!(invoked.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&marker)?, "invoked");
    fs::remove_file(&marker)?;

    for (name, contents) in cases {
        let config = temp.path().join(format!("{name}.toml"));
        fs::write(&config, contents)?;
        let output = support::lager_with_path(&home, &config, Some(&bin))
            .env("MARKER", &marker)
            .args([
                "add",
                "file:///guaranteed-external-operation.git",
                "--no-register",
            ])
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists(), "{name} started a subprocess");
    }
    Ok(())
}

#[test]
fn full_clone_url_for_unknown_host_remains_valid_and_preserved() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(
        &config,
        "root = \"repos\"\n[[repositories]]\nurl = \"ssh://git@git.example.com:2222/team/repo.git\"\n",
    )?;
    let output = lager(&home, &config, &["list", "--json"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json["repositories"][0]["clone_url"],
        "ssh://git@git.example.com:2222/team/repo.git"
    );
    Ok(())
}

#[test]
fn register_accepts_only_trailing_wildcards_and_add_rejects_them() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(
        &config,
        "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\n",
    )?;

    let registered = lager(&home, &config, &["register", "github.com/org/*"]);
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stderr)
    );
    assert!(fs::read_to_string(&config)?.contains("github.com/org/*"));
    let again = lager(&home, &config, &["register", "github.com/org/*"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("already managed"));

    let added = lager(
        &home,
        &config,
        &["add", "github.com/org/*", "--no-register"],
    );
    assert_eq!(added.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&added.stderr).contains("wildcard"));
    Ok(())
}

#[test]
fn explicit_configured_reference_uses_one_exact_path_for_list_ensure_and_hook() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    let remotes = temp.path().join("remotes/org");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&remotes)?;
    let remote = bare_remote(temp.path())?;
    fs::rename(remote, remotes.join("repo.git"))?;
    let git_config = temp.path().join("gitconfig");
    fs::write(
        &git_config,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = git@github.com:\n",
            temp.path().join("remotes").display()
        ),
    )?;
    fs::write(
        &config,
        "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"work/team\"\n[[repositories]]\nurl = \"git@github.com:org/repo.git\"\npost_clone = \"pwd > \\\"$HOOK_LOG\\\"\"\n",
    )?;
    let hook_log = temp.path().join("hook-path");
    let ensure = support::lager(&home, &config)
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("HOOK_LOG", &hook_log)
        .args(["ensure"])
        .output()?;
    assert!(
        ensure.status.success(),
        "{}",
        String::from_utf8_lossy(&ensure.stderr)
    );
    let exact = home.join("repos/work/team/org/repo");
    let canonical_exact = fs::canonicalize(&exact)?;
    assert!(exact.join("README").is_file());
    assert_eq!(
        fs::read_to_string(&hook_log)?.trim(),
        canonical_exact.to_str().unwrap()
    );

    let listed = lager(&home, &config, &["list", "--json"]);
    assert!(listed.status.success());
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout)?;
    assert_eq!(
        json["repositories"][0]["destination"],
        exact.to_str().unwrap()
    );

    fs::remove_file(&hook_log)?;
    let hook = support::lager(&home, &config)
        .env("HOOK_LOG", &hook_log)
        .args(["hook", "github.com/org/repo"])
        .output()?;
    assert!(hook.status.success());
    assert_eq!(
        fs::read_to_string(hook_log)?.trim(),
        canonical_exact.to_str().unwrap()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn no_argument_remove_deletes_the_exact_custom_clone_selected_by_path() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let root = home.join("repos");
    let custom = root.join("custom/location");
    let config = temp.path().join("config.toml");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&bin)?;
    let remote = bare_remote(temp.path())?;
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    fs::create_dir_all(custom.parent().unwrap())?;
    git(
        temp.path(),
        &[
            "clone",
            &format!("file://{}", remote.display()),
            custom.to_str().unwrap(),
        ],
    )?;
    fs::write(&config, "root = \"repos\"\n")?;
    symlink(support::real_tool("git"), bin.join("git"))?;
    let fzf = bin.join("fzf");
    let fzf_log = temp.path().join("fzf-input");
    fs::write(
        &fzf,
        "#!/bin/sh\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> \"$FZF_LOG\"; printf '%s\\n' \"$line\"; done\n",
    )?;
    fs::set_permissions(&fzf, fs::Permissions::from_mode(0o755))?;

    let output = support::lager_with_path(&home, &config, Some(&bin))
        .env("FZF_LOG", &fzf_log)
        .args(["remove", "--keep-registered", "--yes", "--force"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let picker_input = fs::read_to_string(&fzf_log).unwrap_or_default();
    assert!(
        picker_input.contains(custom.to_str().unwrap()),
        "picker did not display the exact custom path; input={picker_input:?}"
    );
    assert!(
        !custom.exists(),
        "selected custom clone was not removed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("remote").exists());
    Ok(())
}

#[test]
fn release_please_bootstrap_contract_is_exactly_v_0_1_0() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".github/release-please-config.json"))?)?;
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
        root.join(".github/.release-please-manifest.json"),
    )?)?;
    let package = &config["packages"]["."];
    assert_eq!(package["release-type"], "rust");
    assert_eq!(package["initial-version"], "0.1.0");
    assert!(package.get("bootstrap-sha").is_none());
    assert!(package.get("bump-minor-pre-major").is_none());
    assert!(package.get("bump-patch-for-minor-pre-major").is_none());
    assert_eq!(manifest, serde_json::json!({}));
    assert!(fs::read_to_string(root.join("Cargo.toml"))?.contains("version = \"0.1.0\""));
    Ok(())
}

#[test]
fn unregister_rejects_post_clone_as_usage() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;
    let output = lager(
        &home,
        &config,
        &["unregister", "github.com/org/repo", "--post-clone", "false"],
    );
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}
