//! Worktree rules for one repository: listing, placement, creation and guarded removal.
//! Inspection is separate from execution, so a caller shows what an operation does before
//! it runs, and execution verifies again what was shown.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::application::git_dir;
use crate::application::ports::{
    PathKind, WorktreeCommandError, WorktreeFault, WorktreeFilesystem, WorktreeGit, WorktreeLosses,
};

/// A repository with a non-bare primary checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub primary: PathBuf,
    pub common_dir: PathBuf,
}

/// Makes `path` absolute against `cwd` and resolves `.` and `..` by name.
pub fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    let mut absolute = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                absolute.pop();
            }
            other => absolute.push(other),
        }
    }
    absolute
}

/// Resolves the repository a checkout path belongs to; a linked worktree resolves to its
/// primary. With `exact`, `path` has to be the root of a working tree.
pub fn resolve_repository<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    path: &Path,
    exact: bool,
) -> Result<Repository, String> {
    let location = git
        .locate(path)
        .map_err(|_| format!("not a Git checkout: {}", path.display()))?;
    if exact && filesystem.canonicalize(path)? != location.top_level {
        return Err(format!("not the root of a checkout: {}", path.display()));
    }
    let entries = git
        .entries(&location.top_level)
        .map_err(|fault| fault.error)?;
    match entries.first() {
        Some(primary) if !primary.bare => Ok(Repository {
            primary: filesystem.canonicalize(&primary.path)?,
            common_dir: location.common_dir,
        }),
        _ => Err(format!(
            "repository has no primary checkout: {}",
            path.display()
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub repository_identity: Option<String>,
    pub primary_path: PathBuf,
    pub path: PathBuf,
    pub is_primary: bool,
    /// Full local ref; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Full object ID; `None` on an unborn branch.
    pub head: Option<String>,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
}

/// Records sorted by path text; errors sorted by path text, then code. Any error means the records
/// are incomplete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeListing {
    pub worktrees: Vec<WorktreeRecord>,
    pub errors: Vec<WorktreeFault>,
}

/// Lists the primary and every linked worktree Git knows, wherever it sits.
pub fn list<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    repository: &Repository,
) -> WorktreeListing {
    let mut errors = git.metadata_faults(&repository.common_dir);
    let fault = |path: &Path, code, error| WorktreeFault {
        path: Some(path.to_path_buf()),
        code,
        error,
    };
    let identity = git
        .origin_identity(&repository.primary)
        .unwrap_or_else(|error| {
            errors.push(fault(
                &repository.primary,
                "git_failed",
                format!("origin: {error}"),
            ));
            None
        });
    let mut worktrees = Vec::new();
    match git.entries(&repository.primary) {
        Err(fault) => errors.push(fault),
        Ok(entries) => {
            for (index, entry) in entries.into_iter().enumerate() {
                if let Err(error) = filesystem.kind(&entry.path) {
                    errors.push(fault(&entry.path, "unreadable", error));
                }
                worktrees.push(WorktreeRecord {
                    repository_identity: identity.clone(),
                    primary_path: repository.primary.clone(),
                    path: entry.path,
                    is_primary: index == 0,
                    branch: entry.branch,
                    head: entry.head,
                    locked: entry.locked,
                    lock_reason: entry.lock_reason,
                    prunable: entry.prunable,
                    prune_reason: entry.prune_reason,
                });
            }
        }
    }
    // Byte order of the path text, which is the order a reader of the JSON strings sees.
    worktrees.sort_by(|left, right| left.path.as_os_str().cmp(right.path.as_os_str()));
    errors.sort_by_key(|fault| (fault.path.clone().map(PathBuf::into_os_string), fault.code));
    WorktreeListing { worktrees, errors }
}

/// The local state a removal of a listed working tree would delete; `None` for a prunable
/// entry, whose directory is absent.
pub fn local_changes<G: WorktreeGit>(
    git: &G,
    record: &WorktreeRecord,
) -> Option<Result<WorktreeLosses, String>> {
    (!record.prunable).then(|| git.local_state(&record.path).map(|state| state.losses))
}

/// What removing a listed linked worktree deletes from its Git directory, by name; `None`
/// for the primary checkout and a prunable entry.
pub fn named_state_of<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    record: &WorktreeRecord,
) -> Option<Result<Vec<String>, String>> {
    (!record.is_primary && !record.prunable).then(|| {
        let location = git.locate(&record.path)?;
        let shown = record.path.display().to_string();
        named_state(git, filesystem, &record.path, &location.git_dir, &shown)
    })
}

