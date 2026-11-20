//! Rex as a session host, through its `rex` CLI. Windows are tabs and blocks are panes.

use std::env;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::application::ports::{HostError, HostSession, HostTab, SessionHost};
use crate::domain::session::{Layout, Panes};

/// Creates the session, attaches to it so its blocks can be disowned and its focus window
/// set, and returns the raw `session.create` result.
const CREATE_SCRIPT: &str = r#"local function call(method, args)
  local result, err = rex.call(method, args)
  if not result then
    error(method .. ": " .. tostring(err))
  end
  return result
end

local request = rex.args
local created = call("session.create", { label = request.label, initial_windows = request.initial_windows })
local session_id = created.session_id
call("session.attach", { session_id = session_id })
call("session.disown_block", { session_id = session_id })
local focus = created.initial_windows[request.focus]
if focus then
  call("session.focus_window", { session_id = session_id, window_id = focus.window_id })
end
return created
"#;

const SHELL: &str = "com.superlogical.terminal.shell";

/// Rex answers within its own `--timeout`; lager kills a `rex` that outlives this.
const KILL_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Clone, Copy)]
pub struct Rex;

impl SessionHost for Rex {
    fn live(&self) -> Result<Vec<String>, HostError> {
        let listed = call("session.list", None).map_err(|failure| failure.error)?;
        // Any other reply would read as "no sessions" and make the caller forget them all.
        listed["sessions"]
            .as_array()
            .and_then(|sessions| {
                sessions
                    .iter()
                    .map(|session| session["session_id"].as_str().map(str::to_owned))
                    .collect()
            })
            .ok_or_else(|| HostError::Failed("rex listed no session ids".to_owned()))
    }

    fn create(
        &self,
        label: &str,
        directory: &Path,
        layout: &Layout,
    ) -> Result<HostSession, HostError> {
        let cwd = directory.to_string_lossy();
        let windows: Vec<Value> = layout
            .tabs
            .iter()
            .map(|tab| json!({ "window_label": tab.name, "layout": node(&tab.panes, &cwd) }))
            .collect();
        let focus = layout.tabs.iter().position(|tab| tab.focus);
        let arguments = json!({
            "label": label,
            "initial_windows": windows,
            "focus": focus.map(|index| index + 1),
        })
        .to_string();
        let created = run(
            &["do", "-", "--json", "--args", &arguments],
            Some(CREATE_SCRIPT),
        )
        .map_err(|failure| failure.error)?;
        let id = created["session_id"]
            .as_str()
            .ok_or_else(|| HostError::Failed("rex created no session".to_owned()))?;
        // Rex lists a window's blocks in layout order, each split's first area ahead of its
        // second.
        let tabs = layout
            .tabs
            .iter()
            .zip(created["initial_windows"].as_array().into_iter().flatten())
            .map(|(tab, window)| HostTab {
                name: tab.name.clone(),
                panes: tab
                    .panes
                    .names()
                    .into_iter()
                    .zip(window["block_ids"].as_array().into_iter().flatten())
                    .filter_map(|(name, id)| Some((name.to_owned(), id.as_str()?.to_owned())))
                    .collect(),
            })
            .collect();
        Ok(HostSession {
            id: id.to_owned(),
            tabs,
        })
    }

    fn show(&self, id: &str) -> Result<(), HostError> {
        let clients = call("client.list", Some(json!({ "kinds": ["app"] })))
            .map_err(|failure| failure.error)?;
        // shortcut: compares `connected_at` as RFC 3339 text, which orders correctly only while
        // all app clients share one UTC offset; parse it to an instant once that matters.
        let client = clients["clients"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|client| client["connection_state"].as_str() != Some("lost"))
            .max_by_key(|client| client["connected_at"].as_str().unwrap_or_default())
            .and_then(|client| client["client_id"].as_str())
            .ok_or_else(|| HostError::Failed("no Rex app connected".to_owned()))?;
        call(
            "client.perform",
            Some(json!({
                "client_id": client,
                "action": "session.select",
                "args": { "session_id": id },
            })),
        )
        .map(|_| ())
        .map_err(|failure| failure.error)
    }

    fn close(&self, id: &str) -> Result<(), HostError> {
        match call("session.destroy", Some(json!({ "session_id": id }))) {
            Ok(_) => Ok(()),
            Err(Failure {
                exit: Some(3),
                error: HostError::Failed(reason),
            }) if reason.contains("code=session_not_found") => Ok(()),
            Err(failure) => Err(failure.error),
        }
    }
}

fn node(panes: &Panes, cwd: &str) -> Value {
    let (direction, ratio, first, second) = match panes {
        Panes::Pane(name) => {
            return json!({
                "block": { "flavor": SHELL, "options": { "cwd": cwd }, "label": name }
            });
        }
        Panes::Row(ratio, first, second) => ("horizontal", ratio, first, second),
        Panes::Column(ratio, first, second) => ("vertical", ratio, first, second),
    };
    json!({
        "split": {
            "direction": direction,
            "ratio": ratio,
            "before": node(first, cwd),
            "after": node(second, cwd),
        }
    })
}

struct Failure {
    exit: Option<i32>,
    error: HostError,
}

fn call(method: &str, arguments: Option<Value>) -> Result<Value, Failure> {
    let arguments = arguments.map(|arguments| arguments.to_string());
    let mut args = vec!["api", "call", method];
    args.extend(arguments.as_deref());
    run(&args, None)
}

/// The first `rex` on `PATH`, else a Homebrew install, else the one in `REX_BIN_DIR`, else the
/// one inside the Rex server app. Rex launched from the Dock gives its programs a short `PATH`
/// and no `REX_BIN_DIR`.
fn program() -> PathBuf {
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path)
        .map(|directory| directory.join("rex"))
        .chain(["/opt/homebrew/bin/rex", "/usr/local/bin/rex"].map(PathBuf::from))
        .chain(env::var_os("REX_BIN_DIR").map(|directory| PathBuf::from(directory).join("rex")))
        .chain(env::var_os("HOME").map(|home| PathBuf::from(home).join(SERVER_APP_REX)))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("rex"))
}

const SERVER_APP_REX: &str = "Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex";

/// Runs `rex` without starting a server. Exit 4 means no server answered.
fn run(args: &[&str], stdin: Option<&str>) -> Result<Value, Failure> {
    let failed = |exit, reason: String| Failure {
        exit,
        error: HostError::Failed(reason),
    };
    let mut child = Command::new(program())
        .args(["--autostart=false", "--timeout", "8s"])
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => Failure {
                exit: None,
                error: HostError::Unavailable("rex CLI not found".to_owned()),
            },
            _ => failed(None, format!("rex: {error}")),
        })?;
    // A rex that exits before reading its script breaks the pipe; its exit status says why.
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(input.as_bytes());
    }
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let output = match receiver.recv_timeout(KILL_AFTER) {
        Ok(output) => output.map_err(|error| failed(None, format!("rex: {error}")))?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
            #[cfg(not(unix))]
            let _ = pid;
            return Err(failed(
                None,
                format!("rex did not answer within {} s", KILL_AFTER.as_secs()),
            ));
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr.trim().lines().next().unwrap_or_default();
        let reason = line.strip_prefix("Error: ").unwrap_or(line).to_owned();
        let reason = if reason.is_empty() {
            format!("rex exited with {}", output.status)
        } else {
            reason
        };
        let exit = output.status.code();
        return Err(if exit == Some(4) {
            Failure {
                exit,
                error: HostError::Unavailable(reason),
            }
        } else {
            failed(exit, reason)
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&stdout).map_err(|error| failed(None, format!("rex: {error}")))
}
