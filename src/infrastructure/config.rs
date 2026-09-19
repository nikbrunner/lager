use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use toml_edit::{ArrayOfTables, Document, DocumentMut, Item, Table, value};

use crate::application::ports::{
    AtomicRegistrationStore, ConfigStore, Mutation, RegistrationRequest,
};
use crate::domain::config::{Config, ConfigError};
use crate::domain::repository::RepositoryRef;
use crate::presentation::escape;

#[derive(Debug, thiserror::Error)]
pub enum ConfigStoreError {
    #[error("could not read config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse config {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("could not write config {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not lock config: {0}")]
    Lock(#[source] std::io::Error),
    #[error(transparent)]
    Invalid(#[from] ConfigError),
    #[error("repository reference is invalid: {0}")]
    Reference(String),
    #[error("conflicting declaration for {0}")]
    Conflict(String),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FileConfigStore;

pub fn load(path: &Path) -> Result<Config, ConfigStoreError> {
    load_with_warnings(path, true)
}

fn load_with_warnings(path: &Path, warnings: bool) -> Result<Config, ConfigStoreError> {
    let content = fs::read_to_string(path).map_err(|source| ConfigStoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let document = parse_document(path, &content)?;
    let config = decode_config(path, &document)?;
    if warnings {
        warn_unknown_keys(&document.into_mut());
    }
    Ok(config)
}

pub fn init(path: &Path, root: &str, github: bool) -> Result<(), ConfigStoreError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(ConfigStoreError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "config already exists"),
        });
    }
    crate::domain::config::resolve_portable_path(root, &home_dir())?;
    let mut document = DocumentMut::new();
    document["root"] = value(root);
    if github {
        let mut provider = Table::new();
        provider["preset"] = value("github");
        provider["prefix"] = value("");
        let mut providers = Table::new();
        providers["github.com"] = Item::Table(provider);
        document["providers"] = Item::Table(providers);
    }
    write_document(path, &document)
}

pub fn register(
    path: &Path,
    references: &[String],
    post_clone: Option<&str>,
) -> Result<Vec<Mutation>, ConfigStoreError> {
    register_with_warnings(path, references, post_clone, true)
}

fn register_with_warnings(
    path: &Path,
    references: &[String],
    post_clone: Option<&str>,
    warnings: bool,
) -> Result<Vec<Mutation>, ConfigStoreError> {
    mutate(path, warnings, |document| {
        edit_registrations(
            document,
            references
                .iter()
                .map(|reference| (reference.as_str(), post_clone)),
        )
    })
}

