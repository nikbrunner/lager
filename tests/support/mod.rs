use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// Logs `$GH_HOST $*` per call and keeps the repository's state in `$GH_STATE`.
#[allow(dead_code)]
pub const FAKE_GH: &str = r##"#!/bin/sh
printf '%s %s\n' "$GH_HOST" "$*" >> "$GH_ARGS_LOG"
case "$1 $2" in
  "auth status")
    if [ -e "$GH_STATE/offline" ]; then
      printf '%s\n' "error connecting to api.github.com" >&2
      exit 1
    fi
    if [ -e "$GH_STATE/logged-out" ]; then
      printf '%s\n' "You are not logged into any GitHub hosts. To log in, run: gh auth login" >&2
      exit 1
    fi
    ;;
  "repo view")
    if [ -e "$GH_STATE/free" ] && [ "$3" = "$(cat "$GH_STATE/free")" ]; then
      printf '%s\n' "GraphQL: Could not resolve to a Repository with the name '$3'. (repository)" >&2
      exit 1
    fi
    if [ -e "$GH_STATE/broken" ] && [ "$3" = "$(cat "$GH_STATE/broken")" ]; then
      printf '%s\n' "HTTP 502: Bad Gateway (https://api.github.com/graphql)" >&2
      exit 1
    fi
    if [ -e "$GH_STATE/missing" ] && [ ! -e "$GH_STATE/created" ]; then
      printf '%s\n' "GraphQL: Could not resolve to a Repository with the name '$3'. (repository)" >&2
      exit 1
    fi
    # A `from to` line in redirects stands in for GitHub redirecting a former name.
    name=""
    if [ -e "$GH_STATE/redirects" ]; then
      while read -r from to; do [ "$from" = "$3" ] && name="$to"; done < "$GH_STATE/redirects"
    fi
    printf '{"nameWithOwner":"%s","isArchived":%s,"visibility":"%s"}\n' "${name:-$3}" "$(cat "$GH_STATE/archived")" "$(cat "$GH_STATE/visibility")"
    ;;
  "repo archive")
    if [ -e "$GH_STATE/archive-fails" ]; then
      printf '%s\n' "HTTP 403: Must have admin rights to Repository." >&2
      exit 1
    fi
    # Stands in for a slow GitHub until the test removes the hold file.
    while [ -e "$GH_STATE/hold" ]; do sleep 0.05; done
    printf 'true' > "$GH_STATE/archived"
    ;;
  "repo unarchive") printf 'false' > "$GH_STATE/archived" ;;
  "repo edit") printf '%s' "$5" | tr 'a-z' 'A-Z' > "$GH_STATE/visibility" ;;
  "repo create")
    # Stands in for --source DIR --remote origin --push.
    source=""; previous=""
    for arg in "$@"; do
      [ "$previous" = "--source" ] && source="$arg"
      previous="$arg"
    done
    if [ -e "$GH_STATE/push-fails" ]; then
      : > "$GH_STATE/created"
      printf '%s\n' "failed to run git: rejected push" >&2
      exit 1
    fi
    if [ -n "$source" ]; then
      git -C "$source" remote add origin "git@github.com:$3.git" || exit 1
      git -C "$source" push --quiet origin HEAD || exit 1
    fi
    ;;
  "api graphql")
    # The catalog pages, or none.
    if [ -e "$GH_STATE/catalog" ]; then cat "$GH_STATE/catalog"; else printf '[]\n'; fi
    ;;
  "api --hostname")
    printf 'HTTP/2.0 200 OK\r\n'
    [ -e "$GH_STATE/scopes" ] && printf 'X-Oauth-Scopes: %s\r\n' "$(cat "$GH_STATE/scopes")"
    printf '\r\n{"login":"org"}\n'
    ;;
  "repo delete")
    : > "$GH_STATE/deleted"
    # Stands in for work arriving in the checkout while GitHub deletes.
    if [ -e "$GH_STATE/dirty-on-delete" ]; then : > "$(cat "$GH_STATE/dirty-on-delete")"; fi
    ;;
  "repo rename")
    : > "$GH_STATE/renamed"
    # Stands in for something changing locally while GitHub renames.
    if [ -e "$GH_STATE/on-rename" ]; then eval "$(cat "$GH_STATE/on-rename")"; fi
    ;;
  *) printf 'unexpected gh call: %s\n' "$*" >&2; exit 1 ;;
esac
"##;

/// Puts the fake `gh` into `<dir>/bin` and its repository state into `<dir>/state`: not
/// archived, private. Returns both directories.
#[cfg(unix)]
#[allow(dead_code)]
pub fn install_fake_gh(dir: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    let state = dir.join("state");
    for directory in [&bin, &state] {
        std::fs::create_dir_all(directory).expect("create fake gh directory");
    }
    let gh = bin.join("gh");
    std::fs::write(&gh, FAKE_GH).expect("write fake gh");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
        .expect("make gh executable");
    std::fs::write(state.join("archived"), "false").expect("write fake gh state");
    std::fs::write(state.join("visibility"), "PRIVATE").expect("write fake gh state");
    (bin, state)
}