/// The state in the linked Git directory `git_dir` of the worktree at `path` that its
/// removal deletes: entries beyond normal use, and the refs only this worktree sees.
fn named_state<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    path: &Path,
    git_dir: &Path,
    shown: &str,
) -> Result<Vec<String>, String> {
    let entries = filesystem.entries(git_dir)?;
    let mut state = git_dir::losses(
        &entries,
        true,
        &git_dir.display().to_string(),
        &format!("git -C {shown}"),
        |below| filesystem.holds(&git_dir.join(below)),
    );
    let refs = git
        .worktree_refs(path)
        .map_err(|error| format!("cannot read the refs of this worktree: {error}"))?;
    state.extend(
        refs.into_iter()
            .map(|name| format!("per-worktree ref {name}")),
    );
    Ok(state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchChoice<'a> {
    /// A new branch named after the worktree.
    Named,
    New(&'a str),
    Existing(&'a str),
}

#[derive(Debug, Clone, Copy)]
pub struct AddRequest<'a> {
    pub name: &'a str,
    pub branch: BranchChoice<'a>,
    /// The revision a new branch starts at; the primary's HEAD when absent.
    pub start: Option<&'a str>,
    /// An absolute destination that overrides the configured placement.
    pub path: Option<&'a Path>,
}

/// The configured `root` and `worktree_root`, resolved from HOME.
#[derive(Debug, Clone, Copy, Default)]
pub struct Placement<'a> {
    pub root: Option<&'a Path>,
    pub worktree_root: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// The request itself is invalid.
    Usage(String),
    /// The repository or the destination does not allow it.
    Refused(String),
}

/// A validated creation: where it goes and which branch it checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPlan {
    pub repository: Repository,
    pub destination: PathBuf,
    pub branch: String,
    /// The commit a new branch starts at; `None` checks out the existing `branch`.
    pub start: Option<String>,
}

fn valid_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// Checks what a creation request states on its own, before any repository is read.
pub fn check_request<G: WorktreeGit>(git: &G, request: &AddRequest<'_>) -> Result<(), String> {
    if !valid_name(request.name) {
        return Err(
            "NAME is one path component of letters, digits, `.`, `_` or `-`, starting with a letter or digit".to_owned(),
        );
    }
    let branch = match request.branch {
        BranchChoice::Existing(_) if request.start.is_some() => {
            return Err("--start applies only to a new branch".to_owned());
        }
        BranchChoice::Existing(_) => return Ok(()),
        BranchChoice::Named => request.name,
        BranchChoice::New(branch) => branch,
    };
    if !git.valid_branch_name(branch) {
        return Err(format!("invalid branch name: {branch}"));
    }
    Ok(())
}

/// The request a bare name stands for: the local branch of that name when it exists, else a
/// new branch of that name.
pub fn request_named<'a, G: WorktreeGit>(
    git: &G,
    repository: &Repository,
    name: &'a str,
) -> Result<AddRequest<'a>, String> {
    let named = AddRequest {
        name,
        branch: BranchChoice::Named,
        start: None,
        path: None,
    };
    check_request(git, &named)?;
    Ok(if git.branch_exists(&repository.primary, name)? {
        AddRequest {
            branch: BranchChoice::Existing(name),
            ..named
        }
    } else {
        named
    })
}

