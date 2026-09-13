use std::path::Path;
use std::process::Command;

use crate::application::ports::HookRunner;

#[derive(Debug, Default, Clone, Copy)]
pub struct Shell;

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("could not start /bin/sh: {0}")]
    Start(#[source] std::io::Error),
    #[error("hook exited with status {0}")]
    Failed(std::process::ExitStatus),
}

pub fn run(command: &str, directory: &Path) -> Result<(), ShellError> {
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(directory)
        .status()
        .map_err(ShellError::Start)?;
    if status.success() {
        Ok(())
    } else {
        Err(ShellError::Failed(status))
    }
}

impl HookRunner for Shell {
    type Error = ShellError;

    fn run_hook(&self, command: &str, directory: &Path) -> Result<(), Self::Error> {
        run(command, directory)
    }
}
