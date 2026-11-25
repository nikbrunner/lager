use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "lager",
    version = env!("LAGER_VERSION"),
    about = "Declarative local Git repository manager",
    after_help = "Examples:\n  lager register github.com/org/project\n  lager add github.com/org/project --register\n  lager remove github.com/org/project --unregister --yes --force"
)]
pub struct Args {
    #[arg(
        long,
        global = true,
        env = "LAGER_CONFIG",
        value_name = "PATH",
        help = "Use PATH instead of the default configuration file"
    )]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Create a new portable configuration file")]
    Init(InitArgs),
    #[command(about = "Declare repositories for management")]
    Register(RepositoryArgs),
    #[command(about = "Remove repository declarations without deleting local data")]
    Unregister(UnregisterArgs),
    #[command(about = "Create local checkouts and optionally declare them")]
    Add(CloneArgs),
    #[command(about = "Permanently delete guarded local checkouts")]
    Remove(RemoveArgs),
    #[command(about = "Create missing declared checkouts sequentially")]
    Ensure(EnsureArgs),
    #[command(about = "Run configured hooks for explicit repositories")]
    Hook(HookArgs),
    #[command(
        about = "Rename a GitHub repository with its origin, checkout and declaration",
        after_help = "Example:\n  lager rename github.com/org/project project-cli --yes"
    )]
    Rename(RenameArgs),
    #[command(about = "Archive a GitHub repository, or unarchive it with --undo")]
    Archive(ArchiveArgs),
    #[command(about = "Make a GitHub repository public or private")]
    Visibility(VisibilityArgs),
    #[command(
        visible_aliases = ["list", "ls"],
        about = "Print the inventory rows as a table or one JSON document"
    )]
    Status(StatusArgs),
    #[command(name = "tui", about = "Open the repository inventory")]
    Inventory(InventoryArgs),
    #[command(
        subcommand,
        about = "List, create and remove worktrees of one repository"
    )]
    Worktree(WorktreeCommand),
    #[command(
        subcommand,
        about = "Open, create, close and list terminal sessions of checkouts"
    )]
    Session(SessionCommand),
    #[command(
        about = "Print the usage guide for agents, or install the agent skill",
        after_help = "Examples:\n  lager skill\n  lager skill install"
    )]
    Skill(SkillArgs),
}

#[derive(Debug, ClapArgs)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: Option<SkillCommand>,
}

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    #[command(about = "Write the skill to $HOME/.agents/skills/lager, replacing existing files")]
    Install,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    #[command(
        about = "Show the checkout's session, creating it first when there is none",
        after_help = "Examples:\n  lager session open org/project\n  lager session open .\n  lager session open org/project hotfix"
    )]
    Open(SessionPickArgs),
    #[command(about = "Create the checkout's session without showing it")]
    Create(SessionCreateArgs),
    #[command(about = "Close the checkout's session")]
    Close(SessionPickArgs),
    #[command(about = "List the checkouts that have a session")]
    List(SessionListArgs),
}

#[derive(Debug, ClapArgs)]
pub struct SessionArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Checkout path (absolute, or starting with . or ..) or repository reference"
    )]
    pub repository: String,
    #[arg(
        value_name = "NAME",
        help = "Directory name of a linked worktree of the repository"
    )]
    pub name: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct SessionPickArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Checkout path (absolute, or starting with . or ..) or repository reference; omit to pick one"
    )]
    pub repository: Option<String>,
    #[arg(
        value_name = "NAME",
        help = "Directory name of a linked worktree of the repository"
    )]
    pub name: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct SessionCreateArgs {
    #[command(flatten)]
    pub checkout: SessionArgs,
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}

#[derive(Debug, ClapArgs)]
pub struct SessionListArgs {
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum WorktreeCommand {
    #[command(about = "List the primary checkout and every linked worktree")]
    List(WorktreeListArgs),
    #[command(
        about = "Create a linked worktree through git worktree add",
        after_help = "Examples:\n  lager worktree add org/project hotfix\n  lager worktree add . review --branch feature/login\n  lager worktree add . --branch origin/feat/session-tabs\n  lager worktree add . spike --new-branch dev/spike --start v2.3.0 --path ../spike"
    )]
    Add(WorktreeAddArgs),
    #[command(
        about = "Remove one linked worktree through git worktree remove; its branch stays",
        after_help = "--force accepts the listed losses only (modified, untracked and ignored files and named Git state); ownership, lock, primary-checkout, detached-commit and submodule checks remain active."
    )]
    Remove(WorktreeRemoveArgs),
}