/// Validates a creation without creating anything.
pub fn plan_add<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    repository: &Repository,
    request: &AddRequest<'_>,
    placement: &Placement<'_>,
) -> Result<AddPlan, AddError> {
    let refused = AddError::Refused;
    check_request(git, request).map_err(AddError::Usage)?;
    let primary = &repository.primary;
    let (branch, new) = match request.branch {
        BranchChoice::Named => (request.name, true),
        BranchChoice::New(branch) => (branch, true),
        BranchChoice::Existing(branch) => (branch, false),
    };
    let destination = match (request.path, placement.worktree_root) {
        (Some(path), _) => path.to_path_buf(),
        (None, None) => derived(primary, request.name),
        (None, Some(worktree_root)) => {
            let outside = || {
                AddError::Usage(format!(
                    "{} is outside the configured root; worktree_root needs --path here",
                    primary.display()
                ))
            };
            let canonical = |path: &Path| filesystem.canonicalize(path).ok();
            let relative = placement
                .root
                .and_then(canonical)
                .and_then(|root| primary.strip_prefix(root).ok().map(Path::to_path_buf))
                .filter(|relative| !relative.as_os_str().is_empty())
                .ok_or_else(outside)?;
            derived(&worktree_root.join(relative), request.name)
        }
    };
    let exists = git.branch_exists(primary, branch).map_err(refused)?;
    let start = if new {
        if exists {
            return Err(refused(format!(
                "branch already exists: {branch}; --branch checks it out"
            )));
        }
        Some(match request.start {
            Some(revision) => git.commit(primary, revision).ok_or_else(|| {
                refused(format!("--start does not name a local commit: {revision}"))
            })?,
            None => git.commit(primary, "HEAD").ok_or_else(|| {
                refused("the primary checkout has no commit to branch from".to_owned())
            })?,
        })
    } else if exists {
        None
    } else {
        return Err(refused(format!("no local branch named {branch}")));
    };
    validate_destination(git, filesystem, primary, &destination).map_err(refused)?;
    Ok(AddPlan {
        repository: repository.clone(),
        destination,
        branch: branch.to_owned(),
        start,
    })
}

/// `<checkout>.worktrees/<name>`.
fn derived(checkout: &Path, name: &str) -> PathBuf {
    let mut container = OsString::from(checkout);
    container.push(".worktrees");
    PathBuf::from(container).join(name)
}

/// A destination is a missing path whose existing ancestors are plain directories outside
/// every checkout and Git directory.
fn validate_destination<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    primary: &Path,
    destination: &Path,
) -> Result<(), String> {
    if destination
        .components()
        .any(|component| component.as_os_str() == ".git")
    {
        return Err(format!(
            "destination is inside Git metadata: {}",
            destination.display()
        ));
    }
    if filesystem.kind(destination)? != PathKind::Missing {
        return Err(format!("destination exists: {}", destination.display()));
    }
    let mut ancestors = destination.ancestors().skip(1).collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        let shown = ancestor.display();
        match filesystem.kind(ancestor)? {
            PathKind::Missing => break,
            PathKind::Symlink => {
                return Err(format!("destination has a symlink ancestor: {shown}"));
            }
            PathKind::Other => {
                return Err(format!("destination ancestor is not a directory: {shown}"));
            }
            PathKind::Directory => {
                if filesystem.kind(&ancestor.join(".git"))? != PathKind::Missing {
                    return Err(format!("destination is inside the checkout {shown}"));
                }
                if filesystem.kind(&ancestor.join("HEAD"))? != PathKind::Missing
                    && filesystem.kind(&ancestor.join("objects"))? == PathKind::Directory
                {
                    return Err(format!("destination is inside Git metadata: {shown}"));
                }
            }
        }
    }
    let entries = git.entries(primary).map_err(|fault| fault.error)?;
    if entries.iter().any(|entry| entry.path == destination) {
        return Err(format!(
            "destination is still registered as a worktree: {}; clear it with: git -C {} worktree prune",
            destination.display(),
            primary.display()
        ));
    }
    Ok(())
}

/// A creation that did not finish. Nothing is rolled back; `remaining` names what is left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddFailure {
    pub message: String,
    pub cancelled: bool,
    pub remaining: Vec<String>,
}

