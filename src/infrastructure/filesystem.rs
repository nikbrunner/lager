use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::application::ports::{ConfigurationFilesystem, RemovalFilesystem};

#[derive(Debug, Default, Clone, Copy)]
pub struct Filesystem;

impl ConfigurationFilesystem for Filesystem {
    type Error = std::io::Error;

    fn create_dir_all(&self, path: &Path) -> Result<(), Self::Error> {
        std::fs::create_dir_all(path)
    }
}

impl RemovalFilesystem for Filesystem {
    type Error = std::io::Error;

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error> {
        std::fs::canonicalize(path)
    }

    fn is_symlink(&self, path: &Path) -> Result<bool, Self::Error> {
        Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
    }

    fn exists(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn metadata_exists(&self, path: &Path) -> Result<bool, Self::Error> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn contains_symlink(&self, path: &Path) -> Result<bool, Self::Error> {
        for entry in WalkDir::new(path).follow_links(false) {
            let entry = entry.map_err(|error| {
                let message = error.to_string();
                error
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other(message))
            })?;
            if entry.path() != path && entry.file_type().is_symlink() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(unix)]
    fn directory_identity(&self, path: &Path) -> Result<String, Self::Error> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), Self::Error> {
        std::fs::remove_dir_all(path)
    }

    fn discover_checkouts(&self, root: &Path) -> Result<Vec<PathBuf>, Self::Error> {
        let mut checkouts = Vec::new();
        let mut entries = WalkDir::new(root)
            .min_depth(1)
            .follow_links(false)
            .into_iter();
        while let Some(entry) = entries.next() {
            let entry = entry.map_err(|error| {
                let message = error.to_string();
                error
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other(message))
            })?;
            if entry.file_type().is_dir() && entry.path().join(".git").exists() {
                checkouts.push(entry.into_path());
                entries.skip_current_dir();
            }
        }
        Ok(checkouts)
    }
}

impl crate::application::ports::WorktreeFilesystem for Filesystem {
    fn kind(&self, path: &Path) -> Result<crate::application::ports::PathKind, String> {
        use crate::application::ports::PathKind;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Ok(PathKind::Symlink),
            Ok(metadata) if metadata.is_dir() => Ok(PathKind::Directory),
            Ok(_) => Ok(PathKind::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PathKind::Missing),
            Err(error) => Err(error.to_string()),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        std::fs::canonicalize(path).map_err(|error| error.to_string())
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|error| error.to_string())
    }

    fn identity(&self, path: &Path) -> Result<String, String> {
        RemovalFilesystem::directory_identity(self, path).map_err(|error| error.to_string())
    }
}
