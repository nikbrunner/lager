mod support;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
fn executable(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

struct HttpFixture {
    base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl HttpFixture {
    fn new(routes: Vec<(String, String)>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_thread = Arc::clone(&requests);
        let routes: Vec<_> = routes
            .into_iter()
            .map(|(path, body)| (path, body.replace("{BASE}", &base)))
            .collect();
        thread::spawn(move || {
            let routes: HashMap<_, _> = routes.into_iter().collect();
            loop {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_nonblocking(false);
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let count = stream.read(&mut buffer).unwrap_or(0);
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let mut lines = text.lines();
                let path = lines
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_owned();
                let authorization = text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim())
                    })
                    .unwrap_or("")
                    .to_owned();
                requests_thread
                    .lock()
                    .unwrap()
                    .push((path.clone(), authorization));
                let (status, body) = routes
                    .get(&path)
                    .map(|body| ("200 OK", body.as_str()))
                    .unwrap_or(("404 Not Found", "{}"));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base, requests }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
    }
}

fn git(directory: &Path, args: &[&str]) -> std::io::Result<()> {
    let output = support::external(directory, "git")
        .current_dir(directory)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(String::from_utf8_lossy(
            &output.stderr,
        )))
    }
}

fn gitconfig(temp: &Path, prefix: &str, remote_root: &Path) -> std::io::Result<std::path::PathBuf> {
    let config = temp.join("gitconfig");
    std::fs::write(
        &config,
        format!(
            "[url \"file://{}/\"]\n    insteadOf = {}\n",
            remote_root.display(),
            prefix
        ),
    )?;
    Ok(config)
}