/// Creates the planned worktree through `git worktree add`. The destination is validated
/// again first; missing parent directories are created only after that.
pub fn add<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    plan: &AddPlan,
) -> Result<(), AddFailure> {
    let primary = &plan.repository.primary;
    let failure = |message, cancelled, remaining| AddFailure {
        message,
        cancelled,
        remaining,
    };
    validate_destination(git, filesystem, primary, &plan.destination)
        .map_err(|error| failure(error, false, Vec::new()))?;
    let mut created = plan
        .destination
        .ancestors()
        .skip(1)
        .take_while(|ancestor| filesystem.kind(ancestor) == Ok(PathKind::Missing))
        .collect::<Vec<_>>();
    created.reverse();
    let remaining = || {
        let mut remaining = created
            .iter()
            .copied()
            .chain([plan.destination.as_path()])
            .filter(|path| filesystem.kind(path) != Ok(PathKind::Missing))
            .map(|path| format!("directory {}", path.display()))
            .collect::<Vec<_>>();
        if plan.start.is_some() && git.branch_exists(primary, &plan.branch) != Ok(false) {
            remaining.push(format!("branch {}", plan.branch));
        }
        if git.entries(primary).map_or(true, |entries| {
            entries.iter().any(|entry| entry.path == plan.destination)
        }) {
            remaining.push(format!(
                "worktree metadata for {}",
                plan.destination.display()
            ));
        }
        remaining
    };
    if let Some(parent) = plan.destination.parent() {
        filesystem.create_dir_all(parent).map_err(|error| {
            failure(
                format!("could not create {}: {error}", parent.display()),
                false,
                remaining(),
            )
        })?;
    }
    git.add(
        primary,
        &plan.destination,
        &plan.branch,
        plan.start.as_deref(),
    )
    .map_err(|error| failure(error.message, error.cancelled, remaining()))
}

/// A reason a removal does not run. `--force` waives none of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub message: String,
    /// The native Git command that resolves it.
    pub hint: Option<String>,
}

/// One linked worktree as the removal guards saw it. A confirmation binds to this value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalInspection {
    /// The exact path; canonical once it is known to exist.
    pub path: PathBuf,
    pub primary: Option<PathBuf>,
    /// Full `refs/heads/` ref of the checked-out branch, which the removal keeps; `None`
    /// when HEAD is detached or names any other ref.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The shared ref that keeps the HEAD commit when no branch is checked out.
    pub protected_by: Option<String>,
    /// Commits on the branch that no remote ref holds.
    pub unpushed: Option<usize>,
    pub losses: WorktreeLosses,
    /// State in the worktree's Git directory that the removal deletes, by name.
    pub git_state: Vec<String>,
    pub blockers: Vec<Blocker>,
    /// Identity of the worktree directory, its Git directory and the common directory.
    ownership: String,
}

impl RemovalInspection {
    /// True when the removal deletes local state.
    pub fn lossy(&self) -> bool {
        !self.losses.is_empty() || !self.git_state.is_empty()
    }

    /// True when the only obstacle is local state that `--force` accepts.
    pub fn force_helps(&self) -> bool {
        self.blockers.is_empty() && self.lossy()
    }
}

pub fn short_id(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

/// Runs every removal guard on one exact path and changes nothing. `context` is a checkout
/// of the repository expected to own the path; it names the owner of an absent path.
pub fn inspect_removal<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    path: &Path,
    context: Option<&Path>,
) -> RemovalInspection {
    let mut inspection = RemovalInspection {
        path: path.to_path_buf(),
        ..RemovalInspection::default()
    };
    if let Err(blocker) = inspect(git, filesystem, context, &mut inspection) {
        inspection.blockers.push(blocker);
    }
    inspection
}

fn blocker(message: String, hint: Option<String>) -> Blocker {
    Blocker { message, hint }
}

