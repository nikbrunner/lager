use std::collections::HashMap;
#[cfg(test)]
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::application::ports::{RepositorySelector, SelectionCandidate, SelectionError};
use crate::presentation::escape;

// Exact-checkout labels have renderer-owned columns. Never interpret the public
// display string as a row: library-supplied labels without paths are opaque fields.
fn label(candidate: &SelectionCandidate) -> String {
    match &candidate.exact_path {
        Some(path) => format!(
            "{}\t{}",
            escape(candidate.reference.identity()),
            escape(path.display())
        ),
        None => escape(&candidate.display),
    }
}

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
        let rows: Vec<_> = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| format!("{index}\t{}", label(candidate)))
            .collect();
        let mut command = Command::new(&self.executable);
        #[cfg(test)]
        command.args(&self.arguments);
        let mut child = command
            // with-nth transforms both display and search; applying nth again
            // would drop the first visible field (the whole label for most rows).
            .args(["--multi", "--delimiter=\t", "--with-nth=2.."])
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
            for row in &rows {
                if let Err(error) = writeln!(stdin, "{row}") {
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

        // Hidden row IDs, never rendered labels, identify untouched candidates.
        let mut indexes: HashMap<String, usize> = (0..candidates.len())
            .map(|index| (index.to_string(), index))
            .collect();
        let mut selected = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((id, _)) = line.split_once('\t')
                && let Some(index) = indexes.remove(id)
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

    fn script(contents: &str) -> (tempfile::TempDir, Fzf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fzf-shim");
        fs::write(&path, format!("#!/bin/sh\n{contents}\n")).unwrap();
        // Forked test children can retain writable script descriptors until exec.
        let mut selector = Fzf::new("/bin/sh");
        selector.arguments.push(path.into_os_string());
        (directory, selector)
    }

    #[test]
    fn maps_one_newline_selection_without_parsing_display_as_identity() {
        let (_directory, selector) = script("read line; printf '%s\\n' \"$line\"");
        let selected = selector
            .select(&[candidate("git@host:team/repo.git", "team/repo label")])
            .unwrap();
        assert_eq!(selected[0].reference.clone_url, "git@host:team/repo.git");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fixture_selection_works_while_a_script_writer_is_open() {
        let (directory, selector) = script("read line; printf '%s\\n' \"$line\"");
        let _writer = fs::OpenOptions::new()
            .write(true)
            .open(directory.path().join("fzf-shim"))
            .unwrap();
        let selected = selector.select(&[candidate("org/repo", "repo")]).unwrap();
        assert_eq!(selected[0].reference.identity(), "github.com/org/repo");
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
    fn selecting_only_second_identical_label_preserves_identity() {
        let (_directory, selector) = script("sed -n '2p'");
        let candidates = [
            candidate("org/first", "duplicate"),
            candidate("org/second", "duplicate"),
        ];
        assert_eq!(
            selector.select(&candidates).unwrap(),
            [candidates[1].clone()]
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
        let (_directory, selector) = script("exit 130");
        assert_eq!(
            selector.select(&[candidate("org/repo", "repo")]),
            Err(SelectionError::Cancelled)
        );
    }

    #[test]
    fn maps_exit_130_to_cancellation_when_fzf_closes_a_full_input_pipe() {
        let (_directory, selector) = script("exec 0<&-; exit 130");
        let display = "x".repeat(16 * 1024);
        let candidates = (0..256)
            .map(|index| candidate(&format!("org/repo-{index}"), &display))
            .collect::<Vec<_>>();

        assert_eq!(selector.select(&candidates), Err(SelectionError::Cancelled));
    }
}
