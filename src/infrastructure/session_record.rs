//! `sessions.json` in lager's cache directory: the session lager created for each checkout.
//! Known limit: wiping the cache makes lager forget its sessions, which keep running.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::application::ports::{Recorded, SessionRecords};
use crate::infrastructure::config::{cache_dir, lock, replace_file};

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionFile;

#[derive(Serialize, Deserialize)]
struct Document {
    schema_version: u32,
    sessions: BTreeMap<PathBuf, Recorded>,
}

impl SessionRecords for SessionFile {
    fn update<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<PathBuf, Recorded>) -> T,
    ) -> Result<T, String> {
        let _lock =
            lock("sessions.lock").map_err(|error| format!("could not lock sessions: {error}"))?;
        let path = cache_dir().join("sessions.json");
        let unreadable = |error: &dyn std::fmt::Display| {
            format!("could not read sessions {}: {error}", path.display())
        };
        let mut sessions = match std::fs::read(&path) {
            Ok(bytes) => {
                let document: Document =
                    serde_json::from_slice(&bytes).map_err(|error| unreadable(&error))?;
                if document.schema_version != 1 {
                    return Err(unreadable(&format!(
                        "unknown schema version {}",
                        document.schema_version
                    )));
                }
                document.sessions
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(unreadable(&error)),
        };
        let before = sessions.clone();
        let result = change(&mut sessions);
        if sessions != before {
            let document = Document {
                schema_version: 1,
                sessions,
            };
            let bytes = serde_json::to_vec_pretty(&document)
                .map_err(|error| format!("could not write sessions: {error}"))?;
            replace_file(&path, &bytes).map_err(|(path, error)| {
                format!("could not write sessions {}: {error}", path.display())
            })?;
        }
        Ok(result)
    }
}