fn edit_registrations<'a>(
    document: &mut DocumentMut,
    requests: impl Iterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<Vec<Mutation>, ConfigStoreError> {
    let mut results = Vec::new();
    for (input, post_clone) in requests {
        let reference = RepositoryRef::parse(input)
            .map_err(|error| ConfigStoreError::Reference(error.to_string()))?;
        if reference.is_wildcard() {
            if post_clone.is_some() {
                return Err(ConfigStoreError::Reference(
                    "wildcard declarations cannot have post_clone".to_owned(),
                ));
            }
            let repositories = document
                .get("repositories")
                .and_then(Item::as_array_of_tables)
                .cloned()
                .unwrap_or_default();
            let exists = repositories.iter().any(|declaration| {
                declaration
                    .get("url")
                    .and_then(Item::as_str)
                    .and_then(|url| RepositoryRef::parse(url).ok())
                    .is_some_and(|parsed| {
                        parsed.is_wildcard() && parsed.identity() == reference.identity()
                    })
            });
            if exists {
                results.push(Mutation::Noop);
            } else {
                let mut updated = repositories;
                let mut declaration = Table::new();
                declaration["url"] = value(input);
                updated.push(declaration);
                document["repositories"] = Item::ArrayOfTables(updated);
                results.push(Mutation::Changed);
            }
            continue;
        }

        let mut changed = false;
        let mut matched_wildcard = false;
        let repositories = document
            .get("repositories")
            .and_then(Item::as_array_of_tables)
            .cloned()
            .unwrap_or_default();
        let explicit_index = repositories.iter().position(|declaration| {
            declaration
                .get("url")
                .and_then(Item::as_str)
                .and_then(|url| RepositoryRef::parse(url).ok())
                .is_some_and(|parsed| {
                    !parsed.is_wildcard() && parsed.identity() == reference.identity()
                })
        });

        if let Some(index) = explicit_index
            && let Some(command) = post_clone
            && let Some(existing) = repositories
                .iter()
                .nth(index)
                .and_then(|declaration| declaration.get("post_clone"))
                .and_then(Item::as_str)
            && existing != command
        {
            return Err(ConfigStoreError::Conflict(reference.identity()));
        }

        let mut updated = repositories.clone();
        for declaration in updated.iter_mut() {
            let Some(url) = declaration.get("url").and_then(Item::as_str) else {
                continue;
            };
            let parsed = RepositoryRef::parse(url)
                .map_err(|error| ConfigStoreError::Reference(error.to_string()))?;
            if !parsed.is_wildcard() || !wildcard_matches(&parsed, &reference) {
                continue;
            }
            matched_wildcard = true;
            let relative = relative_wildcard_name(&parsed, &reference);
            if let Some(excludes) = declaration.get_mut("exclude").and_then(Item::as_array_mut) {
                let before = excludes.len();
                excludes.retain(|item| item.as_str() != Some(&relative));
                if excludes.len() != before {
                    changed = true;
                    if excludes.is_empty() {
                        declaration.remove("exclude");
                    }
                }
            }
        }

        if let Some(index) = explicit_index {
            if let Some(command) = post_clone
                && let Some(declaration) = updated.iter_mut().nth(index)
                && declaration
                    .get("post_clone")
                    .and_then(Item::as_str)
                    .is_none()
            {
                declaration["post_clone"] = value(command);
                changed = true;
            }
        } else if post_clone.is_some() || !matched_wildcard {
            let mut declaration = Table::new();
            declaration["url"] = value(input);
            if let Some(command) = post_clone {
                declaration["post_clone"] = value(command);
            }
            updated.push(declaration);
            changed = true;
        }

        if changed {
            document["repositories"] = Item::ArrayOfTables(updated);
            results.push(Mutation::Changed);
        } else {
            results.push(Mutation::Noop);
        }
    }
    Ok(results)
}

pub fn remove(path: &Path, references: &[String]) -> Result<Vec<Mutation>, ConfigStoreError> {
    remove_with_warnings(path, references, true)
}

fn remove_with_warnings(
    path: &Path,
    references: &[String],
    warnings: bool,
) -> Result<Vec<Mutation>, ConfigStoreError> {
    mutate(path, warnings, |document| {
        let mut results = Vec::with_capacity(references.len());
        for input in references {
            let reference = RepositoryRef::parse(input)
                .map_err(|error| ConfigStoreError::Reference(error.to_string()))?;
            let repositories = document
                .get("repositories")
                .and_then(Item::as_array_of_tables)
                .cloned()
                .unwrap_or_default();
            let mut removed = false;
            let mut keep = ArrayOfTables::new();
            for declaration in repositories.iter() {
                let Some(url) = declaration.get("url").and_then(Item::as_str) else {
                    keep.push(declaration.clone());
                    continue;
                };
                let parsed = RepositoryRef::parse(url)
                    .map_err(|error| ConfigStoreError::Reference(error.to_string()))?;
                if parsed.identity() == reference.identity() {
                    removed = true;
                    continue;
                }
                let mut declaration = declaration.clone();
                if parsed.is_wildcard() && wildcard_matches(&parsed, &reference) {
                    let relative = relative_wildcard_name(&parsed, &reference);
                    let excludes = declaration["exclude"]
                        .or_insert(value(toml_edit::Array::new()))
                        .as_array_mut()
                        .ok_or_else(|| {
                            ConfigStoreError::Reference(
                                "repository exclude must be an array".to_owned(),
                            )
                        })?;
                    if !excludes.iter().any(|item| item.as_str() == Some(&relative)) {
                        excludes.push(relative);
                        removed = true;
                    }
                }
                keep.push(declaration);
            }
            if removed {
                document["repositories"] = Item::ArrayOfTables(keep);
                results.push(Mutation::Changed);
            } else {
                results.push(Mutation::Noop);
            }
        }
        Ok(results)
    })
}

