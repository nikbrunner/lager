use std::path::{Path, PathBuf};

use super::args::{
    Args, CloneArgs, Command, EnsureArgs, HookArgs, InitArgs, ListArgs, RemoveArgs, RepositoryArgs,
    UnregisterArgs,
};
use super::interaction::{Interaction, InteractionError};
use crate::application::ports::{
    ConfigStore, EnsureEvent, EnsureReporter, RepositorySelector, SelectionError, ToolInspector,
};
use crate::application::{configuration, ports, warehouse};
use crate::domain::repository::RepositoryRef;
use crate::infrastructure::{
    config::FileConfigStore, git::NativeGit, providers::ConfiguredProviders, shell::Shell,
};

pub fn run(
    args: Args,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    tools: &dyn ToolInspector,
    interactive: bool,
) -> i32 {
    let config_path = ports::config_path(args.config.as_deref());
    match args.command {
        Command::Init(command) => init(&config_path, command, interaction, tools, interactive),
        Command::Register(command) => {
            register(&config_path, command, selector, interaction, interactive)
        }
        Command::Unregister(command) => unregister(&config_path, command, selector),
        Command::Add(command) => add(&config_path, command, selector, interaction, interactive),
        Command::Remove(command) => {
            remove(&config_path, command, selector, interaction, interactive)
        }
        Command::Ensure(command) => ensure(&config_path, command),
        Command::Hook(command) => hook(&config_path, command),
        Command::List(command) => list(&config_path, command),
    }
}

