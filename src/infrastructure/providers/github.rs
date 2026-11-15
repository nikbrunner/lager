use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use serde::Deserialize;

use crate::application::ports::{RemoteState, Visibility};
use crate::domain::config::ProviderConfig;
use crate::domain::repository::{RepositoryRef, RepositorySummary};
use crate::infrastructure::inventory::{ProbeError, command_output};

const PAGE_QUERY: &str = "query($endCursor:String){viewer{repositories(first:100,after:$endCursor){nodes{name isArchived isPrivate sshUrl owner{login}} pageInfo{hasNextPage endCursor}}}}";

#[derive(Debug, Clone)]
pub struct Github {
    pub host: String,
    pub config: ProviderConfig,
    gh_executable: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ViewerData {
    viewer: Viewer,
}

#[derive(Debug, Deserialize)]
struct Viewer {
    repositories: Connection<Repository>,
}

#[derive(Debug, Deserialize)]
struct OrganizationData {
    organization: Option<Organization>,
}

#[derive(Debug, Deserialize)]
struct Organization {
    repositories: Connection<Repository>,
}

#[derive(Debug, Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViewedRepository {
    #[serde(rename = "isArchived")]
    is_archived: bool,
    visibility: String,
}

#[derive(Debug, Deserialize)]
struct Repository {
    #[serde(rename = "isArchived")]
    is_archived: bool,
    #[serde(rename = "sshUrl")]
    ssh_url: String,
}

impl Github {
    pub fn new(host: String, config: ProviderConfig) -> Self {
        Self {
            host,
            config,
            gh_executable: PathBuf::from("gh"),
        }
    }

    #[cfg(test)]
    fn with_executable(host: String, config: ProviderConfig, executable: PathBuf) -> Self {
        Self {
            host,
            config,
            gh_executable: executable,
        }
    }

    pub fn catalog(&self, include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
        self.catalog_cancellable(include_archived, &AtomicBool::new(false))
    }

    /// `catalog`, ended early once `cancelled` is set.
    pub fn catalog_cancellable(
        &self,
        include_archived: bool,
        cancelled: &AtomicBool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let pages: Vec<Page<ViewerData>> = self.query(PAGE_QUERY, cancelled)?;
        let mut result = Vec::new();
        for page in pages {
            let Some(data) = page.data else { continue };
            let PageInfo {
                has_next_page,
                end_cursor,
            } = data.viewer.repositories.page_info;
            let _ = (has_next_page, end_cursor);
            for repository in data.viewer.repositories.nodes {
                if !include_archived && repository.is_archived {
                    continue;
                }
                result.push(summary(&self.host, &self.config, repository)?);
            }
        }
        Ok(result)
    }

    pub fn expand(
        &self,
        reference: &RepositoryRef,
        include_archived: bool,
    ) -> Result<Vec<RepositorySummary>, String> {
        self.expand_cancellable(reference, include_archived, &AtomicBool::new(false))
    }