fn mutate<T>(
    path: &Path,
    warnings: bool,
    operation: impl FnOnce(&mut DocumentMut) -> Result<T, ConfigStoreError>,
) -> Result<T, ConfigStoreError> {
    let target = resolve_target(path).map_err(|source| ConfigStoreError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    let lock = lock_for(&target)?;
    let _guard = lock;
    let content = fs::read_to_string(&target).map_err(|source| ConfigStoreError::Read {
        path: target.clone(),
        source,
    })?;
    let document = parse_document(&target, &content)?;
    decode_config(&target, &document)?;
    let mut document = document.into_mut();
    if warnings {
        warn_unknown_keys(&document);
    }
    let original = document.to_string();
    let result = operation(&mut document)?;
    decode_config(&target, &parse_document(&target, &document.to_string())?)?;
    if document.to_string() != original {
        write_document(&target, &document)?;
    }
    Ok(result)
}

fn decode_config(path: &Path, document: &Document<String>) -> Result<Config, ConfigStoreError> {
    let config: Config = toml_edit::de::from_document(document.clone()).map_err(|error| {
        ConfigStoreError::Parse {
            path: path.to_path_buf(),
            message: config_error_location(
                "invalid TOML configuration",
                document.raw(),
                error.span(),
            ),
        }
    })?;
    config.validate()?;
    config.resolve_root(&home_dir())?;
    Ok(config)
}

fn parse_document(path: &Path, content: &str) -> Result<Document<String>, ConfigStoreError> {
    content
        .parse::<Document<String>>()
        .map_err(|error| ConfigStoreError::Parse {
            path: path.to_path_buf(),
            message: config_error_location("invalid TOML syntax", content, error.span()),
        })
}

fn config_error_location(
    kind: &str,
    content: &str,
    span: Option<std::ops::Range<usize>>,
) -> String {
    let Some(span) = span else {
        return format!("{kind} (values omitted)");
    };
    let before = &content[..span.start];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{kind} at line {line}, column {column} (values omitted)")
}

struct LockGuard(File);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_for(path: &Path) -> Result<LockGuard, ConfigStoreError> {
    let lock_root = env::var_os("LAGER_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".cache/lager"));
    fs::create_dir_all(&lock_root).map_err(ConfigStoreError::Lock)?;
    let identity = lock_identity(path);
    let key = format!("{:x}", seahash(identity.to_string_lossy().as_bytes()));
    let lock_path = lock_root.join(format!("config-{key}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(ConfigStoreError::Lock)?;
    file.lock_exclusive().map_err(ConfigStoreError::Lock)?;
    Ok(LockGuard(file))
}

fn lock_identity(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    match (fs::canonicalize(parent), path.file_name()) {
        (Ok(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

fn write_document(path: &Path, document: &DocumentMut) -> Result<(), ConfigStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigStoreError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let metadata = fs::metadata(path).ok();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_file_name(format!(
        ".{}.lager-{stamp}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut file = File::create(&temporary).map_err(|source| ConfigStoreError::Write {
        path: temporary.clone(),
        source,
    })?;
    file.write_all(document.to_string().as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|source| ConfigStoreError::Write {
            path: temporary.clone(),
            source,
        })?;
    if let Some(metadata) = metadata {
        fs::set_permissions(&temporary, metadata.permissions()).map_err(|source| {
            ConfigStoreError::Write {
                path: temporary.clone(),
                source,
            }
        })?;
    }
    fs::rename(&temporary, path).map_err(|source| ConfigStoreError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn resolve_target(path: &Path) -> Result<PathBuf, std::io::Error> {
    let mut current = path.to_path_buf();
    for _ in 0..64 {
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(current);
        }
        let link = fs::read_link(&current)?;
        current = if link.is_absolute() {
            link
        } else {
            current.parent().unwrap_or(Path::new(".")).join(link)
        };
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "too many config symlink hops",
    ))
}

fn wildcard_matches(pattern: &RepositoryRef, candidate: &RepositoryRef) -> bool {
    candidate.host == pattern.host
        && candidate.path.starts_with(&(pattern.path.clone() + "/"))
        && !candidate.is_wildcard()
}

fn relative_wildcard_name(pattern: &RepositoryRef, candidate: &RepositoryRef) -> String {
    candidate
        .path
        .strip_prefix(&(pattern.path.clone() + "/"))
        .unwrap_or(&candidate.path)
        .to_owned()
}

fn warn_unknown_keys(document: &DocumentMut) {
    for (key, _) in document.iter() {
        if !matches!(key, "root" | "providers" | "repositories") {
            eprintln!("lager: warning: unknown config key `{}`", escape(key));
        }
    }
    if let Some(providers) = document.get("providers").and_then(Item::as_table_like) {
        for (host, item) in providers.iter() {
            if let Some(table) = item.as_table_like() {
                for (key, _) in table.iter() {
                    if !matches!(
                        key,
                        "preset"
                            | "prefix"
                            | "api_url"
                            | "auth"
                            | "token_env"
                            | "username_env"
                            | "password_env"
                            | "ssh_user"
                            | "ssh_port"
                    ) {
                        eprintln!(
                            "lager: warning: unknown provider key `{}.{}`",
                            escape(host),
                            escape(key)
                        );
                    }
                }
            }
        }
    }
    if let Some(repositories) = document
        .get("repositories")
        .and_then(Item::as_array_of_tables)
    {
        for (index, repository) in repositories.iter().enumerate() {
            for (key, _) in repository.iter() {
                if !matches!(key, "url" | "post_clone" | "exclude") {
                    eprintln!(
                        "lager: warning: unknown repository key `repositories[{index}].{}`",
                        escape(key)
                    );
                }
            }
        }
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn seahash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

impl AtomicRegistrationStore for FileConfigStore {
    fn register_batch(
        &self,
        path: &Path,
        requests: &[RegistrationRequest],
    ) -> Result<Vec<Mutation>, Self::Error> {
        register_batch_with_warnings(path, requests, true)
    }
}

fn register_batch_with_warnings(
    path: &Path,
    requests: &[RegistrationRequest],
    warnings: bool,
) -> Result<Vec<Mutation>, ConfigStoreError> {
    mutate(path, warnings, |document| {
        edit_registrations(
            document,
            requests
                .iter()
                .map(|request| (request.reference.as_str(), request.post_clone.as_deref())),
        )
    })
}

/// Command-scoped warning policy only: every read and locked mutation still
/// decodes and validates the latest document, without caching configuration.
#[derive(Default)]
pub(crate) struct CommandConfigStore {
    warned: std::cell::Cell<bool>,
}

impl CommandConfigStore {
    fn should_warn(&self) -> bool {
        !self.warned.replace(true)
    }
}

impl ConfigStore for CommandConfigStore {
    type Error = ConfigStoreError;

    fn exists(&self, path: &Path) -> bool {
        FileConfigStore.exists(path)
    }
    fn load(&self, path: &Path) -> Result<Config, Self::Error> {
        load_with_warnings(path, self.should_warn())
    }
    fn init(&self, path: &Path, root: &str, github: bool) -> Result<(), Self::Error> {
        init(path, root, github)
    }
    fn register(
        &self,
        path: &Path,
        references: &[String],
        post_clone: Option<&str>,
    ) -> Result<Vec<Mutation>, Self::Error> {
        register_with_warnings(path, references, post_clone, self.should_warn())
    }
    fn remove(&self, path: &Path, references: &[String]) -> Result<Vec<Mutation>, Self::Error> {
        remove_with_warnings(path, references, self.should_warn())
    }
}

impl AtomicRegistrationStore for CommandConfigStore {
    fn register_batch(
        &self,
        path: &Path,
        requests: &[RegistrationRequest],
    ) -> Result<Vec<Mutation>, Self::Error> {
        register_batch_with_warnings(path, requests, self.should_warn())
    }
}

impl ConfigStore for FileConfigStore {
    type Error = ConfigStoreError;

    fn exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn load(&self, path: &Path) -> Result<Config, Self::Error> {
        load(path)
    }

    fn init(&self, path: &Path, root: &str, github: bool) -> Result<(), Self::Error> {
        init(path, root, github)
    }

    fn register(
        &self,
        path: &Path,
        references: &[String],
        post_clone: Option<&str>,
    ) -> Result<Vec<Mutation>, Self::Error> {
        register(path, references, post_clone)
    }

    fn remove(&self, path: &Path, references: &[String]) -> Result<Vec<Mutation>, Self::Error> {
        remove(path, references)
    }
}