static TOOLS: OnceLock<tempfile::TempDir> = OnceLock::new();
static OUTBOUND_TRIPWIRE: OnceLock<OutboundTripwire> = OnceLock::new();

struct OutboundTripwire {
    proxy_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl OutboundTripwire {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind outbound tripwire");
        let proxy_url = format!(
            "http://{}",
            listener.local_addr().expect("tripwire address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut bytes = [0_u8; 4096];
                let length = stream.read(&mut bytes).unwrap_or(0);
                recorded
                    .lock()
                    .expect("tripwire requests")
                    .push(String::from_utf8_lossy(&bytes[..length]).into_owned());
                let _ = stream.write_all(
                    b"HTTP/1.1 502 Lager outbound network tripwire\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        Self {
            proxy_url,
            requests,
        }
    }
}

pub fn lager(home: &Path, config: &Path) -> Command {
    lager_with_path(home, config, None)
}

pub fn lager_with_path(home: &Path, config: &Path, extra_path: Option<&Path>) -> Command {
    let mut command = isolated_command(env!("CARGO_BIN_EXE_lager"), home, config, extra_path);
    command.args(["--config", config.to_str().expect("UTF-8 config path")]);
    command
}

#[allow(dead_code)]
pub fn lager_with_exact_path(home: &Path, config: &Path, path: &Path) -> Command {
    let mut command = lager(home, config);
    command.env("PATH", path);
    command
}

#[allow(dead_code)]
pub fn external(scope: &Path, program: impl AsRef<OsStr>) -> Command {
    let home = scope.join(".lager-command-home");
    let config = scope.join(".lager-command-config.toml");
    isolated_command(program, &home, &config, None)
}

fn isolated_command(
    program: impl AsRef<OsStr>,
    home: &Path,
    config: &Path,
    extra_path: Option<&Path>,
) -> Command {
    let cache = config
        .parent()
        .expect("config parent")
        .join("lager-test-cache");
    let xdg = config.parent().expect("config parent").join("xdg");
    let path = match extra_path {
        Some(extra) => std::env::join_paths([extra, controlled_tools()]).expect("test PATH"),
        None => controlled_tools().as_os_str().to_owned(),
    };
    let tripwire = OUTBOUND_TRIPWIRE.get_or_init(OutboundTripwire::start);
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("HOME", home)
        .env("LAGER_CONFIG", config)
        .env("LAGER_CACHE_DIR", cache)
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("PATH", path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", false_command())
        .env("SSH_ASKPASS", false_command())
        .env("GIT_SSH_COMMAND", false_command())
        .env("GH_PROMPT_DISABLED", "1")
        .env("HTTP_PROXY", &tripwire.proxy_url)
        .env("HTTPS_PROXY", &tripwire.proxy_url)
        .env("ALL_PROXY", &tripwire.proxy_url)
        .env("NO_PROXY", "127.0.0.1,localhost,::1")
        .env("http_proxy", &tripwire.proxy_url)
        .env("https_proxy", &tripwire.proxy_url)
        .env("all_proxy", &tripwire.proxy_url)
        .env("no_proxy", "127.0.0.1,localhost,::1");
    command
}

#[allow(dead_code)]
pub fn clear_outbound_requests() {
    OUTBOUND_TRIPWIRE
        .get_or_init(OutboundTripwire::start)
        .requests
        .lock()
        .expect("tripwire requests")
        .clear();
}

#[allow(dead_code)]
pub fn outbound_requests() -> Vec<String> {
    OUTBOUND_TRIPWIRE
        .get_or_init(OutboundTripwire::start)
        .requests
        .lock()
        .expect("tripwire requests")
        .clone()
}

#[allow(dead_code)]
pub fn real_tool(name: impl AsRef<OsStr>) -> PathBuf {
    controlled_tools().join(name.as_ref())
}

fn controlled_tools() -> &'static Path {
    TOOLS
        .get_or_init(|| {
            let directory = tempfile::tempdir().expect("controlled test tools");
            // Git's own shell helpers (not Lager) require this explicit utility set.
            for tool in [
                "git", "basename", "cat", "cut", "dirname", "expr", "grep", "mkdir", "sed", "tr",
                "uname",
            ] {
                if let Some(source) = find_tool(tool) {
                    link_tool(&source, &directory.path().join(tool));
                }
            }
            directory
        })
        .path()
}

fn find_tool(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(unix)]
fn link_tool(source: &Path, destination: &Path) {
    std::os::unix::fs::symlink(source, destination).expect("link controlled tool");
}

#[cfg(windows)]
fn link_tool(source: &Path, destination: &Path) {
    std::fs::copy(source, destination).expect("copy controlled tool");
}

#[cfg(unix)]
fn null_device() -> &'static str {
    "/dev/null"
}

#[cfg(windows)]
fn null_device() -> &'static str {
    "NUL"
}

#[cfg(unix)]
fn false_command() -> &'static str {
    "/usr/bin/false"
}

#[cfg(windows)]
fn false_command() -> &'static str {
    "cmd /c exit 1"
}
