use std::fmt::Display;
use std::path::{Path, PathBuf};

use crate::domain::config::Config;
use crate::domain::repository::RepositoryRef;
use crate::domain::state::LocalState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    Changed,
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionError {
    Cancelled,
    Failed(String),
}

pub trait Interaction {
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, InteractionError>;
    fn input(
        &mut self,
        message: &str,
        placeholder: &str,
        default: Option<&str>,
    ) -> Result<String, InteractionError>;
}

pub trait ToolInspector {
    fn command_available(&self, name: &str) -> bool;
    fn github_authenticated(&self) -> bool;
}

pub trait ConfigurationFilesystem {
    type Error: Display;

    fn create_dir_all(&self, path: &Path) -> Result<(), Self::Error>;
}

pub trait ConfigStore {
    type Error: Display;

    fn exists(&self, path: &Path) -> bool;
    fn load(&self, path: &Path) -> Result<Config, Self::Error>;
    fn init(&self, path: &Path, root: &str, github: bool) -> Result<(), Self::Error>;
    fn register(
        &self,
        path: &Path,
        references: &[String],
        post_clone: Option<&str>,
    ) -> Result<Vec<Mutation>, Self::Error>;
    fn remove(&self, path: &Path, references: &[String]) -> Result<Vec<Mutation>, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureEvent<'a> {
    CloneStarted {
        reference: &'a str,
        destination: &'a Path,
    },
    CloneSucceeded {
        reference: &'a str,
    },
    CloneFailed {
        reference: &'a str,
        error: &'a str,
    },
}

pub trait EnsureReporter {
    fn report(&mut self, event: EnsureEvent<'_>);
}

pub trait GitClient {
    type Error: Display;

    fn clone_repository(
        &self,
        url: &str,
        home: &Path,
        root: &Path,
        destination: &Path,
    ) -> Result<(), Self::Error>;
}

pub trait GitState {
    fn classify_destination(&self, destination: &Path, expected_url: &str) -> LocalState;
}

pub trait GitRemoval {
    fn inspect_removal(
        &self,
        destination: &Path,
        expected_url: &str,
    ) -> Result<Vec<String>, String>;
    fn origin_identity(&self, destination: &Path) -> Result<String, String>;
}

pub trait RemovalFilesystem {
    type Error: Display;

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error>;
    fn is_symlink(&self, path: &Path) -> Result<bool, Self::Error>;
    fn exists(&self, path: &Path) -> bool;
    /// Returns false only when metadata reports NotFound; all other failures are errors.
    fn metadata_exists(&self, path: &Path) -> Result<bool, Self::Error> {
        Ok(self.exists(path))
    }
    fn contains_symlink(&self, path: &Path) -> Result<bool, Self::Error>;
    fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error>;
    fn discover_directories(&self, root: &Path) -> Result<Vec<PathBuf>, Self::Error>;
}

pub trait HookRunner {
    type Error: Display;

    fn run_hook(&self, command: &str, directory: &Path) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionCandidate {
    pub reference: RepositoryRef,
    pub display: String,
    pub archived: bool,
    /// Canonical discovered path retained only for local removal selections.
    pub exact_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    Cancelled,
    Unavailable(String),
    Failed(String),
}

pub trait RepositorySelector {
    fn select(
        &self,
        candidates: &[SelectionCandidate],
    ) -> Result<Vec<SelectionCandidate>, SelectionError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub provider: String,
    pub error: String,
}

pub trait ProviderCatalog {
    fn catalog(
        &self,
        include_archived: bool,
    ) -> Result<Vec<crate::domain::repository::RepositorySummary>, String>;
    fn expand(
        &self,
        reference: &crate::domain::repository::RepositoryRef,
        include_archived: bool,
    ) -> Result<Vec<crate::domain::repository::RepositorySummary>, String>;

    fn catalog_with_failures(
        &self,
        include_archived: bool,
    ) -> (
        Vec<crate::domain::repository::RepositorySummary>,
        Vec<ProviderFailure>,
    ) {
        match self.catalog(include_archived) {
            Ok(repositories) => (repositories, Vec::new()),
            Err(error) => (
                Vec::new(),
                vec![ProviderFailure {
                    provider: "catalog".to_owned(),
                    error,
                }],
            ),
        }
    }
}

pub fn config_path(override_path: Option<&Path>) -> PathBuf {
    if let Some(path) = override_path {
        return path.to_path_buf();
    }
    if let Ok(path) = std::env::var("LAGER_CONFIG") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/lager/config.toml")
}
