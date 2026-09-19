use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::domain::config::ProviderConfig;
use crate::domain::repository::{RepositoryRef, RepositorySummary};

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
        let pages: Vec<Page<ViewerData>> = self.query(PAGE_QUERY)?;
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
        let organization = reference
            .path
            .split('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "GitHub wildcard requires an organization".to_owned())?;
        let query = format!(
            "query($endCursor:String){{organization(login:{organization:?}){{repositories(first:100,after:$endCursor){{nodes{{name isArchived isPrivate sshUrl owner{{login}}}} pageInfo{{hasNextPage endCursor}}}}}}}}"
        );
        let pages: Vec<Page<OrganizationData>> = self.query(&query)?;
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

    fn query<T: for<'de> Deserialize<'de>>(&self, query: &str) -> Result<Vec<T>, String> {
        let mut command = Command::new(&self.gh_executable);
        command.args(["api"]);
        if self.host != "github.com" {
            command.args(["--hostname", &self.host]);
        }
        command.args(["graphql", "--paginate", "--slurp", "-f"]);
        command.arg(format!("query={query}"));
        let output = command.output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "GitHub provider requires `gh`; install GitHub CLI and run `gh auth login`"
                    .to_owned()
            } else {
                format!("could not start gh: {error}")
            }
        })?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("could not parse gh response: {error}"))
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
