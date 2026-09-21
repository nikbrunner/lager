pub mod args;
pub mod controller;
pub mod interaction;
mod inventory;
mod signals;
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
    run_args(Args::parse())
}

/// Run the binary with scoped interruption cleanup for inventory sessions.
pub fn run_binary() -> i32 {
    let args = Args::parse();
    if let args::Command::Inventory(command) = args.command {
        let signals = match signals::InventorySignals::install() {
            Ok(signals) => signals,
            Err(error) => {
                eprintln!("lager: could not install inventory signal handlers: {error}");
                return 1;
            }
        };
        inventory::run(
            &crate::application::ports::config_path(args.config.as_deref()),
            command,
            || signals.exit_code(),
        )
    } else {
        run_args(args)
    }
}

fn run_args(args: Args) -> i32 {
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
