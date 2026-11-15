use std::path::{Path, PathBuf};

use super::args::{
    ArchiveArgs, Args, CloneArgs, Command, EnsureArgs, HookArgs, InitArgs, InventoryArgs,
    RemoveArgs, RenameArgs, RepositoryArgs, UnregisterArgs, VisibilityArgs, VisibilityValue,
};
use super::interaction::{Interaction, InteractionError};
use crate::application::ports::{
    ConfigStore, EnsureEvent, EnsureReporter, RepositorySelector, SelectionError, ToolInspector,
};
use crate::application::remote::ActionError;
use crate::application::{configuration, ports, remote, warehouse};
use crate::domain::repository::RepositoryRef;
use crate::infrastructure::{
    config::{CommandConfigStore, FileConfigStore},
    filesystem::Filesystem,
    git::NativeGit,
    providers::ConfiguredProviders,
    shell::Shell,
};
use crate::presentation::escape;

fn displayed_reference(reference: &str) -> String {
    if RepositoryRef::parse(reference).is_ok() {
        escape(reference)
    } else {
        "invalid repository reference".to_owned()
    }
}

pub fn run(
    args: Args,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    tools: &dyn ToolInspector,
    interactive: bool,
) -> i32 {
    let config_path = ports::config_path(args.config.as_deref());
    let references = match &args.command {
        Command::Register(command) => command.repositories.as_slice(),
        Command::Unregister(command) => command.repositories.as_slice(),
        Command::Add(command) => command.repositories.as_slice(),
        Command::Remove(command) => command.repositories.as_slice(),
        Command::Hook(command) => command.repositories.as_slice(),
        Command::Archive(command) => std::slice::from_ref(&command.repository),
        Command::Rename(command) => std::slice::from_ref(&command.repository),
        Command::Visibility(command) => std::slice::from_ref(&command.repository),
        _ => &[],
    };
    for reference in references {
        if let Err(error) = crate::domain::repository::validate_reference_safety(reference) {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    }
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
        Command::Hook(command) => hook(&config_path, command, selector),
        Command::Archive(command) => archive(&config_path, command),
        Command::Rename(command) => rename(&config_path, command, interaction, interactive),
        Command::Visibility(command) => visibility(&config_path, command, interaction, interactive),
        Command::Status(command) => super::inventory::status::run(&config_path, command),
        Command::Inventory(command) => inventory(&config_path, command),
        Command::Worktree(command) => {
            super::worktree::run(&config_path, command, interaction, interactive)
        }
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
            eprintln!("lager: {}", escape(error));
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
                eprintln!("lager: warning: {}", escape(message));
            }
            0
        }
        Err(error) => {
            eprintln!("lager: {}", escape(error));
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
    let store = CommandConfigStore::default();
    let home = home_dir();
    let (repositories, provider_failed) = if args.repositories.is_empty() {
        match pick(
            &store,
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
    match crate::application::registry::register_batch(
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
            eprintln!("lager: {}", escape(error));
            1
        }
    }
}

fn unregister(path: &Path, args: UnregisterArgs, selector: &dyn RepositorySelector) -> i32 {
    let store = CommandConfigStore::default();
    let home = home_dir();
    let (repositories, provider_failed) = if args.repositories.is_empty() {
        match pick(
            &store,
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
            eprintln!("lager: {}", escape(error));
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
    let mut ask_registration =
        interactive && !args.register && !args.no_register && args.post_clone.is_none();
    if !interactive && !args.register && !args.no_register && args.post_clone.is_none() {
        eprintln!("lager: add requires exactly one of --register or --no-register outside a TTY");
        return 2;
    }
    let mut register = args.register || args.post_clone.is_some();
    let mut post_clone = args.post_clone.clone();
    let target = if args.create {
        match create_target(path, &args) {
            Ok(target) => Some(target),
            Err(code) => return code,
        }
    } else {
        None
    };
    // Asked before GitHub is touched, so cancelling creates nothing.
    if target.is_some() && ask_registration {
        (register, post_clone) =
            match registration_before_create(interaction, &args.repositories[0]) {
                Ok(choice) => choice,
                Err(code) => return code,
            };
        ask_registration = false;
    }
    let store = CommandConfigStore::default();
    let home = home_dir();
    let created = match target {
        Some(target) => match create(&args, target, &home) {
            Ok(created) => Some(created),
            Err(code) => return code,
        },
        None => None,
    };
    if let (Some((_, destination)), Some(source)) = (&created, &args.from) {
        let result = remote::settle_published(
            &store,
            &Shell,
            path,
            &args.repositories[0],
            register,
            post_clone.as_deref(),
            source,
            destination,
        );
        return match result {
            Ok(()) => 0,
            Err(error) => report_remote(&args.repositories[0], Err(error)),
        };
    }
    let selected = if args.repositories.is_empty() {
        match pick(
            &store,
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
    let git = NativeGit;
    let shell = Shell;
    let mut reporter = TerminalEnsureReporter::default();
    let outcome = if let Some((repositories, _)) = selected {
        let repositories = repositories
            .iter()
            .map(|reference| reference.clone_url.clone())
            .collect::<Vec<_>>();
        if ask_registration {
            warehouse::clone_many_with_interaction_reporter(
                &store,
                &git,
                &shell,
                interaction,
                path,
                &repositories,
                &home,
                &mut reporter,
            )
        } else {
            warehouse::clone_many_with_reporter(
                &store,
                &git,
                &shell,
                path,
                &repositories,
                register,
                post_clone.as_deref(),
                &home,
                &mut reporter,
            )
        }
    } else if ask_registration {
        warehouse::clone_many_with_interaction_reporter(
            &store,
            &git,
            &shell,
            interaction,
            path,
            &args.repositories,
            &home,
            &mut reporter,
        )
    } else {
        warehouse::clone_many_with_reporter(
            &store,
            &git,
            &shell,
            path,
            &args.repositories,
            register,
            post_clone.as_deref(),
            &home,
            &mut reporter,
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
                    let error = match &created {
                        // clone_repository removes the destination when the clone fails.
                        Some((identity, destination)) if !destination.exists() => format!(
                            "created {identity}; clone failed: {error}; rerun `lager add {identity}` without --create"
                        ),
                        Some((identity, _)) => format!("created {identity}; {error}"),
                        None => error.clone(),
                    };
                    eprintln!(
                        "lager: {}: {}",
                        escape(&repository.reference),
                        escape(error)
                    );
                }
            }
            if outcome.cancelled {
                130
            } else {
                i32::from(provider_failed || outcome.failed())
            }
        }
        Err(error) => {
            let error = match &created {
                Some((identity, _)) => format!("created {identity}; {error}"),
                None => error,
            };
            eprintln!("lager: {}", escape(error));
            1
        }
    }
}

/// The answers `add` would ask for after cloning: whether to register, and which post_clone.
fn registration_before_create(
    interaction: &mut dyn Interaction,
    repository: &str,
) -> Result<(bool, Option<String>), i32> {
    interaction
        .confirm(&format!("Register {repository}?"), true)
        .and_then(|register| {
            if register {
                crate::application::registry::registration_choice(
                    repository,
                    None,
                    true,
                    interaction,
                )
                .map(|post_clone| (true, post_clone))
            } else {
                Ok((false, None))
            }
        })
        .map_err(|error| match error {
            InteractionError::Cancelled => 130,
            InteractionError::Failed(error) => {
                eprintln!("lager: {}", escape(error));
                1
            }
        })
}

/// The one repository `add --create` names, once its provider allows `create`; asks nothing of
/// GitHub.
fn create_target(path: &Path, args: &CloneArgs) -> Result<RemoteTarget, i32> {
    let [repository] = args.repositories.as_slice() else {
        eprintln!("lager: --create needs exactly one repository reference");
        return Err(2);
    };
    let (config, providers, reference) = remote_target(path, repository).map_err(|error| {
        eprintln!("lager: {}", escape(error));
        1
    })?;
    if reference.is_wildcard() {
        eprintln!("lager: --create needs one repository, not a wildcard");
        return Err(2);
    }
    remote::eligible(
        &config,
        &reference,
        crate::domain::config::RemoteAction::Create,
    )
    .map_err(|error| report_remote(repository, Err(error.into())))?;
    Ok((config, providers, reference))
}

/// Creates the repository `create_target` checked; returns its identity and checkout path.
fn create(
    args: &CloneArgs,
    (config, providers, reference): RemoteTarget,
    home: &Path,
) -> Result<(String, PathBuf), i32> {
    let repository = &args.repositories[0];
    let visibility = if args.public {
        ports::Visibility::Public
    } else {
        ports::Visibility::Private
    };
    let result = match &args.from {
        Some(source) => remote::publish(
            &config,
            &providers,
            &NativeGit,
            &Filesystem,
            &reference,
            visibility,
            args.description.as_deref(),
            source,
            home,
        ),
        None => remote::create(
            &config,
            &providers,
            &NativeGit,
            &reference,
            visibility,
            args.description.as_deref(),
            home,
        ),
    };
    let destination = match result {
        Ok(destination) => destination,
        Err(error) => return Err(report_remote(repository, Err(error))),
    };
    let outcome = if args.from.is_some() {
        "published"
    } else {
        "created"
    };
    report_remote(repository, Ok(outcome.to_owned()));
    Ok((reference.identity(), destination))
}

#[derive(Default)]
struct TerminalEnsureReporter {
    destination: Option<PathBuf>,
    skipped: usize,
}

impl EnsureReporter for TerminalEnsureReporter {
    fn report(&mut self, event: EnsureEvent<'_>) {
        let result = match event {
            EnsureEvent::CloneStarted {
                reference,
                destination,
            } => {
                self.destination = Some(destination.to_path_buf());
                cliclack::log::info(format!(
                    "Cloning {} into {}",
                    escape(reference),
                    escape(destination.display())
                ))
            }
            EnsureEvent::CloneSucceeded { reference } => {
                let destination = self
                    .destination
                    .as_deref()
                    .map(|destination| format!(" into {}", escape(destination.display())))
                    .unwrap_or_default();
                cliclack::log::success(format!("Cloned {}{destination}", escape(reference)))
            }
            EnsureEvent::CloneFailed { reference, error } => {
                cliclack::log::error(format!("Failed {}: {}", escape(reference), escape(error)))
            }
        };
        let _ = result;
    }

    fn skipped(&mut self, reference: &str, note: &str) {
        self.skipped += 1;
        let _ = cliclack::log::warning(format!("{}: skipped, {}", escape(reference), escape(note)));
    }
}

fn ensure(path: &Path, args: EnsureArgs) -> i32 {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let store = CommandConfigStore::default();
    let git = NativeGit;
    let shell = Shell;
    let mut reporter = TerminalEnsureReporter::default();
    let config = match store.load(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let providers = match ConfiguredProviders::from_config(&config) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
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
            print_ensure_summary(&outcome, reporter.skipped);
            if outcome.cancelled {
                130
            } else {
                i32::from(outcome.failed())
            }
        }
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            1
        }
    }
}

fn print_provider_failures(outcome: &warehouse::BatchOutcome) {
    for error in &outcome.provider_errors {
        eprintln!(
            "lager: {}: {}",
            escape(&error.provider),
            escape(&error.error)
        );
    }
}

fn print_ensure_summary(outcome: &warehouse::BatchOutcome, skipped: usize) {
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
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
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

fn hook(path: &Path, args: HookArgs, selector: &dyn RepositorySelector) -> i32 {
    let store = CommandConfigStore::default();
    let home = home_dir();
    let (repositories, picker_failed) = if args.repositories.is_empty() {
        match pick(
            &store,
            path,
            warehouse::PickerContext::Hook,
            false,
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
    let git = NativeGit;
    let shell = Shell;
    match warehouse::hook(&store, &git, &shell, path, &repositories, &home) {
        Ok(outcome) => {
            for repository in &outcome.outcomes {
                match &repository.status {
                    warehouse::OperationStatus::Failed(error) => {
                        eprintln!(
                            "lager: {}: {}",
                            displayed_reference(&repository.reference),
                            escape(error)
                        );
                    }
                    warehouse::OperationStatus::Noop => {
                        eprintln!(
                            "lager: {}: no post-clone hook configured",
                            escape(&repository.reference)
                        );
                    }
                    _ => {}
                }
            }
            if outcome.cancelled {
                130
            } else {
                i32::from(outcome.failed() || picker_failed)
            }
        }
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            1
        }
    }
}

fn pick(
    store: &CommandConfigStore,
    path: &Path,
    context: warehouse::PickerContext,
    include_archived: bool,
    home: &Path,
    selector: &dyn RepositorySelector,
) -> Result<(Vec<RepositoryRef>, bool), i32> {
    let git = NativeGit;
    let config = store.load(path).map_err(|error| {
        eprintln!("lager: {}", escape(error));
        1
    })?;
    let providers = ConfiguredProviders::from_config(&config).map_err(|error| {
        eprintln!("lager: {}", escape(error));
        1
    })?;
    let report = warehouse::picker_candidates(
        store,
        &git,
        &providers,
        path,
        context,
        include_archived,
        home,
    )
    .map_err(|error| {
        eprintln!("lager: {}", escape(error));
        1
    })?;
    for error in &report.provider_errors {
        eprintln!(
            "lager: {}: {}",
            escape(&error.provider),
            escape(&error.error)
        );
    }
    if report.candidates.is_empty() {
        return Ok((Vec::new(), report.failed()));
    }
    let selected = selector
        .select(&report.candidates)
        .map_err(|error| match error {
            SelectionError::Cancelled => 130,
            SelectionError::Unavailable(message) | SelectionError::Failed(message) => {
                eprintln!("lager: {}", escape(message));
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

/// The configuration, its providers and the parsed reference a remote action works on.
type RemoteTarget = (
    crate::domain::config::Config,
    ConfiguredProviders,
    RepositoryRef,
);

fn remote_target(path: &Path, repository: &str) -> Result<RemoteTarget, String> {
    let config = CommandConfigStore::default()
        .load(path)
        .map_err(|error| error.to_string())?;
    let providers = ConfiguredProviders::from_config(&config)?;
    let reference = RepositoryRef::parse(repository).map_err(|error| error.to_string())?;
    Ok((config, providers, reference))
}

/// Prints `<repository>: <outcome>` and returns the exit code.
fn report_remote(repository: &str, result: Result<String, ActionError>) -> i32 {
    match result {
        Ok(outcome) => {
            println!("{}: {outcome}", escape(repository));
            0
        }
        Err(ActionError::Declined) => {
            println!("{}: skipped", escape(repository));
            0
        }
        Err(ActionError::Cancelled) => 130,
        Err(ActionError::Failed(error)) => {
            eprintln!(
                "lager: {}: {}",
                displayed_reference(repository),
                escape(error)
            );
            1
        }
    }
}

fn archive(path: &Path, args: ArchiveArgs) -> i32 {
    let (config, providers, reference) = match remote_target(path, &args.repository) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let result = remote::archive(&config, &providers, &reference, !args.undo)
        .map(|mutation| remote::Done::Archived(!args.undo, mutation).outcome());
    report_remote(&args.repository, result)
}

fn visibility(
    path: &Path,
    args: VisibilityArgs,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    let target = match args.value {
        VisibilityValue::Public => ports::Visibility::Public,
        VisibilityValue::Private => ports::Visibility::Private,
    };
    if !interactive && !args.yes {
        eprintln!(
            "lager: visibility {} requires --yes outside a TTY",
            target.as_str()
        );
        return 2;
    }
    let (config, providers, reference) = match remote_target(path, &args.repository) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let result = remote::visibility(
        &config,
        &providers,
        interaction,
        &reference,
        target,
        args.yes,
    )
    .map(|mutation| remote::Done::Visibility(target, mutation).outcome());
    report_remote(&args.repository, result)
}

fn rename(
    path: &Path,
    args: RenameArgs,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    if !interactive && !args.yes {
        eprintln!("lager: rename requires --yes outside a TTY");
        return 2;
    }
    if !remote::valid_name(&args.new_name) {
        eprintln!(
            "lager: invalid repository name `{}`; use letters, digits, -, _ and . without a leading -",
            escape(&args.new_name)
        );
        return 2;
    }
    let (config, providers, reference) = match remote_target(path, &args.repository) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let result = remote::rename(
        &config,
        &providers,
        &CommandConfigStore::default(),
        &NativeGit,
        &Filesystem,
        interaction,
        path,
        &reference,
        &args.new_name,
        args.yes,
        &home_dir(),
    )
    .map(|renamed| remote::Done::Renamed(renamed).outcome());
    report_remote(&args.repository, result)
}

fn inventory(path: &Path, args: InventoryArgs) -> i32 {
    super::inventory::run(path, args, || None)
}

fn remove(
    path: &Path,
    args: RemoveArgs,
    selector: &dyn RepositorySelector,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    if args.remote {
        return delete(path, args, interaction, interactive);
    }
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
    let store = CommandConfigStore::default();
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
                        eprintln!("lager: {}", escape(error));
                        return 1;
                    }
                }
            }
            Err(error) => {
                eprintln!("lager: {}", escape(error));
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
                        eprintln!(
                            "lager: {}: {}",
                            displayed_reference(&repository.reference),
                            escape(error)
                        )
                    }
                    warehouse::RemovalStatus::Removed => {
                        println!("{}: removed", escape(&repository.reference))
                    }
                    warehouse::RemovalStatus::Absent => {
                        println!("{}: absent", escape(&repository.reference))
                    }
                    warehouse::RemovalStatus::Skipped => {
                        println!("{}: skipped", escape(&repository.reference))
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
            eprintln!("lager: {}", escape(error));
            1
        }
    }
}

/// `remove --remote`: deletes one repository on GitHub, then its checkout and declaration.
fn delete(
    path: &Path,
    args: RemoveArgs,
    interaction: &mut dyn Interaction,
    interactive: bool,
) -> i32 {
    let [repository] = args.repositories.as_slice() else {
        eprintln!("lager: --remote needs exactly one repository reference");
        return 2;
    };
    if !interactive && !args.yes {
        eprintln!("lager: remove --remote requires --yes outside a TTY");
        return 2;
    }
    let (config, providers, reference) = match remote_target(path, repository) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("lager: {}", escape(error));
            return 1;
        }
    };
    let result = remote::delete(
        &config,
        &providers,
        &CommandConfigStore::default(),
        &NativeGit,
        &Filesystem,
        interaction,
        path,
        &reference,
        args.yes,
        args.force,
        &home_dir(),
    )
    .map(|deleted| remote::Done::Deleted(deleted).outcome());
    report_remote(repository, result)
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
    fn add_create_refuses_a_disabled_create_before_asking_anything() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        std::fs::write(
            &config,
            "root = \"repos\"\n[providers.\"github.com\"]\npreset = \"github\"\nremote_actions = [\"archive\"]\n",
        )
        .unwrap();
        let args = Args::parse_from([
            "lager",
            "--config",
            config.to_str().unwrap(),
            "add",
            "org/demo",
            "--create",
        ]);
        let mut interaction = FakeInteraction::default();
        assert_eq!(
            run(
                args,
                &UnusedSelector,
                &mut interaction,
                &AvailableTools,
                true
            ),
            1
        );
        assert_eq!(interaction.calls, 0);
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