fn bare_remote(temp: &Path, name: &str) -> std::io::Result<std::path::PathBuf> {
    let source = temp.join(format!("{name}-source"));
    let remote = temp.join(format!("{name}.git"));
    git(temp, &["init", source.to_str().unwrap()])?;
    git(&source, &["config", "user.email", "test@example.invalid"])?;
    git(&source, &["config", "user.name", "Test"])?;
    std::fs::write(source.join("README.md"), name)?;
    git(&source, &["add", "README.md"])?;
    git(&source, &["commit", "-m", "init"])?;
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
fn github_catalog_drives_register_add_multi_selection_and_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    let remotes = temp.path().join("remotes/org");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&bin)?;
    std::fs::create_dir_all(&remotes)?;
    bare_remote(&remotes, "one")?;
    bare_remote(&remotes, "two")?;
    let git_config = gitconfig(temp.path(), "git@github.com:", &temp.path().join("remotes"))?;
    executable(
        &bin.join("gh"),
        r##"#!/bin/sh
printf '%s' "$*" > "$GH_ARGS_LOG"
cat <<'JSON'
[{"data":{"viewer":{"repositories":{"nodes":[{"name":"one","isArchived":false,"viewerCanAccess":true,"sshUrl":"git@github.com:org/one.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":true,"endCursor":"cursor-1"}}}}},{"data":{"viewer":{"repositories":{"nodes":[{"name":"two","isArchived":false,"viewerCanAccess":true,"sshUrl":"git@github.com:org/two.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]
JSON
"##,
    )?;
    executable(
        &bin.join("fzf"),
        r##"#!/bin/sh
if [ "$FZF_MODE" = cancel ]; then exit 130; fi
if [ "$FZF_MODE" = one ]; then
  cat > /dev/null
  printf '%s\n' github.com/org/one
else
  cat
fi
"##,
    )?;
    let config = temp.path().join("register.toml");
    std::fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n",
    )?;
    let registered = support::lager_with_path(&home, &config, Some(&bin))
        .env("FZF_MODE", "all")
        .env("GH_ARGS_LOG", temp.path().join("gh-args"))
        .args(["register"])
        .output()?;
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&config)?
            .matches("[[repositories]]")
            .count(),
        2
    );
    assert!(std::fs::read_to_string(temp.path().join("gh-args"))?.contains("after:$endCursor"));

    let add_config = temp.path().join("add.toml");
    std::fs::write(
        &add_config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"forge/team\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\n",
    )?;
    let added = support::lager_with_path(&home, &add_config, Some(&bin))
        .env("FZF_MODE", "one")
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .args(["add", "--no-register"])
        .output()?;
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(home.join("repos/forge/team/org/one/README.md").is_file());
    std::fs::create_dir_all(home.join("repos/forge/team/org/two"))?;
    std::fs::write(home.join("repos/forge/team/org/two/untracked.txt"), "keep")?;
    let conflict = support::lager_with_path(&home, &add_config, Some(&bin))
        .env("FZF_MODE", "all")
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .args(["add", "--no-register"])
        .output()?;
    assert_eq!(conflict.status.code(), Some(1));
    assert!(
        home.join("repos/forge/team/org/two/untracked.txt")
            .is_file()
    );

    std::fs::write(
        &add_config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"forge/team\"\n",
    )?;
    let cancelled = support::lager_with_path(&home, &add_config, Some(&bin))
        .env("FZF_MODE", "cancel")
        .args(["register"])
        .output()?;
    assert_eq!(cancelled.status.code(), Some(130));
    Ok(())
}

#[test]
fn init_persists_portable_root_and_github_provider() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;

    let output = support::lager(&home, &config)
        .args(["init", "--root", "repos", "--create-root", "--github"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join("repos").is_dir());
    let contents = std::fs::read_to_string(config)?;
    assert!(contents.contains("root = \"repos\""));
    assert!(contents.contains("[providers.\"github.com\"]"));
    assert!(contents.contains("preset = \"github\""));
    Ok(())
}

#[test]
fn add_remove_is_idempotent_and_reenables_wildcard_members()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    std::fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\n",
    )?;

    let remove = run(&home, &config, &["unregister", "github.com/org/repo"])?;
    assert!(remove.status.success());
    let excluded = std::fs::read_to_string(&config)?;
    assert!(excluded.contains("exclude = [\"repo\"]"));

    let add = run(&home, &config, &["register", "github.com/org/repo"])?;
    assert!(add.status.success());
    assert!(!std::fs::read_to_string(&config)?.contains("repo\"]"));

    let add_again = run(&home, &config, &["register", "github.com/org/repo"])?;
    assert!(add_again.status.success());
    assert!(String::from_utf8(add_again.stdout)?.contains("already managed"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn remote_list_uses_configured_github_cli_and_filters_archived_repositories()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&bin)?;
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        r##"#!/bin/sh
cat <<'JSON'
[{"data":{"organization":{"repositories":{"nodes":[{"name":"active","isArchived":false,"viewerCanAccess":true,"sshUrl":"git@github.com:org/active.git","owner":{"login":"org"}},{"name":"old","isArchived":true,"viewerCanAccess":true,"sshUrl":"git@github.com:org/old.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]
JSON
"##,
    )?;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\nprefix = \"github/team\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\n",
    )?;

    let output = support::lager_with_path(&home, &config, Some(&bin))
        .args(["list", "--remote", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let rows = document["repositories"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rows[0]["identity"], "github.com/org/active");
    assert!(
        rows[0]["destination"]
            .as_str()
            .unwrap()
            .ends_with("/repos/github/team/org/active")
    );
    Ok(())
}

#[test]
fn cloud_pagination_rejects_cross_origin_before_bearer_or_basic_credentials_leak()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    let attacker = HttpFixture::new(vec![(
        "/steal".to_owned(),
        "{\"values\":[],\"next\":null}".to_owned(),
    )]);

    for (auth, fields, variables) in [
        (
            "bearer",
            "token_env = \"CLOUD_TOKEN\"",
            vec![("CLOUD_TOKEN", "credential-must-not-cross-origin")],
        ),
        (
            "basic",
            "username_env = \"CLOUD_USER\"\npassword_env = \"CLOUD_PASSWORD\"",
            vec![
                ("CLOUD_USER", "alice"),
                ("CLOUD_PASSWORD", "credential-must-not-cross-origin"),
            ],
        ),
    ] {
        let trusted = HttpFixture::new(vec![(
            "/2.0/repositories/workspace".to_owned(),
            format!("{{\"values\":[],\"next\":\"{}/steal\"}}", attacker.base),
        )]);
        std::fs::write(
            &config,
            format!(
                "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"{}\"\nauth = \"{auth}\"\n{fields}\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n",
                trusted.base
            ),
        )?;
        let output = run_with_env(&home, &config, &["list", "--remote", "--json"], &variables)?;
        assert_eq!(output.status.code(), Some(1), "{auth}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("origin"));
        assert!(attacker.requests().is_empty(), "{auth} credentials leaked");
    }
    Ok(())
}

#[test]
fn cloud_pagination_accepts_relative_and_same_origin_opaque_links()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    let fixture = HttpFixture::new(vec![
        (
            "/2.0/repositories/workspace".to_owned(),
            "{\"values\":[],\"next\":\"/opaque?page=2\"}".to_owned(),
        ),
        (
            "/opaque?page=2".to_owned(),
            "{\"values\":[],\"next\":\"{BASE}/opaque?page=3\"}".to_owned(),
        ),
        (
            "/opaque?page=3".to_owned(),
            "{\"values\":[],\"next\":null}".to_owned(),
        ),
    ]);
    std::fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"{}\"\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n",
            fixture.base
        ),
    )?;
    let output = run(&home, &config, &["list", "--remote", "--json"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = fixture.requests();
    assert!(requests.iter().any(|(path, _)| path == "/opaque?page=2"));
    assert!(requests.iter().any(|(path, _)| path == "/opaque?page=3"));
    Ok(())
}

#[test]
fn cloud_listing_follows_opaque_next_filters_archived_and_ensures_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    let remote_root = temp.path().join("remotes");
    std::fs::create_dir_all(&remote_root)?;
    std::fs::create_dir_all(remote_root.join("workspace"))?;
    let _remote = bare_remote(&remote_root.join("workspace"), "active")?;
    let git_config = gitconfig(temp.path(), "git@bitbucket.org:", &remote_root)?;
    let workspace_body =
        "{\"values\":[{\"workspace\":{\"slug\":\"workspace\"}}],\"next\":null}".to_owned();
    let fixture = HttpFixture::new(vec![
        ("/2.0/user/permissions/workspaces".to_owned(), workspace_body),
        (
            "/2.0/repositories/workspace".to_owned(),
            "{\"values\":[{\"is_archived\":false,\"full_name\":\"workspace/active\",\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://git@bitbucket.org/workspace/active.git\"}]} }],\"next\":\"{BASE}/opaque?page=2\"}".replace(" }", "}"),
        ),
        (
            "/opaque?page=2".to_owned(),
            "{\"values\":[{\"is_archived\":true,\"full_name\":\"workspace/old\",\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://git@bitbucket.org/workspace/old.git\"}]}}],\"next\":null}".to_owned(),
        ),
    ]);
    std::fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\nprefix = \"cloud/team\"\napi_url = \"{}\"\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n",
            fixture.base
        ),
    )?;
    let list = run_with_env(
        &home,
        &config,
        &["list", "--remote", "--json"],
        &[("GIT_CONFIG_GLOBAL", "")],
    )?;
    assert!(
        list.status.success(),
        "{} requests={:?}",
        String::from_utf8_lossy(&list.stderr),
        fixture.requests()
    );
    let listed: serde_json::Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(listed["repositories"].as_array().unwrap().len(), 1);
    assert!(
        listed["repositories"][0]["destination"]
            .as_str()
            .unwrap()
            .ends_with("/repos/cloud/team/workspace/active")
    );
    let include = run(
        &home,
        &config,
        &["list", "--remote", "--include-archived", "--json"],
    )?;
    assert!(include.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&include.stdout)?["repositories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let excluded_config = std::fs::read_to_string(&config)?.replace(
        "url = \"bitbucket.org/workspace/*\"",
        "url = \"bitbucket.org/workspace/*\"\nexclude = [\"old\"]",
    );
    std::fs::write(&config, excluded_config)?;
    let excluded = run(
        &home,
        &config,
        &["list", "--remote", "--include-archived", "--json"],
    )?;
    assert!(excluded.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&excluded.stdout)?["repositories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let requests = fixture.requests();
    assert!(
        requests
            .iter()
            .any(|(path, auth)| path == "/opaque?page=2" && auth.is_empty())
    );
    let ensured = run_with_env(
        &home,
        &config,
        &["ensure"],
        &[("GIT_CONFIG_GLOBAL", git_config.to_str().unwrap())],
    )?;
    assert!(
        ensured.status.success(),
        "{}",
        String::from_utf8_lossy(&ensured.stderr)
    );
    assert!(
        home.join("repos/cloud/team/workspace/active/README.md")
            .is_file()
    );
    git(
        &home.join("repos/cloud/team/workspace/active"),
        &[
            "remote",
            "set-url",
            "origin",
            "git@bitbucket.org:workspace/active.git",
        ],
    )?;
    let origin = support::external(&home, "git")
        .args([
            "-C",
            home.join("repos/cloud/team/workspace/active")
                .to_str()
                .unwrap(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&origin.stdout).trim(),
        "git@bitbucket.org:workspace/active.git"
    );
    let second = run(&home, &config, &["ensure"])?;
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn add_keeps_selected_repository_when_another_provider_fails()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    let config = temp.path().join("config.toml");
    let remotes = temp.path().join("remotes/org");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&bin)?;
    std::fs::create_dir_all(&remotes)?;
    bare_remote(&remotes, "active")?;
    let git_config = gitconfig(temp.path(), "git@github.com:", &temp.path().join("remotes"))?;
    let system_git = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).find(|dir| dir.join("git").is_file()))
        .ok_or("could not locate git")?;
    symlink(system_git.join("git"), bin.join("git"))?;
    executable(
        &bin.join("gh"),
        r##"#!/bin/sh
case "$*" in
  *organization*)
    printf '%s' '[{"data":{"organization":{"repositories":{"nodes":[{"name":"active","isArchived":false,"viewerCanAccess":true,"sshUrl":"git@github.com:org/active.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]'
    ;;
  *)
    printf '%s' '[{"data":{"viewer":{"repositories":{"nodes":[{"name":"active","isArchived":false,"viewerCanAccess":true,"sshUrl":"git@github.com:org/active.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]'
    ;;
esac
"##,
    )?;
    executable(
        &bin.join("fzf"),
        "#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' github.com/org/active\n",
    )?;
    std::fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"github.com\"]\npreset = \"github\"\n\n[providers.\"dc.example\"]\npreset = \"bitbucket-data-center\"\napi_url = \"http://127.0.0.1:9\"\n\n[[repositories]]\nurl = \"github.com/org/*\"\n\n[[repositories]]\nurl = \"dc.example/project/*\"\n",
    )?;

    let output = support::lager_with_path(&home, &config, Some(&bin))
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .args(["add", "--no-register"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("dc.example"));
    assert!(home.join("repos/org/active/README.md").is_file());
    Ok(())
}

#[cfg(unix)]
#[test]
fn provider_failures_preserve_successful_rows_and_skip_fzf_when_all_fail()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&bin)?;
    let remote_root = temp.path().join("remotes");
    std::fs::create_dir_all(remote_root.join("workspace"))?;
    let _remote = bare_remote(&remote_root.join("workspace"), "active")?;
    let git_config = gitconfig(temp.path(), "git@bitbucket.org:", &remote_root)?;
    let fixture = HttpFixture::new(vec![
        (
            "/2.0/repositories/workspace".to_owned(),
            "{\"values\":[{\"is_archived\":false,\"full_name\":\"workspace/active\",\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://git@bitbucket.org/workspace/active.git\"}]} }],\"next\":null}".replace(" }", "}"),
        ),
    ]);
    std::fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"{}\"\n\n[providers.\"dc.example\"]\npreset = \"bitbucket-data-center\"\napi_url = \"http://127.0.0.1:9\"\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n\n[[repositories]]\nurl = \"dc.example/project/*\"\n",
            fixture.base
        ),
    )?;
    let listed = run(&home, &config, &["list", "--remote", "--json"])?;
    assert_eq!(listed.status.code(), Some(1));
    let document: serde_json::Value = serde_json::from_slice(&listed.stdout)?;
    assert_eq!(document["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(document["provider_errors"].as_array().unwrap().len(), 1);
    assert!(String::from_utf8_lossy(&listed.stderr).contains("dc.example"));
    let ensured = run_with_env(
        &home,
        &config,
        &["ensure"],
        &[("GIT_CONFIG_GLOBAL", git_config.to_str().unwrap())],
    )?;
    assert_eq!(ensured.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&ensured.stderr).contains("dc.example"));
    assert!(
        !String::from_utf8_lossy(&ensured.stderr).contains("wildcard references cannot be cloned")
    );
    assert!(home.join("repos/workspace/active/README.md").is_file());
    let origin = support::external(&home, "git")
        .args([
            "-C",
            home.join("repos/workspace/active").to_str().unwrap(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&origin.stdout).trim(),
        "git@bitbucket.org:workspace/active.git"
    );

    executable(
        &bin.join("fzf"),
        "#!/bin/sh\nprintf '%s' > \"$FZF_MARKER\"\n",
    )?;
    std::fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"dc.example\"]\npreset = \"bitbucket-data-center\"\napi_url = \"http://127.0.0.1:9\"\n\n[[repositories]]\nurl = \"dc.example/project/*\"\n",
    )?;
    let marker = temp.path().join("fzf-used");
    let picked = support::lager_with_path(&home, &config, Some(&bin))
        .env("FZF_MARKER", &marker)
        .args(["add", "--no-register"])
        .output()?;
    assert_eq!(picked.status.code(), Some(1));
    assert!(
        !marker.exists(),
        "fzf must not run when every provider fails"
    );
    Ok(())
}

