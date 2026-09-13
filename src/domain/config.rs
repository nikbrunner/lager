use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::domain::repository::RepositoryRef;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub root: String,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub repositories: Vec<RepositoryDeclaration>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub preset: String,
    #[serde(default)]
    pub prefix: String,
    pub api_url: Option<String>,
    pub auth: Option<String>,
    pub token_env: Option<String>,
    pub username_env: Option<String>,
    pub password_env: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepositoryDeclaration {
    pub url: String,
    pub post_clone: Option<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config root must be a portable path: {0}")]
    InvalidRoot(String),
    #[error("config root cannot be empty")]
    EmptyRoot,
    #[error("invalid config: {0}")]
    Semantic(String),
}

impl Config {
    pub fn resolve_root(&self, home: &Path) -> Result<PathBuf, ConfigError> {
        resolve_portable_path(&self.root, home)
    }

    /// Validates all recognized semantic values before an application operation may run.
    pub fn validate(&self) -> Result<(), ConfigError> {
        resolve_portable_path(&self.root, Path::new("/"))?;
        for (host, provider) in &self.providers {
            validate_provider(host, provider)?;
        }

        let mut declarations: HashMap<String, (Option<&str>, Vec<&str>, bool)> = HashMap::new();
        for declaration in &self.repositories {
            if declaration.url.matches('*').count() > usize::from(declaration.url.ends_with("/*")) {
                return semantic(format!(
                    "repository `{}` uses a wildcard anywhere other than trailing /*",
                    declaration.url
                ));
            }
            let reference = RepositoryRef::parse(&declaration.url)
                .map_err(|error| ConfigError::Semantic(error.to_string()))?;
            if reference.is_wildcard() {
                if declaration.post_clone.is_some() {
                    return semantic(format!(
                        "wildcard declaration `{}` cannot have post_clone",
                        declaration.url
                    ));
                }
                if !self.providers.contains_key(&reference.host) {
                    return semantic(format!(
                        "wildcard declaration `{}` requires a configured provider",
                        declaration.url
                    ));
                }
            } else if !declaration.exclude.is_empty() {
                return semantic(format!(
                    "explicit declaration `{}` cannot have exclusions",
                    declaration.url
                ));
            }
            if is_canonical_id(&declaration.url)
                && !is_builtin_host(&reference.host)
                && !self.providers.contains_key(&reference.host)
            {
                return semantic(format!(
                    "canonical repository ID `{}` requires a configured provider",
                    declaration.url
                ));
            }
            for exclusion in &declaration.exclude {
                validate_portable_component_path(exclusion, false, "repository exclusion")?;
                if exclusion.contains('*') {
                    return semantic(format!(
                        "repository exclusion must not contain a wildcard: {exclusion}"
                    ));
                }
            }
            let identity = format!(
                "{}{}",
                reference.identity(),
                if reference.is_wildcard() { "/*" } else { "" }
            );
            let semantics = (
                declaration.post_clone.as_deref(),
                declaration.exclude.iter().map(String::as_str).collect(),
                reference.is_wildcard(),
            );
            if declarations
                .insert(identity.clone(), semantics.clone())
                .is_some_and(|existing| existing != semantics)
            {
                return semantic(format!("conflicting declarations for `{identity}`"));
            }
        }
        Ok(())
    }
}

fn validate_provider(host: &str, provider: &ProviderConfig) -> Result<(), ConfigError> {
    if host.is_empty()
        || host != host.to_ascii_lowercase()
        || host.contains(['/', '@'])
        || host.contains("://")
        || host.chars().any(char::is_whitespace)
    {
        return semantic(format!("provider host is invalid: {host}"));
    }
    validate_portable_component_path(&provider.prefix, true, "provider prefix")?;
    match provider.preset.as_str() {
        "github" => {
            if provider.api_url.is_some()
                || provider.auth.is_some()
                || provider.token_env.is_some()
                || provider.username_env.is_some()
                || provider.password_env.is_some()
            {
                return semantic(format!(
                    "GitHub provider `{host}` uses gh authentication and cannot configure REST credentials"
                ));
            }
        }
        "bitbucket-cloud" => validate_bitbucket(host, provider, false)?,
        "bitbucket-data-center" => validate_bitbucket(host, provider, true)?,
        other => return semantic(format!("unknown provider preset `{other}` for `{host}`")),
    }
    if provider.ssh_port == Some(0) {
        return semantic(format!("provider `{host}` has invalid ssh_port 0"));
    }
    if provider
        .ssh_user
        .as_deref()
        .is_some_and(|value| value.is_empty() || value.contains(['@', '/', ':']))
    {
        return semantic(format!("provider `{host}` has invalid ssh_user"));
    }
    Ok(())
}

