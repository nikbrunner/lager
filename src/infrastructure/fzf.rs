use std::collections::{HashMap, VecDeque};
#[cfg(test)]
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::application::ports::{RepositorySelector, SelectionCandidate, SelectionError};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FzfError {
    #[error("fzf is required for interactive repository selection; install fzf and try again")]
    Missing,
    #[error("repository selection was cancelled")]
    Cancelled,
    #[error("could not start fzf: {0}")]
    Start(String),
    #[error("fzf failed with status {0}")]
    Failed(i32),
}

#[derive(Debug, Clone)]
pub struct Fzf {
    executable: PathBuf,
    #[cfg(test)]
    arguments: Vec<OsString>,
}

impl Default for Fzf {
    fn default() -> Self {
        Self::new("fzf")
    }
}

impl Fzf {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            #[cfg(test)]
            arguments: Vec::new(),
        }
    }

    #[cfg(test)]
    fn with_arguments(executable: impl Into<PathBuf>, arguments: &[&str]) -> Self {
        Self {
            executable: executable.into(),
            arguments: arguments.iter().map(OsString::from).collect(),
        }
    }

    fn pick(&self, candidates: &[SelectionCandidate]) -> Result<Vec<SelectionCandidate>, FzfError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut command = Command::new(&self.executable);
        #[cfg(test)]
        command.args(&self.arguments);
        let mut child = command
            .args(["--multi"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    FzfError::Missing
                } else {
                    FzfError::Start(error.to_string())
                }
            })?;
        let mut write_error = None;
        {
            let stdin = child.stdin.as_mut().expect("fzf stdin configured");
            for candidate in candidates {
                if let Err(error) = writeln!(stdin, "{}", candidate.display) {
                    write_error = Some(error);
                    break;
                }
            }
        }
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .map_err(|error| FzfError::Start(error.to_string()))?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(130);
            return if code == 1 || code == 130 {
                Err(FzfError::Cancelled)
            } else {
                Err(FzfError::Failed(code))
            };
        }
        if let Some(error) = write_error {
            return Err(FzfError::Start(error.to_string()));
        }

        // Display strings are only picker labels. Keep a queue per label so duplicate
        // labels map to distinct candidates without interpreting label contents.
        let mut indexes: HashMap<&str, VecDeque<usize>> = HashMap::new();
        for (index, candidate) in candidates.iter().enumerate() {
            indexes
                .entry(candidate.display.as_str())
                .or_default()
                .push_back(index);
        }
        let mut selected = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(queue) = indexes.get_mut(line)
                && let Some(index) = queue.pop_front()
            {
                selected.push(candidates[index].clone());
            }
        }
        Ok(selected)
    }
}

impl RepositorySelector for Fzf {
    fn select(
        &self,
        candidates: &[SelectionCandidate],
    ) -> Result<Vec<SelectionCandidate>, SelectionError> {
        self.pick(candidates).map_err(|error| match error {
            FzfError::Cancelled => SelectionError::Cancelled,
            FzfError::Missing => SelectionError::Unavailable(
                "fzf is required for interactive repository selection; install fzf and try again"
                    .to_owned(),
            ),
            FzfError::Start(message) => SelectionError::Failed(message),
            FzfError::Failed(code) => {
                SelectionError::Failed(format!("fzf failed with status {code}"))
            }
        })
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::domain::repository::RepositoryRef;

    fn candidate(reference: &str, display: &str) -> SelectionCandidate {
        SelectionCandidate {
            reference: RepositoryRef::parse(reference).unwrap(),
            display: display.to_owned(),
            archived: false,
            exact_path: None,
        }
    }

    fn script(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fzf-shim");
        fs::write(&path, format!("#!/bin/sh\n{contents}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (directory, path)
    }

    #[test]
    fn maps_one_newline_selection_without_parsing_display_as_identity() {
        let (_directory, path) = script("read line; printf '%s\\n' \"$line\"");
        let selector = Fzf::new(path);
        let selected = selector
            .select(&[candidate("git@host:team/repo.git", "team/repo label")])
            .unwrap();
        assert_eq!(selected[0].reference.clone_url, "git@host:team/repo.git");
    }

    #[test]
    fn preserves_multiple_selection_order_and_duplicate_safe_mapping() {
        let selector = Fzf::with_arguments(
            "/bin/sh",
            &["-c", "test \"$1\" = \"--multi\" && cat", "fzf-shim"],
        );
        let selected = selector
            .select(&[
                candidate("org/first", "duplicate"),
                candidate("org/second", "duplicate"),
            ])
            .unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|candidate| candidate.reference.identity())
                .collect::<Vec<_>>(),
            ["github.com/org/first", "github.com/org/second"]
        );
    }

    #[test]
    fn reports_missing_fzf_only_when_candidates_need_selection() {
        let selector = Fzf::new("missing-fzf");
        assert_eq!(
            selector.select(&[]).unwrap(),
            Vec::<SelectionCandidate>::new()
        );
        assert!(matches!(
            selector.select(&[candidate("org/repo", "repo")]),
            Err(SelectionError::Unavailable(message)) if message.contains("install fzf")
        ));
    }

    #[test]
    fn maps_exit_130_to_cancellation() {
        let (_directory, path) = script("exit 130");
        let selector = Fzf::new(path);
        assert_eq!(
            selector.select(&[candidate("org/repo", "repo")]),
            Err(SelectionError::Cancelled)
        );
    }

    #[test]
    fn maps_exit_130_to_cancellation_when_fzf_closes_a_full_input_pipe() {
        let (_directory, path) = script("exec 0<&-; exit 130");
        let selector = Fzf::new(path);
        let display = "x".repeat(16 * 1024);
        let candidates = (0..256)
            .map(|index| candidate(&format!("org/repo-{index}"), &display))
            .collect::<Vec<_>>();

        assert_eq!(selector.select(&candidates), Err(SelectionError::Cancelled));
    }
}
