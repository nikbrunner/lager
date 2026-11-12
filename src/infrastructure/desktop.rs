use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::application::ports::{Desktop, ToolInspector};
use crate::infrastructure::tools::NativeTools;

const CLIPBOARD: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
    &[("pbcopy", &[])]
} else if cfg!(windows) {
    &[("clip", &[])]
} else {
    &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ]
};

/// How long the inventory waits for the clipboard in total, across every utility it tries.
const CLIPBOARD_LIMIT: Duration = Duration::from_secs(2);

const OPENER: Option<&str> = if cfg!(target_os = "macos") {
    Some("open")
} else if cfg!(windows) {
    None
} else {
    Some("xdg-open")
};

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeDesktop;

impl Desktop for NativeDesktop {
    fn copy(&self, value: &OsStr) -> Result<(), String> {
        let deadline = Instant::now() + CLIPBOARD_LIMIT;
        let mut failures = Vec::new();
        for (program, args) in CLIPBOARD
            .iter()
            .filter(|(program, _)| NativeTools.command_available(program))
        {
            if Instant::now() >= deadline {
                failures.push(format!("{program} skipped: time limit reached"));
                continue;
            }
            match copy_with(program, args, value, deadline) {
                Ok(()) => return Ok(()),
                Err(error) => failures.push(error),
            }
        }
        if failures.is_empty() {
            let names = CLIPBOARD.iter().map(|(program, _)| *program);
            return Err(format!(
                "no clipboard utility found ({})",
                names.collect::<Vec<_>>().join(", ")
            ));
        }
        Err(failures.join("; "))
    }

    /// The opener may stay in the foreground until the browser exits, so it is reaped on a
    /// detached thread and never waited for here.
    fn open(&self, url: &str) -> Result<(), String> {
        let Some(opener) = OPENER else {
            return Err("no URL opener is available on this platform".to_owned());
        };
        if !NativeTools.command_available(opener) {
            return Err(format!("no URL opener found ({opener})"));
        }
        let mut child = Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{opener}: {error}"))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

fn copy_with(program: &str, args: &[&str], value: &OsStr, deadline: Instant) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    let mut stdin = child.stdin.take().expect("clipboard stdin is piped");
    let bytes = value.as_encoded_bytes().to_vec();
    // A utility that stops reading must not block the caller; the writer is never joined
    // unless it has finished.
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} did not finish within {} s",
                    CLIPBOARD_LIMIT.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program}: {error}"));
            }
        }
    };
    while !writer.is_finished() {
        if Instant::now() >= deadline {
            return Err(format!("{program} exited before reading the value"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let written = writer
        .join()
        .map_err(|_| format!("{program}: writer panicked"))?;
    written.map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