fn validate_bitbucket(
    host: &str,
    provider: &ProviderConfig,
    api_required: bool,
) -> Result<(), ConfigError> {
    if api_required && provider.api_url.is_none() {
        return semantic(format!(
            "Bitbucket Data Center provider `{host}` requires api_url"
        ));
    }
    if let Some(api_url) = &provider.api_url {
        let url = Url::parse(api_url)
            .map_err(|error| ConfigError::Semantic(format!("invalid API URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return semantic(format!(
                "provider `{host}` api_url must be HTTP(S), have a host, and contain no userinfo"
            ));
        }
    }
    let auth = provider.auth.as_deref().unwrap_or("anonymous");
    match auth {
        "anonymous" => {
            if provider.token_env.is_some()
                || provider.username_env.is_some()
                || provider.password_env.is_some()
            {
                return semantic(format!(
                    "anonymous provider `{host}` cannot configure credential environment variables"
                ));
            }
        }
        "bearer" => {
            require_env_name(host, "token_env", provider.token_env.as_deref())?;
            if provider.username_env.is_some() || provider.password_env.is_some() {
                return semantic(format!(
                    "bearer provider `{host}` cannot configure basic credentials"
                ));
            }
        }
        "basic" => {
            require_env_name(host, "username_env", provider.username_env.as_deref())?;
            require_env_name(host, "password_env", provider.password_env.as_deref())?;
            if provider.token_env.is_some() {
                return semantic(format!(
                    "basic provider `{host}` cannot configure token_env"
                ));
            }
        }
        other => return semantic(format!("unsupported authentication mode `{other}`")),
    }
    Ok(())
}

fn require_env_name(host: &str, field: &str, value: Option<&str>) -> Result<(), ConfigError> {
    if !value.is_some_and(|name| {
        !name.is_empty()
            && name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
    }) {
        return semantic(format!("provider `{host}` requires a valid {field}"));
    }
    Ok(())
}

fn is_builtin_host(host: &str) -> bool {
    matches!(host, "github.com" | "bitbucket.org")
}

fn is_canonical_id(value: &str) -> bool {
    !value.starts_with("git@")
        && !value.starts_with('/')
        && !value.contains("://")
        && value.split('/').count() >= 3
}

fn semantic<T>(message: String) -> Result<T, ConfigError> {
    Err(ConfigError::Semantic(message))
}

fn validate_portable_component_path(
    value: &str,
    empty_allowed: bool,
    field: &str,
) -> Result<(), ConfigError> {
    if value.is_empty() {
        return if empty_allowed {
            Ok(())
        } else {
            semantic(format!("{field} cannot be empty"))
        };
    }
    let path = Path::new(value);
    if path.is_absolute()
        || value == "~"
        || value.starts_with("~/")
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
                    | Component::CurDir
            )
        })
    {
        return semantic(format!("{field} must remain below its owner: {value}"));
    }
    let normalized = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => segment.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized != value {
        return semantic(format!("{field} must be normalized: {value}"));
    }
    Ok(())
}

pub fn resolve_portable_path(value: &str, home: &Path) -> Result<PathBuf, ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::EmptyRoot);
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(ConfigError::InvalidRoot(value.to_owned()));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(ConfigError::InvalidRoot(value.to_owned()));
    }
    if value == "~" {
        return Ok(home.to_path_buf());
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return Ok(home.join(relative));
    }
    Ok(home.join(path))
}