#[derive(Debug, ClapArgs)]
pub struct WorktreeListArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Checkout path (absolute, or starting with . or ..) or repository reference; omit for the current repository"
    )]
    pub repository: Option<String>,
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}

#[derive(Debug, ClapArgs)]
pub struct WorktreeAddArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Checkout path (absolute, or starting with . or ..) or repository reference"
    )]
    pub repository: String,
    #[arg(
        value_name = "NAME",
        required_unless_present = "branch",
        help = "Directory name, and the new branch name unless a branch option is given; with --branch it defaults to the last segment of the local branch name"
    )]
    pub name: Option<String>,
    #[arg(
        long,
        value_name = "EXISTING",
        conflicts_with = "new_branch",
        help = "Check out a local branch, or a remote branch such as origin/feat/x as a new local branch that tracks it"
    )]
    pub branch: Option<String>,
    #[arg(long = "new-branch", value_name = "NEW", help = "Name the new branch")]
    pub new_branch: Option<String>,
    #[arg(
        long,
        value_name = "REF",
        help = "Local commit the new branch starts at; defaults to the primary checkout's HEAD"
    )]
    pub start: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Destination, absolute or relative to the working directory; overrides worktree_path"
    )]
    pub path: Option<PathBuf>,
    #[arg(
        long,
        help = "Fetch every remote first, so --branch finds branches pushed since the last fetch; a failing remote is a warning"
    )]
    pub fetch: bool,
    #[arg(
        long = "open-session",
        conflicts_with = "create_session",
        help = "Open the new worktree's session and show it"
    )]
    pub open_session: bool,
    #[arg(
        long = "create-session",
        help = "Create the new worktree's session without showing it"
    )]
    pub create_session: bool,
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}

#[derive(Debug, ClapArgs)]
pub struct WorktreeRemoveArgs {
    #[arg(value_name = "PATH", help = "Exact path of the linked worktree")]
    pub path: PathBuf,
    #[arg(long, help = "Confirm removal without an interactive prompt")]
    pub yes: bool,
    #[arg(long, help = "Accept the listed losses; safety checks still apply")]
    pub force: bool,
    #[arg(
        long = "close-session",
        help = "Close the worktree's session once it is removed"
    )]
    pub close_session: bool,
}

#[derive(Debug, ClapArgs)]
pub struct InitArgs {
    #[arg(
        long,
        value_name = "ROOT",
        help = "Portable repository root under HOME"
    )]
    pub root: Option<String>,
    #[arg(
        long,
        conflicts_with = "no_create_root",
        help = "Create ROOT when it does not exist"
    )]
    pub create_root: bool,
    #[arg(
        long = "no-create-root",
        conflicts_with = "create_root",
        help = "Require ROOT to already exist"
    )]
    pub no_create_root: bool,
    #[arg(
        long,
        conflicts_with = "no_github",
        help = "Configure GitHub repository discovery"
    )]
    pub github: bool,
    #[arg(
        long = "no-github",
        conflicts_with = "github",
        help = "Do not configure GitHub discovery"
    )]
    pub no_github: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct RepositoryArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Repository references; omit to choose interactively"
    )]
    pub repositories: Vec<String>,
    #[arg(
        long,
        value_name = "CMD",
        help = "Store a hook command with each declaration"
    )]
    pub post_clone: Option<String>,
    #[arg(long, help = "Include archived provider repositories")]
    pub include_archived: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct UnregisterArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Repository references; omit to choose interactively"
    )]
    pub repositories: Vec<String>,
    #[arg(long, help = "Include archived provider repositories")]
    pub include_archived: bool,
}

