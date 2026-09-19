use std::io::{self, IsTerminal};

pub use crate::application::ports::{Interaction, InteractionError};
use crate::presentation::escape;

#[derive(Debug, Default, Clone, Copy)]
pub struct TerminalInteraction;

impl Interaction for TerminalInteraction {
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, InteractionError> {
        require_terminal(io::stdin().is_terminal(), io::stderr().is_terminal())?;
        cliclack::confirm(escape(message))
            .initial_value(default)
            .interact()
            .map_err(map_interaction_error)
    }

    fn input(
        &mut self,
        message: &str,
        placeholder: &str,
        default: Option<&str>,
    ) -> Result<String, InteractionError> {
        require_terminal(io::stdin().is_terminal(), io::stderr().is_terminal())?;
        // Cliclack renders default_input again on submission. Keep the actual
        // default outside the renderer, so Enter returns it without ever drawing
        // its raw controls (or decoding a user's literal escape notation).
        let hint = if placeholder.is_empty() {
            default.unwrap_or("")
        } else {
            placeholder
        };
        let answer: String = cliclack::input(escape(message))
            .placeholder(&escape(hint))
            .required(default.is_none())
            .interact()
            .map_err(map_interaction_error)?;
        Ok(if answer.is_empty() {
            default.unwrap_or("").to_owned()
        } else {
            answer
        })
    }
}

fn require_terminal(
    stdin_is_terminal: bool,
    stderr_is_terminal: bool,
) -> Result<(), InteractionError> {
    if crate::cli::prompt_eligible(stdin_is_terminal, stderr_is_terminal) {
        Ok(())
    } else {
        Err(InteractionError::Failed(
            "interactive prompt requires a TTY".to_owned(),
        ))
    }
}

fn map_interaction_error(error: io::Error) -> InteractionError {
    if error.kind() == io::ErrorKind::Interrupted {
        InteractionError::Cancelled
    } else {
        InteractionError::Failed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{Interaction, InteractionError, require_terminal};

    struct FakeInteraction {
        answers: Vec<Result<bool, InteractionError>>,
        prompts: Vec<(String, bool)>,
    }

    impl Interaction for FakeInteraction {
        fn confirm(&mut self, message: &str, default: bool) -> Result<bool, InteractionError> {
            self.prompts.push((message.to_owned(), default));
            self.answers.remove(0)
        }

        fn input(
            &mut self,
            _message: &str,
            _placeholder: &str,
            _default: Option<&str>,
        ) -> Result<String, InteractionError> {
            unreachable!()
        }
    }

    #[test]
    fn redirected_interaction_is_rejected_before_cliclack_rendering() {
        assert_eq!(
            require_terminal(false, true),
            Err(InteractionError::Failed(
                "interactive prompt requires a TTY".to_owned()
            ))
        );
    }

    #[test]
    fn fake_interaction_records_question_and_default() {
        let mut fake = FakeInteraction {
            answers: vec![Ok(true)],
            prompts: Vec::new(),
        };
        assert_eq!(fake.confirm("register?", true), Ok(true));
        assert_eq!(fake.prompts, [("register?".to_owned(), true)]);
    }

    #[test]
    fn fake_interaction_preserves_cancellation() {
        let mut fake = FakeInteraction {
            answers: vec![Err(InteractionError::Cancelled)],
            prompts: Vec::new(),
        };
        assert_eq!(
            fake.confirm("register?", true),
            Err(InteractionError::Cancelled)
        );
    }
}