fn init(
    path: &Path,
    args: InitArgs,
    interaction: &mut dyn Interaction,
    tools: &dyn ToolInspector,
    interactive: bool,
) -> i32 {
    let create_root = if args.create_root {
        Some(true)
    } else if args.no_create_root {
        Some(false)
    } else {
        None
    };
    let github = if args.github {
        Some(true)
    } else if args.no_github {
        Some(false)
    } else {
        None
    };
    let choices = match configuration::resolve_init_choices(
        args.root.as_deref(),
        create_root,
        github,
        interactive,
        interaction,
    ) {
        Ok(choices) => choices,
        Err(InteractionError::Cancelled) => return 130,
        Err(InteractionError::Failed(error)) => {
            eprintln!("lager: {error}");
            return if interactive { 1 } else { 2 };
        }
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let store = FileConfigStore;
    let filesystem = crate::infrastructure::filesystem::Filesystem;
    match configuration::init(
        &store,
        &filesystem,
        path,
        &choices.root,
        choices.create_root,
        choices.github,
        &home,
    ) {
        Ok(()) => {
            for message in configuration::init_diagnostics(tools, choices.github) {
                eprintln!("lager: warning: {message}");
            }
            0
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn register(
    path: &Path,
    args: RepositoryArgs,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    let home = home_dir();
    let (repositories, provider_failed) = if args.repositories.is_empty() {
        match pick(
            path,
            warehouse::PickerContext::Register,
            args.include_archived,
            &home,
            selector,
        ) {
            Ok((references, failed)) => (
                references
                    .into_iter()
                    .map(|reference| reference.clone_url)
                    .collect(),
                failed,
            ),
            Err(code) => return code,
        }
    } else {
        (args.repositories, false)
    };
    if repositories.is_empty() {
        return i32::from(provider_failed);
    }
    let store = FileConfigStore;
    match crate::application::registry::register_many(
        &store,
        path,
        &repositories,
        args.post_clone.as_deref(),
        interactive,
        interaction,
    ) {
        Ok(results) => {
            for result in results {
                println!(
                    "{}",
                    match result {
                        ports::Mutation::Changed => "added",
                        ports::Mutation::Noop => "already managed",
                    }
                );
            }
            i32::from(provider_failed)
        }
        Err(crate::application::registry::RegistrationError::Cancelled) => 130,
        Err(
            crate::application::registry::RegistrationError::Interaction(error)
            | crate::application::registry::RegistrationError::Store(error),
        ) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn unregister(path: &Path, args: UnregisterArgs, selector: &dyn RepositorySelector) -> i32 {
    let home = home_dir();
    let (repositories, provider_failed) = if args.repositories.is_empty() {
        match pick(
            path,
            warehouse::PickerContext::Unregister,
            args.include_archived,
            &home,
            selector,
        ) {
            Ok((references, failed)) => (
                references
                    .into_iter()
                    .map(|reference| reference.clone_url)
                    .collect(),
                failed,
            ),
            Err(code) => return code,
        }
    } else {
        (args.repositories, false)
    };
    if repositories.is_empty() {
        return i32::from(provider_failed);
    }
    let store = FileConfigStore;
    match configuration::unregister(&store, path, &repositories) {
        Ok(results) => {
            for result in results {
                println!(
                    "{}",
                    match result {
                        ports::Mutation::Changed => "removed",
                        ports::Mutation::Noop => "already unmanaged",
                    }
                );
            }
            i32::from(provider_failed)
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn add(
    path: &Path,
    args: CloneArgs,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    if args.no_register && args.post_clone.is_some() {
        eprintln!("lager: --post-clone conflicts with --no-register");
        return 2;
    }
    let ask_registration =
        interactive && !args.register && !args.no_register && args.post_clone.is_none();
    if !interactive && !args.register && !args.no_register && args.post_clone.is_none() {
        eprintln!("lager: add requires exactly one of --register or --no-register outside a TTY");
        return 2;
    }
    let register = args.register || args.post_clone.is_some();
    let home = home_dir();
    let selected = if args.repositories.is_empty() {
        match pick(
            path,
            warehouse::PickerContext::Add,
            args.include_archived,
            &home,
            selector,
        ) {
            Ok(result) => Some(result),
            Err(code) => return code,
        }
    } else {
        None
    };
    let provider_failed = selected.as_ref().is_some_and(|(_, failed)| *failed);
    if selected.as_ref().is_some_and(|(items, _)| items.is_empty()) {
        return i32::from(provider_failed);
    }
    let store = FileConfigStore;
    let git = NativeGit;
    let shell = Shell;
    let outcome = if let Some((repositories, _)) = selected {
        if ask_registration {
            warehouse::clone_references_with_interaction(
                &store,
                &git,
                &shell,
                interaction,
                path,
                &repositories,
                &home,
            )
        } else {
            warehouse::clone_references(
                &store,
                &git,
                &shell,
                path,
                &repositories,
                register,
                args.post_clone.as_deref(),
                &home,
            )
        }
    } else if ask_registration {
        warehouse::clone_many_with_interaction(
            &store,
            &git,
            &shell,
            interaction,
            path,
            &args.repositories,
            &home,
        )
    } else {
        warehouse::clone_many(
            &store,
            &git,
            &shell,
            path,
            &args.repositories,
            register,
            args.post_clone.as_deref(),
            &home,
        )
    };
    match outcome {
        Ok(outcome) => {
            for repository in outcome
                .outcomes
                .iter()
                .filter(|item| matches!(item.status, warehouse::OperationStatus::Failed(_)))
            {
                if let warehouse::OperationStatus::Failed(error) = &repository.status {
                    eprintln!("lager: {}: {error}", repository.reference);
                }
            }
            if outcome.cancelled {
                130
            } else {
                i32::from(provider_failed || outcome.failed())
            }
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

struct TerminalEnsureReporter;

impl EnsureReporter for TerminalEnsureReporter {
    fn report(&mut self, event: EnsureEvent<'_>) {
        let result = match event {
            EnsureEvent::CloneStarted {
                reference,
                destination,
            } => cliclack::log::info(format!(
                "Cloning {reference} into {}",
                destination.display()
            )),
            EnsureEvent::CloneSucceeded { reference } => {
                cliclack::log::success(format!("Cloned {reference}"))
            }
            EnsureEvent::CloneFailed { reference, error } => {
                cliclack::log::error(format!("Failed {reference}: {error}"))
            }
        };
        let _ = result;
    }
}

fn ensure(path: &Path, args: EnsureArgs) -> i32 {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let store = FileConfigStore;
    let git = NativeGit;
    let shell = Shell;
    let mut reporter = TerminalEnsureReporter;
    let config = match store.load(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("lager: {error}");
            return 1;
        }
    };
    let providers = match ConfiguredProviders::from_config(&config) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("lager: {error}");
            return 1;
        }
    };
    match warehouse::ensure(
        &store,
        &git,
        &shell,
        &providers,
        path,
        args.include_archived,
        &home,
        &mut reporter,
    ) {
        Ok(outcome) => {
            print_provider_failures(&outcome);
            print_ensure_summary(&outcome);
            i32::from(outcome.failed())
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn print_provider_failures(outcome: &warehouse::BatchOutcome) {
    for error in &outcome.provider_errors {
        eprintln!("lager: {}: {}", error.provider, error.error);
    }
}

fn print_ensure_summary(outcome: &warehouse::BatchOutcome) {
    let cloned = outcome
        .outcomes
        .iter()
        .filter(|item| matches!(item.status, warehouse::OperationStatus::Cloned))
        .count();
    let present = outcome
        .outcomes
        .iter()
        .filter(|item| matches!(item.status, warehouse::OperationStatus::Noop))
        .count();
    let failed = outcome
        .outcomes
        .iter()
        .filter(|item| matches!(item.status, warehouse::OperationStatus::Failed(_)))
        .count()
        + outcome.provider_errors.len();
    let mut parts = Vec::new();
    if cloned > 0 {
        parts.push(format!("{cloned} cloned"));
    }
    if present > 0 {
        parts.push(format!("{present} already present"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    let message = if parts.is_empty() {
        "Nothing to ensure".to_owned()
    } else {
        parts.join(", ")
    };
    let _ = if outcome.failed() {
        cliclack::log::error(message)
    } else {
        cliclack::log::success(message)
    };
}

fn hook(path: &Path, args: HookArgs) -> i32 {
    if args.repositories.is_empty() {
        eprintln!("lager: hook requires a repository reference");
        return 2;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let store = FileConfigStore;
    let git = NativeGit;
    let shell = Shell;
    match warehouse::hook(&store, &git, &shell, path, &args.repositories, &home) {
        Ok(outcome) => {
            for repository in outcome
                .outcomes
                .iter()
                .filter(|item| matches!(item.status, warehouse::OperationStatus::Failed(_)))
            {
                if let warehouse::OperationStatus::Failed(error) = &repository.status {
                    eprintln!("lager: {}: {error}", repository.reference);
                }
            }
            i32::from(outcome.failed())
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn pick(
    path: &Path,
    context: warehouse::PickerContext,
    include_archived: bool,
    home: &Path,
    selector: &dyn RepositorySelector,
) -> Result<(Vec<RepositoryRef>, bool), i32> {
    let store = FileConfigStore;
    let git = NativeGit;
    let config = store.load(path).map_err(|error| {
        eprintln!("lager: {error}");
        1
    })?;
    let providers = ConfiguredProviders::from_config(&config).map_err(|error| {
        eprintln!("lager: {error}");
        1
    })?;
    let report = warehouse::picker_candidates(
        &store,
        &git,
        &providers,
        path,
        context,
        include_archived,
        home,
    )
    .map_err(|error| {
        eprintln!("lager: {error}");
        1
    })?;
    for error in &report.provider_errors {
        eprintln!("lager: {}: {}", error.provider, error.error);
    }
    if report.candidates.is_empty() {
        return Ok((Vec::new(), report.failed()));
    }
    let selected = selector
        .select(&report.candidates)
        .map_err(|error| match error {
            SelectionError::Cancelled => 130,
            SelectionError::Unavailable(message) | SelectionError::Failed(message) => {
                eprintln!("lager: {message}");
                1
            }
        })?;
    Ok((
        selected
            .into_iter()
            .map(|candidate| candidate.reference)
            .collect(),
        report.failed(),
    ))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn list(path: &Path, args: ListArgs) -> i32 {
    if args.include_archived && !args.remote {
        eprintln!("lager: --include-archived requires --remote");
        return 2;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let store = FileConfigStore;
    let git = NativeGit;
    let config = match store.load(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("lager: {error}");
            return 1;
        }
    };
    let providers = match ConfiguredProviders::from_config(&config) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("lager: {error}");
            return 1;
        }
    };
    match warehouse::list(
        &store,
        &git,
        &providers,
        path,
        args.remote,
        args.include_archived,
        &home,
    ) {
        Ok(report) => {
            if args.json {
                match serde_json::to_string(&report) {
                    Ok(document) => println!("{document}"),
                    Err(error) => {
                        eprintln!("lager: could not render list: {error}");
                        return 1;
                    }
                }
            } else {
                render_list(&report);
            }
            for error in &report.provider_errors {
                eprintln!("lager: {}: {}", error.provider, error.error);
            }
            i32::from(report.failed())
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

fn render_list(report: &warehouse::ListReport) {
    for row in &report.repositories {
        let state = row
            .state
            .map(|state| format!("{state:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "wildcard".to_owned());
        let destination = row.destination.as_deref().unwrap_or("-");
        let archived = if row.archived { " [archived]" } else { "" };
        println!("{}\t{}\t{}{}", row.identity, state, destination, archived);
    }
}

fn remove(
    path: &Path,
    args: RemoveArgs,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    if !interactive && (!args.unregister && !args.keep_registered || !args.yes) {
        eprintln!(
            "lager: remove requires exactly one of --unregister or --keep-registered and --yes outside a TTY"
        );
        return 2;
    }
    let unregister = if args.unregister || args.keep_registered {
        Some(args.unregister)
    } else {
        None
    };
    let home = home_dir();
    let store = FileConfigStore;
    let git = NativeGit;
    let filesystem = crate::infrastructure::filesystem::Filesystem;
    let selected = if args.repositories.is_empty() {
        match warehouse::remove_candidates(&store, &git, &filesystem, path, &home) {
            Ok(report) => {
                if report.candidates.is_empty() {
                    return 0;
                }
                match selector.select(&report.candidates) {
                    Ok(selected) => Some(selected),
                    Err(SelectionError::Cancelled) => return 130,
                    Err(SelectionError::Unavailable(error) | SelectionError::Failed(error)) => {
                        eprintln!("lager: {error}");
                        return 1;
                    }
                }
            }
            Err(error) => {
                eprintln!("lager: {error}");
                return 1;
            }
        }
    } else {
        None
    };
    if selected.as_ref().is_some_and(Vec::is_empty) {
        return 0;
    }
    let outcome = if let Some(selections) = selected {
        warehouse::remove_selected_many(
            &store,
            &git,
            &filesystem,
            interaction,
            path,
            &selections,
            unregister,
            args.yes,
            args.force,
            interactive,
            &home,
        )
    } else {
        warehouse::remove_many(
            &store,
            &git,
            &filesystem,
            interaction,
            path,
            &args.repositories,
            unregister,
            args.yes,
            args.force,
            interactive,
            &home,
        )
    };
    match outcome {
        Ok(outcome) => {
            for repository in &outcome.outcomes {
                match &repository.status {
                    warehouse::RemovalStatus::Failed(error) => {
                        eprintln!("lager: {}: {error}", repository.reference)
                    }
                    warehouse::RemovalStatus::Removed => {
                        println!("{}: removed", repository.reference)
                    }
                    warehouse::RemovalStatus::Absent => {
                        println!("{}: absent", repository.reference)
                    }
                    warehouse::RemovalStatus::Skipped => {
                        println!("{}: skipped", repository.reference)
                    }
                }
            }
            if outcome.cancelled {
                130
            } else {
                i32::from(outcome.failed())
            }
        }
        Err(error) => {
            eprintln!("lager: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use clap::Parser;

    use super::*;
    use crate::application::ports::{SelectionCandidate, ToolInspector};

    #[derive(Default)]
    struct FakeInteraction {
        calls: usize,
        confirmations: VecDeque<bool>,
        input_default: Option<String>,
    }

    impl Interaction for FakeInteraction {
        fn confirm(&mut self, _message: &str, _default: bool) -> Result<bool, InteractionError> {
            self.calls += 1;
            Ok(self.confirmations.pop_front().unwrap_or(true))
        }

        fn input(
            &mut self,
            _message: &str,
            _placeholder: &str,
            default: Option<&str>,
        ) -> Result<String, InteractionError> {
            self.calls += 1;
            self.input_default = default.map(str::to_owned);
            Ok(default.unwrap_or("entered").to_owned())
        }
    }

    struct UnusedSelector;

    impl RepositorySelector for UnusedSelector {
        fn select(
            &self,
            _candidates: &[SelectionCandidate],
        ) -> Result<Vec<SelectionCandidate>, SelectionError> {
            unreachable!()
        }
    }

    struct AvailableTools;

    impl ToolInspector for AvailableTools {
        fn command_available(&self, _name: &str) -> bool {
            true
        }

        fn github_authenticated(&self) -> bool {
            true
        }
    }

    #[test]
    fn interactive_controller_passes_the_real_repos_default_without_a_pty() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let args = Args::parse_from(["lager", "--config", config.to_str().unwrap(), "init"]);
        let mut interaction = FakeInteraction {
            confirmations: [false, false].into(),
            ..FakeInteraction::default()
        };

        assert_eq!(
            run(
                args,
                &UnusedSelector,
                &mut interaction,
                &AvailableTools,
                true
            ),
            0
        );
        assert_eq!(interaction.input_default.as_deref(), Some("repos"));
        assert!(
            std::fs::read_to_string(config)
                .unwrap()
                .contains("root = \"repos\"")
        );
    }

    #[test]
    fn ineligible_descriptors_take_init_non_interactive_usage_path_before_prompting() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let args = Args::parse_from(["lager", "--config", config.to_str().unwrap(), "init"]);
        let mut interaction = FakeInteraction::default();

        assert_eq!(
            run(
                args,
                &UnusedSelector,
                &mut interaction,
                &AvailableTools,
                false
            ),
            2
        );
        assert_eq!(interaction.calls, 0);
        assert!(!config.exists());
    }
}
