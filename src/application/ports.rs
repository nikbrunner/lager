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
    /// Tells the user what a command is about to do without asking.
    fn notice(&mut self, _message: &str) {}
    /// Asks `question` below `heading` and one line per item.
    fn confirm_list(
        &mut self,
        heading: &str,
        items: &[String],
        question: &str,
        default: bool,
    ) -> Result<bool, InteractionError> {
        let items: String = items.iter().map(|item| format!("\n  - {item}")).collect();
        self.confirm(&format!("{heading}{items}\n{question}"), default)
    }
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

/// Optional platform utilities. Errors explain why the utility was unavailable or failed.
pub trait Desktop {
    /// Puts `value` on the clipboard unchanged.
    fn copy(&self, value: &std::ffi::OsStr) -> Result<(), String>;
    /// Opens an already validated HTTPS address.
    fn open(&self, url: &str) -> Result<(), String>;
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

/// One reference and its independently chosen post-clone hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    pub reference: String,
    pub post_clone: Option<String>,
}

/// Registration that commits all requests or leaves the configuration unchanged.
/// Deliberately separate from `ConfigStore`: there is no incremental fallback.
pub trait AtomicRegistrationStore: ConfigStore {
    fn register_batch(
        &self,
        path: &Path,
        requests: &[RegistrationRequest],
    ) -> Result<Vec<Mutation>, Self::Error>;
}

/// Rewrites the declarations of a repository whose name changed.
pub trait RenamingStore: ConfigStore {
    /// Rewrites an explicit entry's `url` in place and renames an `exclude` entry of a covering
    /// wildcard; `old` and `new` are repository references.
    fn rename(&self, path: &Path, old: &str, new: &str) -> Result<Mutation, Self::Error>;
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

    /// Classify a native interruption before its error is reduced to display text.
    fn is_cancelled(&self, _error: &Self::Error) -> bool {
        false
    }

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
    /// What tells this directory from another one put at the same path later, such as its
    /// device and inode. An empty answer means the filesystem cannot tell.
    fn directory_identity(&self, _path: &Path) -> Result<String, Self::Error> {
        Ok(String::new())
    }
    fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error>;
    fn discover_checkouts(&self, root: &Path) -> Result<Vec<PathBuf>, Self::Error>;
}

pub trait HookRunner {
    type Error: Display;

    /// Classify a native interruption before its error is reduced to display text.
    fn is_cancelled(&self, _error: &Self::Error) -> bool {
        false
    }

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

/// What one provider returned: the repositories it listed, and what it could not list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderAnswer {
    pub members: Vec<crate::domain::repository::RepositorySummary>,
    /// Set when the catalog or a wildcard failed; `members` holds what the rest returned.
    pub error: Option<String>,
}

/// The providers remote discovery reads, one blocking request per provider.
pub trait ProviderSource: Send + Sync + 'static {
    /// The configured providers, by host.
    fn hosts(&self) -> Vec<String>;

    /// What a request to `host` is built from besides its wildcards; a change makes earlier
    /// answers stale.
    fn settings(&self, _host: &str) -> String {
        String::new()
    }

    /// The provider's whole catalog and the members of `wildcards`, which all name `host`.
    /// Blocks until the provider answered; `cancelled` ends the request early where the
    /// provider can be interrupted.
    fn fetch(
        &self,
        host: &str,
        wildcards: &[crate::domain::repository::RepositoryRef],
        include_archived: bool,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> ProviderAnswer;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
    /// Visible to an enterprise's members; lager never sets it.
    Internal,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteState {
    /// `owner/name` exactly as the provider spells it.
    pub name: String,
    pub archived: bool,
    pub visibility: Visibility,
}

/// Changes to repositories on their provider. Every call blocks until the provider answered.
pub trait RemoteRepositories {
    /// Whether the provider's CLI is installed and logged in for `host`; the error names the fix.
    fn check(&self, host: &str) -> Result<(), String>;
    /// `None` when the provider has no repository at `reference`, also when the path only
    /// redirects to a repository under another name.
    fn state(&self, reference: &RepositoryRef) -> Result<Option<RemoteState>, String>;
    fn archive(&self, reference: &RepositoryRef, archived: bool) -> Result<(), String>;
    fn set_visibility(
        &self,
        reference: &RepositoryRef,
        visibility: Visibility,
    ) -> Result<(), String>;
    fn create(
        &self,
        reference: &RepositoryRef,
        visibility: Visibility,
        description: Option<&str>,
    ) -> Result<(), String>;
    /// Creates `reference` from the local repository at `source`, sets it as origin and pushes.
    fn publish(
        &self,
        reference: &RepositoryRef,
        visibility: Visibility,
        description: Option<&str>,
        source: &Path,
    ) -> Result<(), String>;
    /// Whether the login for `host` may delete repositories; the error names the fix.
    fn check_delete_scope(&self, host: &str) -> Result<(), String>;
    fn delete(&self, reference: &RepositoryRef) -> Result<(), String>;
    /// Renames `reference` to `name` under the same owner.
    fn rename(&self, reference: &RepositoryRef, name: &str) -> Result<(), String>;
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

/// Where a path sits in its repository, as canonical paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLocation {
    pub top_level: PathBuf,
    /// The Git directory of this working tree; equals `common_dir` in a primary checkout.
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// One working tree as `git worktree list` reports it. The first entry is the primary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Full object ID; `None` on an unborn branch.
    pub head: Option<String>,
    /// Full ref; `None` when HEAD is detached.
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
}

/// `git_failed`, `unreadable` or `invalid_metadata`, with a readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeFault {
    pub path: Option<PathBuf>,
    pub code: &'static str,
    pub error: String,
}

