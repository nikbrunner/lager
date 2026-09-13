use std::path::{Path, PathBuf};

use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RepositoryRefError {
    #[error("repository reference is empty")]
    Empty,
    #[error("repository reference is malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub clone_url: String,
    pub host: String,
    pub path: String,
    pub destination_segments: Vec<String>,
    pub wildcard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySummary {
    pub reference: RepositoryRef,
    pub archived: bool,
}

impl RepositoryRef {
    pub fn parse(input: &str) -> Result<Self, RepositoryRefError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(RepositoryRefError::Empty);
        }

        if let Some(path) = input.strip_prefix("file://") {
            let path = Path::new(path);
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| RepositoryRefError::Malformed(input.to_owned()))?;
            return Ok(Self {
                clone_url: input.to_owned(),
                host: "local".to_owned(),
                path: path.to_string_lossy().into_owned(),
                destination_segments: vec![name.to_owned()],
                wildcard: false,
            });
        }

        if input.starts_with("git@") {
            return Self::parse_scp(input);
        }

        if !input.contains("://") && !input.starts_with('/') {
            if let Some((owner, repo)) = input.split_once('/')
                && !owner.is_empty()
                && !repo.is_empty()
                && !repo.contains('/')
            {
                if owner.contains('.') || owner.contains('@') {
                    return Self::parse_host_path(input);
                }
                return Self::from_host_path(
                    "github.com",
                    &format!("{owner}/{repo}"),
                    format!("git@github.com:{owner}/{repo}.git"),
                );
            }
            if input.matches('/').count() >= 2 {
                return Self::parse_host_path(input);
            }
        }

        let url =
            Url::parse(input).map_err(|error| RepositoryRefError::Malformed(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| RepositoryRefError::Malformed(input.to_owned()))?;
        let path = clean_path(url.path());
        if path.is_empty() {
            return Err(RepositoryRefError::Malformed(input.to_owned()));
        }

        if !url.path().ends_with(".git")
            && let Some(reference) = Self::parse_browser_url(&url, host, &path)?
        {
            return Ok(reference);
        }

        Self::from_host_path(host, &path, input.to_owned())
    }

    pub fn destination(&self, root: &Path) -> PathBuf {
        self.destination_segments
            .iter()
            .fold(root.to_path_buf(), |path, segment| path.join(segment))
    }

    pub fn identity(&self) -> String {
        if self.host == "local" {
            return format!("file://{}", self.path);
        }
        format!("{}/{}", self.host.to_ascii_lowercase(), self.path)
    }

    pub fn is_wildcard(&self) -> bool {
        self.wildcard
    }

    fn parse_scp(input: &str) -> Result<Self, RepositoryRefError> {
        let at = input
            .rfind('@')
            .ok_or_else(|| RepositoryRefError::Malformed(input.to_owned()))?;
        let colon = input[at + 1..]
            .find(':')
            .map(|offset| at + 1 + offset)
            .ok_or_else(|| RepositoryRefError::Malformed(input.to_owned()))?;
        let host = &input[at + 1..colon];
        let path = clean_path(&input[colon + 1..]);
        if host.is_empty() || path.is_empty() {
            return Err(RepositoryRefError::Malformed(input.to_owned()));
        }
        Self::from_host_path(host, &path, input.to_owned())
    }

    fn parse_host_path(input: &str) -> Result<Self, RepositoryRefError> {
        let (host, path) = input
            .split_once('/')
            .ok_or_else(|| RepositoryRefError::Malformed(input.to_owned()))?;
        if host.is_empty() || path.is_empty() {
            return Err(RepositoryRefError::Malformed(input.to_owned()));
        }
        let clone_url = default_ssh_url(host, path);
        Self::from_host_path(host, path, clone_url)
    }

    fn parse_browser_url(
        url: &Url,
        host: &str,
        path: &str,
    ) -> Result<Option<Self>, RepositoryRefError> {
        let pieces: Vec<&str> = path.split('/').filter(|piece| !piece.is_empty()).collect();
        let is_github = host == "github.com" || host.ends_with("github.com");
        let is_bitbucket_cloud = host == "bitbucket.org";
        if is_github && pieces.len() == 2 {
            return Ok(Some(Self::from_host_path(
                host,
                path,
                default_ssh_url(host, path),
            )?));
        }
        if is_bitbucket_cloud && pieces.len() == 2 {
            return Ok(Some(Self::from_host_path(
                host,
                path,
                default_ssh_url(host, path),
            )?));
        }
        if pieces.len() == 5 && pieces[0] == "projects" && pieces[2] == "repos" {
            let dc_path = format!("{}/{}", pieces[1], pieces[3]);
            let ssh = format!("ssh://git@{}:7999/{}.git", host, dc_path);
            return Ok(Some(Self::from_host_path(host, &dc_path, ssh)?));
        }
        let _ = url;
        Ok(None)
    }

    fn from_host_path(
        host: &str,
        path: &str,
        clone_url: String,
    ) -> Result<Self, RepositoryRefError> {
        let path = clean_path(path);
        if path.is_empty() {
            return Err(RepositoryRefError::Malformed(clone_url));
        }
        let wildcard = path.ends_with("/*");
        let path = path.trim_end_matches("/*").trim_end_matches('/');
        let segments: Vec<String> = path
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .map(ToOwned::to_owned)
            .collect();
        if segments.is_empty() || segments.iter().any(|segment| segment.contains('\0')) {
            return Err(RepositoryRefError::Malformed(clone_url));
        }
        let destination_segments = if host == "github.com" || host == "bitbucket.org" {
            segments.clone()
        } else if host == "local" {
            vec![segments.last().cloned().unwrap_or_default()]
        } else {
            let mut destination = vec![host.to_owned()];
            destination.extend(segments.clone());
            destination
        };
        Ok(Self {
            clone_url,
            host: host.to_ascii_lowercase(),
            path: segments.join("/"),
            destination_segments,
            wildcard,
        })
    }
}

