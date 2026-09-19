use reqwest::blocking::{Client, Response};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use url::Url;

use crate::domain::config::ProviderConfig;
use crate::domain::repository::{RepositoryRef, RepositorySummary};

#[derive(Debug, Clone)]
pub struct BitbucketDataCenter {
    pub host: String,
    pub config: ProviderConfig,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    values: Vec<T>,
    #[serde(rename = "isLastPage")]
    is_last_page: bool,
    #[serde(rename = "nextPageStart")]
    next_page_start: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    archived: Option<bool>,
    slug: String,
    project: Project,
    links: Links,
}

#[derive(Debug, Deserialize)]
struct Project {
    key: String,
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

impl BitbucketDataCenter {
    pub fn new(host: String, config: ProviderConfig) -> Result<Self, String> {
        Ok(Self {
            host,
            config,
            client: Client::builder()
                .build()
                .map_err(|error| format!("could not create HTTP client: {error}"))?,
        })
    }

    pub fn catalog(&self, include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
        let mut result = Vec::new();
        let mut start = 0;
        loop {
            let url = self.endpoint(&format!(
                "/rest/api/1.0/repos?permission=REPO_READ&start={start}&limit=25"
            ))?;
            let page: Page<Repository> = self.request(&url)?.json().map_err(parse_error)?;
            for repository in page.values {
                if include_archived || !repository.archived.unwrap_or(false) {
                    result.push(self.summary(repository)?);
                }
            }
            if page.is_last_page {
                break;
            }
            start = page
                .next_page_start
                .ok_or_else(|| "Bitbucket response omitted nextPageStart".to_owned())?;
        }
        Ok(result)
    }

    pub fn expand(
        &self,
        reference: &RepositoryRef,
        include_archived: bool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let project = reference
            .path
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Bitbucket wildcard requires a project".to_owned())?;
        let mut result = Vec::new();
        let mut start = 0;
        loop {
            let encoded = urlencoding(project);
            let url = self.endpoint(&format!(
                "/rest/api/1.0/projects/{encoded}/repos?start={start}&limit=25"
            ))?;
            let page: Page<Repository> = self.request(&url)?.json().map_err(parse_error)?;
            for repository in page.values {
                if include_archived || !repository.archived.unwrap_or(false) {
                    result.push(self.summary(repository)?);
                }
            }
            if page.is_last_page {
                break;
            }
            start = page
                .next_page_start
                .ok_or_else(|| "Bitbucket response omitted nextPageStart".to_owned())?;
        }
        Ok(result)
    }

    fn summary(&self, repository: Repository) -> Result<RepositorySummary, String> {
        let link = repository
            .links
            .clone
            .iter()
            .find(|link| link.name.eq_ignore_ascii_case("ssh"))
            .or_else(|| repository.links.clone.first())
            .ok_or_else(|| "Bitbucket repository omitted clone links".to_owned())?;
        RepositoryRef::parse(&link.href).map_err(|error| error.to_string())?;
        let user = self.config.ssh_user.as_deref().unwrap_or("git");
        let port = self.config.ssh_port.unwrap_or(7999);
        let fallback = format!(
            "ssh://{user}@{}:{port}/{}/{}.git",
            self.host, repository.project.key, repository.slug
        );
        let reference = RepositoryRef::parse(&fallback).map_err(|error| error.to_string())?;
        Ok(RepositorySummary {
            reference,
            archived: repository.archived.unwrap_or(false),
        })
    }

    fn request(&self, url: &str) -> Result<Response, String> {
        let mut request = self.client.get(url);
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
                request = request.header(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}"))
                        .map_err(|_| "invalid bearer token".to_owned())?,
                );
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
            .ok_or_else(|| "Bitbucket Data Center requires api_url".to_owned())?
            .trim_end_matches('/');
        Url::parse(&format!("{base}{path}"))
            .map(|url| url.to_string())
            .map_err(|error| format!("invalid Bitbucket API URL: {error}"))
    }
}

fn parse_error(error: reqwest::Error) -> String {
    format!("could not parse Bitbucket response: {error}")
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::test_support::HttpFixture;

    #[test]
    fn adapter_paginates_readable_repositories_with_bearer_auth_and_filters_archives() {
        let fixture = HttpFixture::new(vec![
            (
                "/rest/api/1.0/repos?permission=REPO_READ&start=0&limit=25".to_owned(),
                "{\"values\":[{\"archived\":false,\"slug\":\"active\",\"project\":{\"key\":\"PROJ\"},\"links\":{\"clone\":[{\"name\":\"ssh\",\"href\":\"ssh://returned@dc.example:7999/PROJ/active.git\"}]}}],\"isLastPage\":false,\"nextPageStart\":13,\"start\":0,\"limit\":25}".to_owned(),
            ),
            (
                "/rest/api/1.0/repos?permission=REPO_READ&start=13&limit=25".to_owned(),
                "{\"values\":[{\"archived\":true,\"slug\":\"old\",\"project\":{\"key\":\"PROJ\"},\"links\":{\"clone\":[{\"name\":\"http\",\"href\":\"https://dc.example/PROJ/old.git\"}]}}],\"isLastPage\":true,\"start\":13,\"limit\":25}".to_owned(),
            ),
        ]);
        let variable = "LAGER_DIRECT_DC_TOKEN";
        unsafe { std::env::set_var(variable, "dc-token") };
        let config = ProviderConfig {
            api_url: Some(fixture.base.clone()),
            auth: Some("bearer".to_owned()),
            token_env: Some(variable.to_owned()),
            ssh_user: Some("forge".to_owned()),
            ssh_port: Some(2222),
            ..ProviderConfig::default()
        };
        let provider = BitbucketDataCenter::new("dc.example".to_owned(), config).unwrap();
        let active = provider.catalog(false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(
            active[0].reference.clone_url,
            "ssh://forge@dc.example:2222/PROJ/active.git"
        );
        assert_eq!(provider.catalog(true).unwrap().len(), 2);
        let requests = fixture.requests();
        assert!(
            requests
                .iter()
                .any(|(path, auth)| path.contains("start=13") && auth == "Bearer dc-token")
        );
        assert!(requests.iter().all(|(_, auth)| auth == "Bearer dc-token"));
        unsafe { std::env::remove_var(variable) };
    }

    #[test]
    fn adapter_expands_personal_project_without_encoding_tilde() {
        let fixture = HttpFixture::new(vec![(
            "/rest/api/1.0/projects/~alice/repos?start=0&limit=25".to_owned(),
            "{\"values\":[],\"isLastPage\":true,\"start\":0,\"limit\":25}".to_owned(),
        )]);
        let config = ProviderConfig {
            api_url: Some(fixture.base.clone()),
            ..ProviderConfig::default()
        };
        let provider = BitbucketDataCenter::new("dc.example".to_owned(), config).unwrap();
        let reference = RepositoryRef::parse("dc.example/~alice/*").unwrap();
        assert!(provider.expand(&reference, false).unwrap().is_empty());
        assert!(
            fixture
                .requests()
                .iter()
                .any(|(path, _)| path.contains("projects/~alice/repos"))
        );
    }
}
