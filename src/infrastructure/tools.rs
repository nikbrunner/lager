use std::path::Path;
use std::process::{Command, Stdio};

use crate::application::ports::ToolInspector;

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeTools;

impl ToolInspector for NativeTools {
    fn command_available(&self, name: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| executable(&directory.join(name)))
        })
    }

    fn github_authenticated(&self) -> bool {
        self.command_available("gh")
            && Command::new("gh")
                .args(["auth", "status"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
    }
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}