fn clean_path(path: &str) -> String {
    path.trim_matches('/').trim_end_matches(".git").to_owned()
}

fn default_ssh_url(host: &str, path: &str) -> String {
    format!(
        "git@{host}:{}.git",
        path.trim_matches('/').trim_end_matches(".git")
    )
}

pub fn normalize_remote(input: &str) -> Result<String, RepositoryRefError> {
    if input.starts_with('/') {
        return Ok(RepositoryRef::parse(&format!("file://{input}"))?.identity());
    }
    Ok(RepositoryRef::parse(input)?.identity())
}

#[cfg(test)]
mod tests {
    use super::RepositoryRef;

    #[test]
    fn parses_common_reference_forms() {
        let scp = RepositoryRef::parse("git@github.com:nikbrunner/lager.git").unwrap();
        assert_eq!(scp.identity(), "github.com/nikbrunner/lager");
        assert_eq!(scp.destination_segments, ["nikbrunner", "lager"]);

        let https = RepositoryRef::parse("https://github.com/nikbrunner/lager.git").unwrap();
        assert_eq!(https.clone_url, "https://github.com/nikbrunner/lager.git");

        let shorthand = RepositoryRef::parse("nikbrunner/lager").unwrap();
        assert_eq!(shorthand.clone_url, "git@github.com:nikbrunner/lager.git");

        let browser = RepositoryRef::parse("https://github.com/nikbrunner/lager").unwrap();
        assert_eq!(browser.clone_url, "git@github.com:nikbrunner/lager.git");
    }

    #[test]
    fn normalizes_local_path_remotes() {
        assert_eq!(
            super::normalize_remote("/tmp/repo.git").unwrap(),
            "file:///tmp/repo.git"
        );
    }

    #[test]
    fn maps_unknown_hosts_without_collisions() {
        let reference = RepositoryRef::parse("git@git.example.com:team/subgroup/repo.git").unwrap();
        assert_eq!(
            reference.destination_segments,
            ["git.example.com", "team", "subgroup", "repo"]
        );
    }

    #[test]
    fn strips_only_trailing_wildcard() {
        let reference = RepositoryRef::parse("git@github.com:org/*").unwrap();
        assert!(reference.is_wildcard());
        assert_eq!(reference.path, "org");
        assert_eq!(reference.destination_segments, ["org"]);
    }
}
