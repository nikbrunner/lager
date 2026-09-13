use std::path::Path;

use crate::application::ports::{ConfigStore, Interaction, InteractionError, Mutation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    Cancelled,
    Interaction(String),
    Store(String),
}

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