fn inspect<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    context: Option<&Path>,
    inspection: &mut RemovalInspection,
) -> Result<(), Blocker> {
    let requested = inspection.path.clone();
    let shown = requested.display().to_string();
    let plain = |message: String| blocker(message, None);
    match filesystem
        .kind(&requested)
        .map_err(|error| plain(format!("cannot read {shown}: {error}")))?
    {
        PathKind::Directory => {}
        PathKind::Missing => return Err(absent(git, filesystem, context, &requested)),
        PathKind::Symlink => return Err(plain(format!("{shown} is a symlink, not a worktree"))),
        PathKind::Other => return Err(plain(format!("{shown} is not a Git worktree"))),
    }
    let path = filesystem
        .canonicalize(&requested)
        .map_err(|error| plain(format!("cannot read {shown}: {error}")))?;
    let location = git
        .locate(&path)
        .map_err(|_| plain(format!("{shown} is not a Git worktree")))?;
    if location.top_level != path {
        return Err(plain(format!(
            "{shown} is inside the worktree {}, not a worktree itself",
            location.top_level.display()
        )));
    }
    let entries = git
        .entries(&path)
        .map_err(|fault| plain(format!("cannot list worktrees: {}", fault.error)))?;
    let primary = entries.first().map(|entry| entry.path.clone());
    inspection.path = path.clone();
    inspection.primary = primary.clone();
    let primary = primary.unwrap_or_default();
    if location.git_dir == location.common_dir {
        let identity = git.origin_identity(&path).ok().flatten();
        return Err(blocker(
            format!("{shown} is the primary checkout, not a linked worktree"),
            identity.map(|identity| format!("lager remove {identity}")),
        ));
    }
    // Both directions: the worktree names its Git directory under the common directory, and
    // that Git directory names this exact worktree back.
    let entry = entries
        .iter()
        .skip(1)
        .find(|entry| filesystem.canonicalize(&entry.path).as_ref() == Ok(&path))
        .filter(|_| location.git_dir.parent() == Some(&location.common_dir.join("worktrees")))
        .filter(|_| git.linked_path(&location.git_dir).as_ref() == Ok(&path))
        .ok_or_else(|| {
            // No repair command: it would bind the path to whatever its `.git` file names.
            plain(format!(
                "{shown} and the Git directory its .git file names, {}, do not name each other",
                location.git_dir.display()
            ))
        })?;
    let identity = |directory: &Path| {
        filesystem
            .identity(directory)
            .map_err(|error| plain(format!("cannot read {}: {error}", directory.display())))
    };
    inspection.ownership = format!(
        "{} {} {}",
        identity(&path)?,
        identity(&location.git_dir)?,
        location.common_dir.display()
    );
    // Only a local branch outlives the worktree; any other ref HEAD names keeps nothing.
    inspection.branch = entry
        .branch
        .clone()
        .filter(|branch| branch.starts_with("refs/heads/"));
    inspection.head = entry.head.clone();
    let blockers = &mut inspection.blockers;
    if entry.locked {
        blockers.push(blocker(
            match &entry.lock_reason {
                Some(reason) if !reason.is_empty() => format!("worktree is locked: {reason}"),
                _ => "worktree is locked".to_owned(),
            },
            Some(format!(
                "git -C {} worktree unlock {shown}",
                primary.display()
            )),
        ));
    }
    match (&inspection.branch, &entry.head) {
        (Some(branch), _) => inspection.unpushed = git.unpushed(&primary, branch),
        (None, Some(head)) => match git.protecting_ref(&primary, head) {
            Ok(Some(protecting)) => inspection.protected_by = Some(protecting),
            proof => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| valid_name(name))
                    .unwrap_or(short_id(head));
                blockers.push(blocker(
                    match proof {
                        Err(error) => format!(
                            "cannot prove that detached HEAD {} is kept by a branch, tag or remote ref: {error}",
                            short_id(head)
                        ),
                        _ => format!(
                            "detached HEAD {} is not reachable from a branch, tag or remote ref",
                            short_id(head)
                        ),
                    },
                    Some(format!(
                        "keep the commit first: git -C {shown} branch keep-{name}"
                    )),
                ));
            }
        },
        (None, None) => blockers.push(plain("HEAD is unknown".to_owned())),
    }
    match git.populated_submodules(&path) {
        Err(error) => blockers.push(plain(format!("submodule state is unknown: {error}"))),
        Ok(populated) => {
            let modules = location.git_dir.join("modules");
            if populated.is_empty() && filesystem.kind(&modules) != Ok(PathKind::Missing) {
                blockers.push(plain(format!(
                    "Git keeps submodule repositories of this worktree in {}",
                    modules.display()
                )));
            }
            for submodule in populated {
                blockers.push(blocker(
                    format!("initialized submodule {submodule}"),
                    Some(format!("git -C {shown} submodule deinit {submodule}")),
                ));
            }
        }
    }
    // Git keeps the changes it set aside for a rebase, merge or checkout only in these files.
    for (file, holder, hint) in [
        (
            "rebase-merge/autostash",
            "a stopped rebase",
            format!("git -C {shown} rebase --abort"),
        ),
        (
            "rebase-apply/autostash",
            "a stopped rebase",
            format!("git -C {shown} rebase --abort"),
        ),
        (
            "MERGE_AUTOSTASH",
            "a stopped merge",
            format!("git -C {shown} merge --abort"),
        ),
        (
            "CHECKOUT_AUTOSTASH_HEAD",
            "an interrupted checkout",
            format!(
                "apply the commit that file names, then delete the file: git -C {shown} stash apply COMMIT"
            ),
        ),
    ] {
        let autostash = location.git_dir.join(file);
        if filesystem.kind(&autostash) != Ok(PathKind::Missing) {
            blockers.push(blocker(
                format!("{holder} holds an autostash in {}", autostash.display()),
                Some(hint),
            ));
        }
    }
    // The canonical path, so a second inspection from the confirmed path names the same.
    match named_state(
        git,
        filesystem,
        &path,
        &location.git_dir,
        &path.display().to_string(),
    ) {
        Ok(state) => inspection.git_state = state,
        Err(error) => blockers.push(plain(error)),
    }
    match git.local_state(&path) {
        Ok(state) => {
            inspection.losses = state.losses;
            for repository in state.nested_repositories {
                blockers.push(plain(format!("nested repository {repository}")));
            }
        }
        Err(error) => blockers.push(plain(format!("local state is unknown: {error}"))),
    }
    Ok(())
}

