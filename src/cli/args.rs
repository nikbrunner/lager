use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};

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
    #[command(about = "Show declarations and local repository state")]
    List(ListArgs),
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
    after_help = "Examples:\n  lager add github.com/org/project --register\n  lager add github.com/org/project --no-register\n  lager add github.com/org/project --post-clone 'make setup'"
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
}

#[derive(Debug, ClapArgs)]
#[command(
    after_help = "Example:\n  lager remove github.com/org/project --unregister --yes --force\n\n--force bypasses local-state warnings only; root, symlink, and origin checks remain active."
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
pub struct ListArgs {
    #[arg(long, help = "Query providers for expanded wildcard members")]
    pub remote: bool,
    #[arg(long, help = "Include archived repositories; requires --remote")]
    pub include_archived: bool,
    #[arg(long, help = "Render one stable JSON document")]
    pub json: bool,
}
