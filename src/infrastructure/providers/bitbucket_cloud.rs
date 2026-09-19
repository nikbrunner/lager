use reqwest::blocking::{Client, Response};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use url::Url;

use crate::domain::config::ProviderConfig;
use crate::domain::repository::{RepositoryRef, RepositorySummary};

#[derive(Debug, Clone)]
pub struct BitbucketCloud {
    pub host: String,
    pub config: ProviderConfig,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct Collection<T> {
    values: Vec<T>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkspacePermission {
    workspace: Workspace,
}

#[derive(Debug, Deserialize)]
struct Workspace {
    slug: Option<String>,
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    is_archived: Option<bool>,
    links: Links,
}

#[derive(Debug, Deserialize)]
struct Links {
    clone: Vec<CloneLink>,
}

#[derive(Debug, Deserialize)]
struct CloneLink {
    name: String,
    href: String,
}

impl BitbucketCloud {
    pub fn new(host: String, config: ProviderConfig) -> Result<Self, String> {
        Ok(Self {
            host,
            config,
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| format!("could not create HTTP client: {error}"))?,
        })
    }

    pub fn catalog(&self, include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
        let workspaces = self
            .paginate::<WorkspacePermission>(&self.endpoint("/2.0/user/permissions/workspaces")?)?;
        let mut result = Vec::new();
        for permission in workspaces {
            let Workspace { slug, uuid } = permission.workspace;
            let workspace = slug
                .or(uuid)
                .ok_or_else(|| "workspace response omitted slug and uuid".to_owned())?;
            result.extend(self.repositories(&workspace, include_archived)?);
        }
        Ok(result)
    }

    pub fn expand(
        &self,
        reference: &RepositoryRef,
        include_archived: bool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let workspace = reference
            .path
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Bitbucket wildcard requires a workspace".to_owned())?;
        self.repositories(workspace, include_archived)
    }

    fn repositories(
        &self,
        workspace: &str,
        include_archived: bool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let path = format!("/2.0/repositories/{workspace}");
        let repositories = self.paginate::<Repository>(&self.endpoint(&path)?)?;
        repositories
            .into_iter()
            .filter(|repository| include_archived || !repository.is_archived.unwrap_or(false))
            .map(|repository| summary(&self.host, &self.config, repository))
            .collect()
    }

    fn paginate<T: for<'de> Deserialize<'de>>(&self, initial: &str) -> Result<Vec<T>, String> {
        let trusted = Url::parse(initial)
            .map_err(|error| format!("invalid Bitbucket pagination URL: {error}"))?;
        validate_pagination_url(&trusted, &trusted)?;
        let mut next = Some(trusted.clone());
        let mut result = Vec::new();
        while let Some(url) = next.take() {
            validate_pagination_url(&trusted, &url)?;
            let response = self.request(&url)?;
            let page: Collection<T> = response
                .json()
                .map_err(|error| format!("could not parse Bitbucket response: {error}"))?;
            result.extend(page.values);
            next = page
                .next
                .map(|link| {
                    url.join(&link)
                        .map_err(|error| format!("invalid Bitbucket pagination URL: {error}"))
                })
                .transpose()?;
        }
        Ok(result)
    }

    fn request(&self, url: &Url) -> Result<Response, String> {
        let mut request = self.client.get(url.clone());
        match self.config.auth.as_deref().unwrap_or("anonymous") {
            "anonymous" => {}
            "bearer" => {
                let variable = self
                    .config
                    .token_env
                    .as_deref()
                    .ok_or_else(|| "bearer authentication requires token_env".to_owned())?;
                let token = std::env::var(variable).map_err(|_| {
                    format!("authentication environment variable {variable} is unset")
                })?;
                let value = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| "invalid bearer token".to_owned())?;
                request = request.header(AUTHORIZATION, value);
            }
            "basic" => {
                let username_env = self
                    .config
                    .username_env
                    .as_deref()
                    .ok_or_else(|| "basic authentication requires username_env".to_owned())?;
                let password_env = self
                    .config
                    .password_env
                    .as_deref()
                    .or(self.config.token_env.as_deref())
                    .ok_or_else(|| "basic authentication requires password_env".to_owned())?;
                let username = std::env::var(username_env).map_err(|_| {
                    format!("authentication environment variable {username_env} is unset")
                })?;
                let password = std::env::var(password_env).map_err(|_| {
                    format!("authentication environment variable {password_env} is unset")
                })?;
                request = request.basic_auth(username, Some(password));
            }
            mode => return Err(format!("unsupported authentication mode `{mode}`")),
        }
        let response = request
            .send()
            .map_err(|error| format!("Bitbucket request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Bitbucket request failed with status {}",
                response.status()
            ));
        }
        Ok(response)
    }

    fn endpoint(&self, path: &str) -> Result<String, String> {
        let base = self
            .config
            .api_url
            .as_deref()
            .unwrap_or("https://api.bitbucket.org")
            .trim_end_matches('/');
        Url::parse(&format!("{base}{path}"))
            .map(|url| url.to_string())
            .map_err(|error| format!("invalid Bitbucket API URL: {error}"))
    }
}