#[derive(Debug, ClapArgs)]
#[command(
    after_help = "Examples:\n  lager add github.com/org/project --register\n  lager add github.com/org/project --no-register\n  lager add github.com/org/project --post-clone 'make setup'\n  lager add github.com/org/new --create --register\n  lager add github.com/org/new --create --from ./new --register"
)]
pub struct CloneArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Repository references; omit to choose interactively"
    )]
    pub repositories: Vec<String>,
    #[arg(
        long,
        conflicts_with = "no_register",
        help = "Declare each successfully added repository"
    )]
    pub register: bool,
    #[arg(
        long = "no-register",
        conflicts_with = "register",
        help = "Do not declare added repositories"
    )]
    pub no_register: bool,
    #[arg(
        long,
        value_name = "CMD",
        conflicts_with = "no_register",
        help = "Run CMD after a fresh add; implies --register"
    )]
    pub post_clone: Option<String>,
    #[arg(long, help = "Include archived provider repositories")]
    pub include_archived: bool,
    #[arg(
        long,
        help = "Create the repository on GitHub first; private unless --public"
    )]
    pub create: bool,
    #[arg(long, requires = "create", help = "Create the repository as public")]
    pub public: bool,
    #[arg(
        long,
        value_name = "TEXT",
        requires = "create",
        help = "Description of the created repository"
    )]
    pub description: Option<String>,
    #[arg(
        long,
        value_name = "DIR",
        requires = "create",
        help = "Publish the local repository in DIR and move it into root"
    )]
    pub from: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
#[command(
    after_help = "Examples:\n  lager remove github.com/org/project --unregister --yes --force\n  lager remove github.com/org/project --remote --yes\n\n--force bypasses local-state warnings only; root, symlink, and origin checks remain active."
)]
pub struct RemoveArgs {
    #[arg(
        value_name = "REPOSITORY",
        help = "Repository references; omit to choose local checkouts"
    )]
    pub repositories: Vec<String>,
    #[arg(
        long,
        conflicts_with = "keep_registered",
        help = "Remove declarations after successful disk removal"
    )]
    pub unregister: bool,
    #[arg(
        long = "keep-registered",
        conflicts_with = "unregister",
        help = "Keep declarations after disk removal"
    )]
    pub keep_registered: bool,
    #[arg(long, help = "Confirm removal without an interactive prompt")]
    pub yes: bool,
    #[arg(long, help = "Bypass local-state warnings; safety checks still apply")]
    pub force: bool,
    #[arg(
        long,
        conflicts_with = "keep_registered",
        help = "Delete the repository on GitHub too; implies --unregister"
    )]
    pub remote: bool,
}

#[derive(Debug, ClapArgs)]
pub struct EnsureArgs {
    #[arg(long, help = "Include archived provider repositories")]
    pub include_archived: bool,
}

#[derive(Debug, ClapArgs)]
pub struct HookArgs {
    #[arg(value_name = "REPOSITORY", help = "Explicit repository references")]
    pub repositories: Vec<String>,
}

#[derive(Debug, ClapArgs)]
pub struct RenameArgs {
    #[arg(value_name = "REPOSITORY", help = "Repository reference")]
    pub repository: String,
    #[arg(value_name = "NEW_NAME", help = "New repository name; the owner stays")]
    pub new_name: String,
    #[arg(long, help = "Rename without an interactive prompt")]
    pub yes: bool,
}

#[derive(Debug, ClapArgs)]
pub struct ArchiveArgs {
    #[arg(value_name = "REPOSITORY", help = "Repository reference")]
    pub repository: String,
    #[arg(long, help = "Unarchive instead")]
    pub undo: bool,
}

#[derive(Debug, ClapArgs)]
pub struct VisibilityArgs {
    #[arg(value_name = "REPOSITORY", help = "Repository reference")]
    pub repository: String,
    #[arg(value_name = "VISIBILITY")]
    pub value: VisibilityValue,
    #[arg(long, help = "Change the visibility without an interactive prompt")]
    pub yes: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum VisibilityValue {
    Public,
    Private,
}

#[derive(Debug, ClapArgs)]
pub struct StatusArgs {
    #[arg(long, help = "Query providers for expanded wildcard members")]
    pub remote: bool,
    #[arg(long, help = "Include archived repositories; requires --remote")]
    pub include_archived: bool,
    #[arg(long, help = "Show only declared repositories and wildcard patterns")]
    pub registered: bool,
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct InventoryArgs {
    #[arg(
        long,
        help = "Start with remote discovery on and fetch the configured providers"
    )]
    pub remote: bool,
    #[arg(long, help = "Include archived repositories; requires --remote")]
    pub include_archived: bool,
}
