use std::path::Path;

use crate::application::ports::{
    AtomicRegistrationStore, ConfigStore, Interaction, InteractionError, Mutation,
    RegistrationRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    Cancelled,
    Interaction(String),
    Store(String),
}

/// Collect every choice before entering the store's atomic commit boundary.
pub fn register_batch<S: AtomicRegistrationStore>(
    store: &S,
    path: &Path,
    references: &[String],
    supplied_hook: Option<&str>,
    interactive: bool,
    interaction: &mut dyn Interaction,
) -> Result<Vec<Mutation>, RegistrationError> {
    // Validate before prompts can display any input. Keep diagnostics independent
    // of raw references; transport/credential policy belongs in the parser.
    for reference in references {
        crate::domain::repository::RepositoryRef::parse(reference)
            .map_err(|_| RegistrationError::Store("repository reference is invalid".to_owned()))?;
    }
    store
        .load(path)
        .map_err(|error| RegistrationError::Store(error.to_string()))?
        .validate()
        .map_err(|error| RegistrationError::Store(error.to_string()))?;
    let requests = references
        .iter()
        .map(|reference| {
            let post_clone =
                registration_choice(reference, supplied_hook, interactive, interaction).map_err(
                    |error| match error {
                        InteractionError::Cancelled => RegistrationError::Cancelled,
                        InteractionError::Failed(message) => {
                            RegistrationError::Interaction(message)
                        }
                    },
                )?;
            Ok(RegistrationRequest {
                reference: reference.clone(),
                post_clone,
            })
        })
        .collect::<Result<Vec<_>, RegistrationError>>()?;
    store
        .register_batch(path, &requests)
        .map_err(|error| RegistrationError::Store(error.to_string()))
}

/// Legacy incremental registration: earlier writes remain if a later choice or
/// write fails. Use `register_batch` for an all-or-nothing CLI-style operation.
pub fn register_many<S: ConfigStore>(
    store: &S,
    path: &Path,
    references: &[String],
    supplied_hook: Option<&str>,
    interactive: bool,
    interaction: &mut dyn Interaction,
) -> Result<Vec<Mutation>, RegistrationError> {
    let mut mutations = Vec::new();
    for reference in references {
        crate::domain::repository::validate_reference_safety(reference)
            .map_err(|error| RegistrationError::Store(error.to_string()))?;
    }
    for reference in references {
        let post_clone = registration_choice(reference, supplied_hook, interactive, interaction)
            .map_err(|error| match error {
                InteractionError::Cancelled => RegistrationError::Cancelled,
                InteractionError::Failed(message) => RegistrationError::Interaction(message),
            })?;
        mutations.extend(
            store
                .register(path, std::slice::from_ref(reference), post_clone.as_deref())
                .map_err(|error| RegistrationError::Store(error.to_string()))?,
        );
    }
    Ok(mutations)
}

