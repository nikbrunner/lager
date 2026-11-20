//! Terminal sessions for checkouts: one per checkout, found again through the records lager
//! keeps, never by label. Every call first forgets records whose session is gone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::application::ports::{HostError, Recorded, SessionHost, SessionRecords};
use crate::domain::session::Layout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    Host(HostError),
    /// The records could not be read or saved.
    Records(String),
}

impl From<HostError> for SessionError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    Created(Recorded),
    Existing(Recorded),
}

impl Ensured {
    pub fn recorded(&self) -> &Recorded {
        match self {
            Self::Created(recorded) | Self::Existing(recorded) => recorded,
        }
    }
}

/// The repository's name for a primary checkout; for a linked worktree also its name in
/// words, with `-` and `_` as spaces and the first letter upper-cased.
pub fn label(repository: &str, worktree: Option<&str>) -> String {
    let label = match worktree {
        None => repository.to_owned(),
        Some(worktree) => {
            let words = worktree.replace(['-', '_'], " ");
            let mut letters = words.chars();
            let words: String = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default();
            format!("{repository} · {words}")
        }
    };
    let mut label: String = label.chars().filter(|c| !c.is_control()).collect();
    let mut end = label.len().min(LABEL_BYTES);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    label.truncate(end);
    label.trim().to_owned()
}

/// The host's label limit, stated in characters; bytes stay within it either way.
const LABEL_BYTES: usize = 128;

fn prune<H: SessionHost>(
    host: &H,
    records: &mut BTreeMap<PathBuf, Recorded>,
) -> Result<(), HostError> {
    let live = host.live()?;
    records.retain(|_, recorded| live.contains(&recorded.id));
    Ok(())
}

fn flatten<T>(result: Result<Result<T, HostError>, String>) -> Result<T, SessionError> {
    result
        .map_err(SessionError::Records)?
        .map_err(SessionError::Host)
}

/// The session of `checkout`, created with `layout` when it has none. The records stay locked
/// meanwhile, so concurrent calls for one checkout create one session.
pub fn ensure<H: SessionHost, R: SessionRecords>(
    host: &H,
    records: &R,
    checkout: &Path,
    label: &str,
    layout: &Layout,
) -> Result<Ensured, SessionError> {
    flatten(records.update(|records| {
        prune(host, records)?;
        if let Some(recorded) = records.get(checkout) {
            return Ok(Ensured::Existing(recorded.clone()));
        }
        let created = host.create(label, checkout, layout)?;
        let recorded = Recorded {
            id: created.id,
            label: label.to_owned(),
            tabs: created.tabs,
        };
        records.insert(checkout.to_path_buf(), recorded.clone());
        Ok(Ensured::Created(recorded))
    }))
}

/// Ensures the session of `checkout`, then shows it.
pub fn open<H: SessionHost, R: SessionRecords>(
    host: &H,
    records: &R,
    checkout: &Path,
    label: &str,
    layout: &Layout,
) -> Result<Ensured, SessionError> {
    let ensured = ensure(host, records, checkout, label, layout)?;
    host.show(&ensured.recorded().id)?;
    Ok(ensured)
}

/// Closes the session of `checkout`; `None` when it has none. The record is saved without it
/// before the host closes it, since closing may end this process, and comes back when the host
/// fails.
pub fn close<H: SessionHost, R: SessionRecords>(
    host: &H,
    records: &R,
    checkout: &Path,
) -> Result<Option<Recorded>, SessionError> {
    let removed = flatten(records.update(|records| {
        prune(host, records)?;
        Ok(records.remove(checkout))
    }))?;
    let Some(recorded) = removed else {
        return Ok(None);
    };
    if let Err(error) = host.close(&recorded.id) {
        // The host's error is the one to report; a record that stays removed only costs a
        // second session on the next open.
        let _ = records.update(|records| {
            records
                .entry(checkout.to_path_buf())
                .or_insert_with(|| recorded.clone());
        });
        return Err(error.into());
    }
    Ok(Some(recorded))
}

/// Every checkout with a live session.
pub fn list<H: SessionHost, R: SessionRecords>(
    host: &H,
    records: &R,
) -> Result<BTreeMap<PathBuf, Recorded>, SessionError> {
    flatten(records.update(|records| {
        prune(host, records)?;
        Ok(records.clone())
    }))
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn a_label_names_the_repository_and_the_worktree_in_words() {
        assert_eq!(label("lager", None), "lager");
        assert_eq!(
            label("labels-web", Some("sign_in-page")),
            "labels-web · Sign in page"
        );
        assert_eq!(label("a\tb", Some(" x\n")), "ab ·  x");
    }

    #[test]
    fn a_label_is_cut_to_128_bytes_on_a_character_boundary() {
        let repository = "r".repeat(126);
        assert_eq!(label(&repository, Some("x")), repository);
        let repository = "r".repeat(127);
        assert_eq!(label(&repository, Some("x")), repository);
        let long = label(&"r".repeat(200), None);
        assert_eq!(long.len(), 128);
    }
}