/// Local state a removal deletes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorktreeLosses {
    pub modified: usize,
    pub untracked: usize,
    pub ignored: bool,
}

impl WorktreeLosses {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What a removal deletes from one working tree, and what it must not delete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeState {
    pub losses: WorktreeLosses,
    /// Present files Git hides from its status, already counted in `losses.modified`.
    pub hidden: usize,
    /// Whether status shows changes in the index, and in the work tree.
    pub staged: bool,
    pub unstaged: bool,
    /// Untracked or ignored directories that hold a Git repository of their own.
    pub nested_repositories: Vec<String>,
}

/// A native Git mutation that did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCommandError {
    pub message: String,
    /// The child reported an interruption.
    pub cancelled: bool,
}

/// Git facts and native worktree commands. `repository` is any working tree of the
/// repository; every call runs without inherited Git location variables.
pub trait WorktreeGit {
    fn locate(&self, path: &Path) -> Result<WorktreeLocation, String>;
    fn entries(&self, repository: &Path) -> Result<Vec<WorktreeEntry>, WorktreeFault>;
    /// Entries under `<common_dir>/worktrees` that Git cannot read and so leaves out of its list.
    fn metadata_faults(&self, common_dir: &Path) -> Vec<WorktreeFault>;
    /// Normalized origin; `None` without an origin or with an unsupported one.
    fn origin_identity(&self, repository: &Path) -> Result<Option<String>, String>;
    /// The commit `revision` names in the local object store.
    fn commit(&self, repository: &Path, revision: &str) -> Option<String>;
    /// The names of the repository's remotes.
    fn remotes(&self, repository: &Path) -> Result<Vec<String>, String>;
    /// The origin URL as written, before `url.<base>.insteadOf` rewrites; `None` without an
    /// origin.
    fn origin_url(&self, repository: &Path) -> Result<Option<String>, String>;
    fn set_origin_url(&self, repository: &Path, url: &str) -> Result<(), String>;
    fn branch_exists(&self, repository: &Path, name: &str) -> Result<bool, String>;
    fn valid_branch_name(&self, name: &str) -> bool;
    /// `git worktree add`, with `-b branch start` when `start` is given. Native streams.
    fn add(
        &self,
        repository: &Path,
        destination: &Path,
        branch: &str,
        start: Option<&str>,
    ) -> Result<(), WorktreeCommandError>;
    /// The working tree path the linked Git directory `git_dir` points back to.
    fn linked_path(&self, git_dir: &Path) -> Result<PathBuf, String>;
    /// Counts files Git hides from its status (skip-worktree, assume-unchanged) as modified.
    fn local_state(&self, worktree: &Path) -> Result<WorktreeState, String>;
    /// Submodule paths that hold a repository in this working tree.
    fn populated_submodules(&self, worktree: &Path) -> Result<Vec<String>, String>;
    /// The refs only this worktree sees: `refs/worktree`, `refs/bisect`, `refs/rewritten`.
    fn worktree_refs(&self, worktree: &Path) -> Result<Vec<String>, String>;
    /// A shared branch, tag or remote ref whose commit has `commit` as an ancestor.
    fn protecting_ref(&self, repository: &Path, commit: &str) -> Result<Option<String>, String>;
    /// Commits on `branch` that no remote ref holds; `None` without remote refs.
    fn unpushed(&self, repository: &Path, branch: &str) -> Option<usize>;
    /// `git worktree remove`, with `--force` only when asked. Native streams.
    fn remove(
        &self,
        repository: &Path,
        worktree: &Path,
        force: bool,
    ) -> Result<(), WorktreeCommandError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Missing,
    Directory,
    Symlink,
    Other,
}

pub trait WorktreeFilesystem {
    /// What sits at `path` itself, without following a symlink.
    fn kind(&self, path: &Path) -> Result<PathKind, String>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String>;
    fn create_dir_all(&self, path: &Path) -> Result<(), String>;
    /// The names directly inside `directory`.
    fn entries(&self, directory: &Path) -> Result<Vec<String>, String>;
    /// `None` when nothing is at `path`, otherwise whether it holds anything: false only
    /// for an empty file or an empty directory, true when it cannot be read.
    fn holds(&self, path: &Path) -> Option<bool>;
    /// What tells this directory from another one put at the same path later.
    fn identity(&self, path: &Path) -> Result<String, String>;
    /// The device the file system holding `path` lives on, following symlinks.
    fn device(&self, path: &Path) -> Result<u64, String>;
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
}