pub fn registration_choice(
    reference: &str,
    supplied_hook: Option<&str>,
    interactive: bool,
    interaction: &mut dyn Interaction,
) -> Result<Option<String>, InteractionError> {
    crate::domain::repository::RepositoryRef::parse(reference)
        .map_err(|error| InteractionError::Failed(error.to_string()))?;
    if let Some(command) = supplied_hook {
        return Ok(Some(command.to_owned()));
    }
    if !interactive
        || !interaction.confirm(
            &format!("Configure a post-clone hook for {reference}?"),
            false,
        )?
    {
        return Ok(None);
    }
    interaction
        .input(
            &format!("Post-clone command for {reference}?"),
            "Command",
            None,
        )
        .map(Some)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::Path;

    use crate::application::ports::{ConfigStore, Interaction, InteractionError, Mutation};
    use crate::domain::config::Config;

    use super::{RegistrationError, register_many, registration_choice};

    #[derive(Default)]
    struct FakeInteraction {
        confirms: VecDeque<Result<bool, InteractionError>>,
        inputs: VecDeque<Result<String, InteractionError>>,
        calls: Vec<String>,
    }

    impl Interaction for FakeInteraction {
        fn confirm(&mut self, message: &str, default: bool) -> Result<bool, InteractionError> {
            self.calls.push(format!("confirm:{message}:{default}"));
            self.confirms.pop_front().expect("confirmation answer")
        }

        fn input(
            &mut self,
            message: &str,
            placeholder: &str,
            default: Option<&str>,
        ) -> Result<String, InteractionError> {
            self.calls
                .push(format!("input:{message}:{placeholder}:{default:?}"));
            self.inputs.pop_front().expect("input answer")
        }
    }

    #[test]
    fn interactive_registration_asks_for_each_hook_with_no_default() {
        let mut interaction = FakeInteraction {
            confirms: [Ok(false), Ok(true)].into(),
            inputs: [Ok("  make setup && printf 'x y'  ".to_owned())].into(),
            ..FakeInteraction::default()
        };

        assert_eq!(
            registration_choice("github.com/org/one", None, true, &mut interaction).unwrap(),
            None
        );
        assert_eq!(
            registration_choice("github.com/org/two", None, true, &mut interaction).unwrap(),
            Some("  make setup && printf 'x y'  ".to_owned())
        );
        assert_eq!(
            interaction.calls,
            [
                "confirm:Configure a post-clone hook for github.com/org/one?:false",
                "confirm:Configure a post-clone hook for github.com/org/two?:false",
                "input:Post-clone command for github.com/org/two?:Command:None"
            ]
        );
    }

    #[test]
    fn explicit_hook_and_non_interactive_registration_do_not_prompt() {
        let mut interaction = FakeInteraction::default();
        assert_eq!(
            registration_choice(
                "github.com/org/repo",
                Some(" printf exact "),
                true,
                &mut interaction
            )
            .unwrap(),
            Some(" printf exact ".to_owned())
        );
        assert_eq!(
            registration_choice("github.com/org/repo", None, false, &mut interaction).unwrap(),
            None
        );
        assert!(interaction.calls.is_empty());
    }

    struct RecordingStore(RefCell<Vec<String>>);

    impl ConfigStore for RecordingStore {
        type Error = String;

        fn exists(&self, _path: &Path) -> bool {
            true
        }
        fn load(&self, _path: &Path) -> Result<Config, Self::Error> {
            unreachable!()
        }
        fn init(&self, _path: &Path, _root: &str, _github: bool) -> Result<(), Self::Error> {
            unreachable!()
        }
        fn register(
            &self,
            _path: &Path,
            references: &[String],
            _post_clone: Option<&str>,
        ) -> Result<Vec<Mutation>, Self::Error> {
            self.0.borrow_mut().extend_from_slice(references);
            Ok(vec![Mutation::Changed; references.len()])
        }
        fn remove(
            &self,
            _path: &Path,
            _references: &[String],
        ) -> Result<Vec<Mutation>, Self::Error> {
            unreachable!()
        }
    }

    struct UnsafeAtomicStore;

    impl ConfigStore for UnsafeAtomicStore {
        type Error = String;
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn load(&self, _: &Path) -> Result<Config, String> {
            Ok(Config {
                root: "repos".into(),
                providers: Default::default(),
                repositories: vec![crate::domain::config::RepositoryDeclaration {
                    url: "ftp://SENTINEL@example.invalid/org/repo".into(),
                    post_clone: None,
                    exclude: vec![],
                }],
            })
        }
        fn init(&self, _: &Path, _: &str, _: bool) -> Result<(), String> {
            unreachable!()
        }
        fn register(
            &self,
            _: &Path,
            _: &[String],
            _: Option<&str>,
        ) -> Result<Vec<Mutation>, String> {
            unreachable!()
        }
        fn remove(&self, _: &Path, _: &[String]) -> Result<Vec<Mutation>, String> {
            unreachable!()
        }
    }

    impl crate::application::ports::AtomicRegistrationStore for UnsafeAtomicStore {
        fn register_batch(
            &self,
            _: &Path,
            _: &[crate::application::ports::RegistrationRequest],
        ) -> Result<Vec<Mutation>, String> {
            panic!("unsafe config reached commit")
        }
    }

    #[test]
    fn atomic_registration_validates_loaded_config_before_interaction() {
        let mut interaction = FakeInteraction::default();
        let result = super::register_batch(
            &UnsafeAtomicStore,
            Path::new("config"),
            &["org/repo".into()],
            None,
            true,
            &mut interaction,
        );
        assert!(matches!(result, Err(RegistrationError::Store(_))));
        assert!(interaction.calls.is_empty());
        assert!(!format!("{result:?}").contains("SENTINEL"));
    }

    #[test]
    fn legacy_registration_keeps_writes_before_ordinary_malformed_input() {
        let store = RecordingStore(RefCell::new(Vec::new()));
        let result = register_many(
            &store,
            Path::new("config"),
            &["org/first".to_owned(), "malformed".to_owned()],
            None,
            false,
            &mut FakeInteraction::default(),
        );
        assert!(result.is_err());
        assert_eq!(store.0.borrow().as_slice(), ["org/first"]);
    }

    #[test]
    fn register_cancellation_stops_before_later_config_mutation() {
        let store = RecordingStore(RefCell::new(Vec::new()));
        let mut interaction = FakeInteraction {
            confirms: [Ok(false), Err(InteractionError::Cancelled), Ok(false)].into(),
            ..FakeInteraction::default()
        };
        let result = register_many(
            &store,
            Path::new("config"),
            &[
                "github.com/org/one".to_owned(),
                "github.com/org/two".to_owned(),
                "github.com/org/three".to_owned(),
            ],
            None,
            true,
            &mut interaction,
        );
        assert_eq!(result, Err(RegistrationError::Cancelled));
        assert_eq!(store.0.borrow().as_slice(), ["github.com/org/one"]);
        assert_eq!(interaction.confirms.len(), 1);
    }

    #[test]
    fn registration_cancellation_is_not_converted_to_an_answer() {
        let mut interaction = FakeInteraction {
            confirms: [Err(InteractionError::Cancelled)].into(),
            ..FakeInteraction::default()
        };
        assert_eq!(
            registration_choice("github.com/org/repo", None, true, &mut interaction),
            Err(InteractionError::Cancelled)
        );
        assert!(interaction.inputs.is_empty());
    }
}