fn validate_pagination_url(trusted: &Url, candidate: &Url) -> Result<(), String> {
    if !matches!(candidate.scheme(), "http" | "https") {
        return Err("Bitbucket pagination URL uses an unsafe scheme".to_owned());
    }
    if !candidate.username().is_empty() || candidate.password().is_some() {
        return Err("Bitbucket pagination URL must not contain userinfo".to_owned());
    }
    if candidate.origin() != trusted.origin() {
        return Err("Bitbucket pagination URL changed the trusted origin".to_owned());
    }
    Ok(())
}

fn summary(
    host: &str,
    config: &ProviderConfig,
    repository: Repository,
) -> Result<RepositorySummary, String> {
    let link = repository
        .links
        .clone
        .iter()
        .find(|link| link.name.eq_ignore_ascii_case("ssh"))
        .or_else(|| repository.links.clone.first())
        .ok_or_else(|| "repository omitted clone links".to_owned())?;
    let path = RepositoryRef::parse(&link.href)
        .map(|reference| reference.path)
        .map_err(|error| error.to_string())?;
    let reference =
        RepositoryRef::parse(&ssh_url(host, config, &path)).map_err(|error| error.to_string())?;
    Ok(RepositorySummary {
        reference,
        archived: repository.is_archived.unwrap_or(false),
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
    use crate::infrastructure::providers::test_support::HttpFixture;

    #[test]
    fn adapter_paginates_workspaces_and_repositories_with_bearer_auth_and_filters_archives() {
        let fixture = HttpFixture::new(vec![
            (
                "/2.0/user/permissions/workspaces".to_owned(),
                "{\"values\":[{\"workspace\":{\"slug\":\"one\"}}],\"next\":\"{BASE}/workspaces?page=2\"}".to_owned(),
            ),
            (
                "/workspaces?page=2".to_owned(),
                "{\"values\":[{\"workspace\":{\"slug\":\"two\"}}],\"next\":null}".to_owned(),
            ),
            (
                "/2.0/repositories/one".to_owned(),
                "{\"values\":[{\"is_archived\":false,\"full_name\":\"one/active\",\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://returned@bb.example/one/active.git\"}]}},{\"is_archived\":true,\"full_name\":\"one/old\",\"links\":{\"clone\":[{\"name\":\"https\",\"href\":\"https://bb.example/one/old.git\"}]}}],\"next\":null}".to_owned(),
            ),
            (
                "/2.0/repositories/two".to_owned(),
                "{\"values\":[{\"is_archived\":false,\"full_name\":\"two/active\",\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://returned@bb.example/two/active.git\"}]}}],\"next\":null}".to_owned(),
            ),
        ]);
        let variable = "LAGER_DIRECT_CLOUD_TOKEN";
        unsafe { std::env::set_var(variable, "direct-token") };
        let config = ProviderConfig {
            api_url: Some(fixture.base.clone()),
            auth: Some("bearer".to_owned()),
            token_env: Some(variable.to_owned()),
            ssh_user: Some("forge".to_owned()),
            ssh_port: Some(2222),
            ..ProviderConfig::default()
        };
        let provider = BitbucketCloud::new("bb.example".to_owned(), config).unwrap();
        let active = provider.catalog(false).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(
            active[0].reference.clone_url,
            "ssh://forge@bb.example:2222/one/active.git"
        );
        let all = provider.catalog(true).unwrap();
        assert_eq!(all.len(), 3);
        let requests = fixture.requests();
        assert!(
            requests
                .iter()
                .any(|(path, auth)| path == "/workspaces?page=2" && auth == "Bearer direct-token")
        );
        assert!(
            requests
                .iter()
                .all(|(_, auth)| auth == "Bearer direct-token")
        );
        unsafe { std::env::remove_var(variable) };
    }

    #[test]
    fn adapter_reports_unknown_auth_mode_before_request() {
        let config = ProviderConfig {
            api_url: Some("http://127.0.0.1:9".to_owned()),
            auth: Some("invalid".to_owned()),
            ..ProviderConfig::default()
        };
        let provider = BitbucketCloud::new("bb.example".to_owned(), config).unwrap();
        let reference = RepositoryRef::parse("bb.example/team/*").unwrap();
        let error = provider.expand(&reference, false).unwrap_err();
        assert!(error.contains("unsupported authentication mode"));
    }
}
