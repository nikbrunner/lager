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
/// words, with `-` and `_` as spaces and the first letter upper-cased. A ticket key at the
/// start of the name, such as `CP-244`, moves behind the words: `repo · Sign in page · CP-244`.
pub fn label(repository: &str, worktree: Option<&str>) -> String {
    let label = match worktree {
        None => repository.to_owned(),
        Some(worktree) => {
            let (key, rest) = split_ticket_key(worktree);
            let words = rest.replace(['-', '_'], " ");
            let mut letters = words.chars();
            let words: String = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default();
            match (words.is_empty(), key) {
                (_, None) => format!("{repository} · {words}"),
                (true, Some(key)) => format!("{repository} · {key}"),
                (false, Some(key)) => format!("{repository} · {words} · {key}"),
            }
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

/// A leading ticket key `[A-Z][A-Z0-9]*-[0-9]+` that ends the name or is followed by `-` or
/// `_`, and the rest of the name after that separator.
fn split_ticket_key(name: &str) -> (Option<&str>, &str) {
    let bytes = name.as_bytes();
    let prefix = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        .count();
    if prefix == 0 || !bytes[0].is_ascii_uppercase() || bytes.get(prefix) != Some(&b'-') {
        return (None, name);
    }
    let digits = bytes[prefix + 1..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let end = prefix + 1 + digits;
    if digits == 0 || !matches!(bytes.get(end), None | Some(b'-' | b'_')) {
        return (None, name);
    }
    (Some(&name[..end]), name.get(end + 1..).unwrap_or_default())
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
    fn a_leading_ticket_key_moves_behind_the_words() {
        assert_eq!(
            label("core", Some("CP-244-sign-in-page")),
            "core · Sign in page · CP-244"
        );
        assert_eq!(
            label("core", Some("WEBSDK2-228_color_mode")),
            "core · Color mode · WEBSDK2-228"
        );
        assert_eq!(label("core", Some("CP-7")), "core · CP-7");
        assert_eq!(label("core", Some("CP-7-")), "core · CP-7");
    }

    #[test]
    fn a_name_without_a_ticket_key_keeps_its_words() {
        for name in [
            "cp-244-x", "CP-x", "CP244-x", "1P-2-x", "CP-244x", "CP--244",
        ] {
            let words = name.replace(['-', '_'], " ");
            let expected = format!("core · {}{}", words[..1].to_uppercase(), &words[1..]);
            assert_eq!(label("core", Some(name)), expected, "{name}");
        }
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
