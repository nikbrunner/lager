pub mod args;
pub mod controller;
pub mod interaction;
use std::io::IsTerminal;

use crate::infrastructure::{fzf::Fzf, tools::NativeTools};
use args::Args;
use clap::Parser;
use interaction::TerminalInteraction;

pub(crate) fn prompt_eligible(stdin_is_terminal: bool, stderr_is_terminal: bool) -> bool {
    stdin_is_terminal && stderr_is_terminal
}

/// Run with the embedding process's signal policy; installs no signal handlers.
pub fn run() -> i32 {
    let args = Args::parse();
    let selector = Fzf::default();
    let mut interaction = TerminalInteraction;
    controller::run(
        args,
        &selector,
        &mut interaction,
        &NativeTools,
        prompt_eligible(
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal(),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::prompt_eligible;

    #[test]
    fn prompts_require_both_input_and_render_terminals() {
        assert!(prompt_eligible(true, true));
        assert!(!prompt_eligible(true, false));
        assert!(!prompt_eligible(false, true));
        assert!(!prompt_eligible(false, false));
    }
}