#[test]
fn data_center_uses_personal_project_path_auth_headers_and_destination_mapping()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    std::fs::create_dir_all(&home)?;
    let remote_root = temp.path().join("dc-remotes");
    std::fs::create_dir_all(remote_root.join("~alice"))?;
    bare_remote(&remote_root.join("~alice"), "active")?;
    let git_config = gitconfig(temp.path(), "ssh://forge@dc.example:2222/", &remote_root)?;
    let fixture = HttpFixture::new(vec![
        (
            "/rest/api/1.0/projects/~alice/repos?start=0&limit=25".to_owned(),
            "{\"values\":[{\"archived\":false,\"slug\":\"active\",\"project\":{\"key\":\"~alice\"},\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://old@dc.example:7999/~alice/active.git\"}]}}],\"isLastPage\":false,\"nextPageStart\":7,\"start\":0,\"limit\":25}".to_owned(),
        ),
        (
            "/rest/api/1.0/projects/~alice/repos?start=7&limit=25".to_owned(),
            "{\"values\":[{\"archived\":true,\"slug\":\"old\",\"project\":{\"key\":\"~alice\"},\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://old@dc.example:7999/~alice/old.git\"}]}}],\"isLastPage\":true,\"start\":7,\"limit\":25}".to_owned(),
        ),
    ]);
    std::fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"dc.example\"]\npreset = \"bitbucket-data-center\"\nprefix = \"company\"\napi_url = \"{}\"\nauth = \"bearer\"\ntoken_env = \"DC_TOKEN\"\nssh_user = \"forge\"\nssh_port = 2222\n\n[[repositories]]\nurl = \"dc.example/~alice/*\"\n",
            fixture.base
        ),
    )?;
    let bearer = run_with_env(
        &home,
        &config,
        &["list", "--remote", "--json"],
        &[("DC_TOKEN", "secret-token")],
    )?;
    assert!(
        bearer.status.success(),
        "{} requests={:?}",
        String::from_utf8_lossy(&bearer.stderr),
        fixture.requests()
    );
    let document: serde_json::Value = serde_json::from_slice(&bearer.stdout)?;
    let row = &document["repositories"][0];
    assert_eq!(
        row["clone_url"],
        "ssh://forge@dc.example:2222/~alice/active.git"
    );
    assert!(
        row["destination"]
            .as_str()
            .unwrap()
            .ends_with("/company/alice/active")
    );
    assert_eq!(document["repositories"].as_array().unwrap().len(), 1);
    let requests = fixture.requests();
    assert!(requests.iter().any(
        |(path, auth)| path.contains("projects/~alice/repos?start=0")
            && auth == "Bearer secret-token"
    ));
    let ensured = run_with_env(
        &home,
        &config,
        &["ensure"],
        &[
            ("DC_TOKEN", "secret-token"),
            ("GIT_CONFIG_GLOBAL", git_config.to_str().unwrap()),
        ],
    )?;
    assert!(
        ensured.status.success(),
        "{}",
        String::from_utf8_lossy(&ensured.stderr)
    );
    assert!(home.join("repos/company/alice/active/README.md").is_file());

    std::fs::write(
        &config,
        std::fs::read_to_string(&config)?.replace(
            "auth = \"bearer\"\ntoken_env = \"DC_TOKEN\"",
            "auth = \"basic\"\nusername_env = \"DC_USER\"\npassword_env = \"DC_PASSWORD\"",
        ),
    )?;
    let basic = run_with_env(
        &home,
        &config,
        &["list", "--remote", "--include-archived", "--json"],
        &[("DC_USER", "alice"), ("DC_PASSWORD", "password")],
    )?;
    assert!(
        basic.status.success(),
        "{}",
        String::from_utf8_lossy(&basic.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&basic.stdout)?["repositories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let requests = fixture.requests();
    assert!(requests.iter().any(|(_, auth)| auth.starts_with("Basic ")));
    Ok(())
}

fn run_with_env(
    home: &Path,
    config: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = support::lager(home, config);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    Ok(command.output()?)
}

fn run(
    home: &std::path::Path,
    config: &std::path::Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(support::lager(home, config).args(args).output()?)
}