fn absent<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    context: Option<&Path>,
    path: &Path,
) -> Blocker {
    let resolved = path
        .parent()
        .zip(path.file_name())
        .and_then(|(parent, name)| Some(filesystem.canonicalize(parent).ok()?.join(name)));
    let listed = context
        .and_then(|context| git.entries(context).ok())
        .and_then(|entries| {
            let primary = entries.first()?.path.clone();
            entries
                .into_iter()
                .skip(1)
                .find(|entry| entry.path == path || Some(&entry.path) == resolved.as_ref())
                .map(|entry| (primary, entry.prunable))
        });
    match listed {
        Some((primary, prunable)) => blocker(
            format!(
                "path is absent; Git still lists this worktree{}",
                if prunable { " as prunable" } else { "" }
            ),
            Some(format!("git -C {} worktree prune", primary.display())),
        ),
        None => blocker(
            format!("path is absent: {}", path.display()),
            Some("if Git still lists it, run inside its repository: git worktree prune".to_owned()),
        ),
    }
}

/// What a removal that Git did not finish left behind, read after it returned.
pub fn removal_remains<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    confirmed: &RemovalInspection,
) -> Vec<String> {
    let path = &confirmed.path;
    let mut remaining = Vec::new();
    if filesystem.kind(path) != Ok(PathKind::Missing) {
        remaining.push(format!("directory {}", path.display()));
    }
    let listed = confirmed
        .primary
        .as_deref()
        .map(|primary| git.entries(primary));
    if !matches!(&listed, Some(Ok(entries)) if entries.iter().all(|entry| entry.path != *path)) {
        remaining.push(format!("worktree metadata for {}", path.display()));
    }
    remaining
}

/// Why a confirmed removal did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    /// The worktree no longer is what was confirmed; nothing was removed.
    Stale,
    Blocked(Vec<Blocker>),
    /// Local state would be lost and was not accepted.
    Losses,
    Failed(WorktreeCommandError),
}

/// Removes the confirmed worktree through `git worktree remove` and keeps its branch.
/// Every guard runs again first, and the result has to equal `confirmed`. Git gets
/// `--force` only when `accept_losses` is set and the inspection lists losses.
pub fn remove<G: WorktreeGit, F: WorktreeFilesystem>(
    git: &G,
    filesystem: &F,
    confirmed: &RemovalInspection,
    accept_losses: bool,
    context: Option<&Path>,
) -> Result<(), RemoveError> {
    let current = inspect_removal(git, filesystem, &confirmed.path, context);
    if current != *confirmed {
        return Err(RemoveError::Stale);
    }
    if !current.blockers.is_empty() {
        return Err(RemoveError::Blocked(current.blockers));
    }
    let lossy = current.lossy();
    if lossy && !accept_losses {
        return Err(RemoveError::Losses);
    }
    let primary = current.primary.as_deref().ok_or(RemoveError::Stale)?;
    git.remove(primary, &current.path, lossy)
        .map_err(RemoveError::Failed)
}