    /// `expand`, ended early once `cancelled` is set.
    pub fn expand_cancellable(
        &self,
        reference: &RepositoryRef,
        include_archived: bool,
        cancelled: &AtomicBool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let organization = reference
            .path
            .split('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "GitHub wildcard requires an organization".to_owned())?;
        let query = format!(
            "query($endCursor:String){{organization(login:{organization:?}){{repositories(first:100,after:$endCursor){{nodes{{name isArchived isPrivate sshUrl owner{{login}}}} pageInfo{{hasNextPage endCursor}}}}}}}}"
        );
        let pages: Vec<Page<OrganizationData>> = self.query(&query, cancelled)?;
        let mut result = Vec::new();
        for page in pages {
            let Some(data) = page.data else { continue };
            let Some(organization) = data.organization else {
                continue;
            };
            let PageInfo {
                has_next_page,
                end_cursor,
            } = organization.repositories.page_info;
            let _ = (has_next_page, end_cursor);
            for repository in organization.repositories.nodes {
                if !include_archived && repository.is_archived {
                    continue;
                }
                result.push(summary(&self.host, &self.config, repository)?);
            }
        }
        Ok(result)
    }

    /// Runs `gh` in its own process group; `cancelled` ends it and every descendant.
    fn query<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<T>, String> {
        let mut command = Command::new(&self.gh_executable);
        command.args(["api"]);
        if self.host != "github.com" {
            command.args(["--hostname", &self.host]);
        }
        command.args(["graphql", "--paginate", "--slurp", "-f"]);
        command.arg(format!("query={query}"));
        command.stdin(Stdio::null());
        let output = command_output(command, cancelled).map_err(gh_error)?;
        serde_json::from_str(&output)
            .map_err(|error| format!("could not parse gh response: {error}"))
    }

    /// `gh` with `GH_HOST` naming this provider, which every `gh repo` subcommand honours.
    /// It stays in lager's process group, so Ctrl-C at the terminal ends it too.
    fn gh(&self, args: &[&str]) -> Result<String, ProbeError> {
        let output = Command::new(&self.gh_executable)
            .args(args)
            .env("GH_HOST", &self.host)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| ProbeError::Spawn(error.kind(), error.to_string()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(ProbeError::Exit {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    pub fn check(&self) -> Result<(), String> {
        let host = self.host.as_str();
        match self.gh(&["auth", "status", "--hostname", host]) {
            Ok(_) => Ok(()),
            Err(ProbeError::Exit { stderr, .. }) => Err(format!(
                "gh auth status failed for {host}: {stderr}; if not logged in, run `gh auth login --hostname {host}`"
            )),
            Err(error) => Err(gh_error(error)),
        }
    }

    pub fn state(&self, path: &str) -> Result<Option<RemoteState>, String> {
        let output = match self.gh(&["repo", "view", path, "--json", "isArchived,visibility"]) {
            Ok(output) => output,
            Err(ProbeError::Exit { stderr, .. })
                if stderr.contains("Could not resolve to a Repository") =>
            {
                return Ok(None);
            }
            Err(error) => return Err(gh_error(error)),
        };
        let viewed: ViewedRepository = serde_json::from_str(&output)
            .map_err(|error| format!("could not parse gh response: {error}"))?;
        let visibility = match viewed.visibility.to_ascii_lowercase().as_str() {
            "public" => Visibility::Public,
            "private" => Visibility::Private,
            "internal" => Visibility::Internal,
            other => return Err(format!("gh reported unknown visibility `{other}`")),
        };
        Ok(Some(RemoteState {
            archived: viewed.is_archived,
            visibility,
        }))
    }

    pub fn archive(&self, path: &str, archived: bool) -> Result<(), String> {
        let verb = if archived { "archive" } else { "unarchive" };
        self.gh(&["repo", verb, path, "--yes"])
            .map(drop)
            .map_err(gh_error)
    }

    pub fn set_visibility(&self, path: &str, visibility: Visibility) -> Result<(), String> {
        self.gh(&[
            "repo",
            "edit",
            path,
            "--visibility",
            visibility.as_str(),
            "--accept-visibility-change-consequences",
        ])
        .map(drop)
        .map_err(gh_error)
    }

    pub fn create(
        &self,
        path: &str,
        visibility: Visibility,
        description: Option<&str>,
    ) -> Result<(), String> {
        let visibility = format!("--{}", visibility.as_str());
        // One argument, so a description starting with `-` stays a value.
        let description = description.map(|text| format!("--description={text}"));
        let mut args = vec!["repo", "create", path, &visibility];
        args.extend(description.as_deref());
        self.gh(&args).map(drop).map_err(gh_error)
    }
}

fn gh_error(error: ProbeError) -> String {
    match error {
        ProbeError::Spawn(std::io::ErrorKind::NotFound, _) => {
            "GitHub provider requires `gh`; install GitHub CLI and run `gh auth login`".to_owned()
        }
        ProbeError::Spawn(_, error) => format!("could not start gh: {error}"),
        ProbeError::Exit { stderr, .. } => stderr,
        other => other.to_string(),
    }
}

fn summary(
    host: &str,
    config: &ProviderConfig,
    repository: Repository,
) -> Result<RepositorySummary, String> {
    let path = RepositoryRef::parse(&repository.ssh_url)
        .map(|reference| reference.path)
        .map_err(|error| error.to_string())?;
    let reference =
        RepositoryRef::parse(&ssh_url(host, config, &path)).map_err(|error| error.to_string())?;
    Ok(RepositorySummary {
        reference,
        archived: repository.is_archived,
    })
}

fn ssh_url(host: &str, config: &ProviderConfig, path: &str) -> String {
    let user = config.ssh_user.as_deref().unwrap_or("git");
    match config.ssh_port {
        Some(port) => format!("ssh://{user}@{host}:{port}/{path}.git"),
        None => format!("{user}@{host}:{path}.git"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn adapter_runs_gh_pagination_filters_archived_and_materializes_ssh() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("gh");
        let log = temp.path().join("args");
        std::fs::write(
            &executable,
            r##"#!/bin/sh
printf '%s' "$*" > "$GH_DIRECT_ARGS"
cat <<'JSON'
[{"data":{"viewer":{"repositories":{"nodes":[{"name":"active","isArchived":false,"sshUrl":"ssh://returned@ghe.example:2222/org/active.git","owner":{"login":"org"}},{"name":"old","isArchived":true,"sshUrl":"ssh://returned@ghe.example:2222/org/old.git","owner":{"login":"org"}}],"pageInfo":{"hasNextPage":true,"endCursor":"cursor-1"}}}}},{"data":{"viewer":{"repositories":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]
JSON
"##,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        unsafe { std::env::set_var("GH_DIRECT_ARGS", &log) };
        let config = ProviderConfig {
            ssh_user: Some("forge".to_owned()),
            ssh_port: Some(2222),
            ..ProviderConfig::default()
        };
        let provider = Github::with_executable("ghe.example".to_owned(), config, executable);
        let active = provider.catalog(false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(
            active[0].reference.clone_url,
            "ssh://forge@ghe.example:2222/org/active.git"
        );
        assert_eq!(provider.catalog(true).unwrap().len(), 2);
        let args = std::fs::read_to_string(log).unwrap();
        assert!(args.contains("--hostname ghe.example"));
        assert!(args.contains("--paginate"));
        assert!(args.contains("--slurp"));
        assert!(args.contains("after:$endCursor"));
        unsafe { std::env::remove_var("GH_DIRECT_ARGS") };
    }

    #[test]
    fn adapter_preserves_gh_stderr_on_command_failure() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("gh-fail");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s' 'auth fixture diagnostic' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let provider = Github::with_executable(
            "github.com".to_owned(),
            ProviderConfig::default(),
            executable,
        );
        let error = provider.catalog(false).unwrap_err();
        assert!(error.contains("auth fixture diagnostic"));
    }

    #[test]
    fn configured_ssh_identity_and_port_are_materialized() {
        let config = ProviderConfig {
            ssh_user: Some("forge".to_owned()),
            ssh_port: Some(2222),
            ..ProviderConfig::default()
        };
        assert_eq!(
            ssh_url("github.example", &config, "org/repo"),
            "ssh://forge@github.example:2222/org/repo.git"
        );
    }
}
