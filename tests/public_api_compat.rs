//! Compile-only downstream fixture: intentionally uses only the original API.
#![allow(dead_code)]

use std::path::Path;

use lager::application::ports::{
    ConfigStore, EnsureEvent, GitClient, HookRunner, Interaction, InteractionError, Mutation,
    SelectionCandidate, SelectionError,
};
use lager::application::registry::{RegistrationError, register_many};
use lager::application::warehouse::{
    BatchOutcome, CandidateReport, ListReport, ListRow, OperationStatus, PickerContext,
    ProviderError, RemovalBatchOutcome, RemovalOutcome, RemovalStatus, RepositoryOutcome,
};
use lager::domain::config::Config;
use lager::domain::repository::{RepositoryRef, RepositoryRefError, RepositorySummary};
use lager::domain::state::LocalState;

struct OriginalAdapter;

impl ConfigStore for OriginalAdapter {
    type Error = String;
    fn exists(&self, _: &Path) -> bool {
        unimplemented!()
    }
    fn load(&self, _: &Path) -> Result<Config, String> {
        unimplemented!()
    }
    fn init(&self, _: &Path, _: &str, _: bool) -> Result<(), String> {
        unimplemented!()
    }
    fn register(&self, _: &Path, _: &[String], _: Option<&str>) -> Result<Vec<Mutation>, String> {
        unimplemented!()
    }
    fn remove(&self, _: &Path, _: &[String]) -> Result<Vec<Mutation>, String> {
        unimplemented!()
    }
}

impl GitClient for OriginalAdapter {
    type Error = String;
    fn clone_repository(&self, _: &str, _: &Path, _: &Path, _: &Path) -> Result<(), String> {
        unimplemented!()
    }
}

impl HookRunner for OriginalAdapter {
    type Error = String;
    fn run_hook(&self, _: &str, _: &Path) -> Result<(), String> {
        unimplemented!()
    }
}

fn legacy_calls(path: &Path, interaction: &mut dyn Interaction) {
    let _: Result<Vec<Mutation>, RegistrationError> =
        register_many(&OriginalAdapter, path, &[], None, false, interaction);
    let _: Result<Vec<Mutation>, String> = OriginalAdapter.register(path, &[], None);
}

fn original_shapes() {
    let reference = RepositoryRef {
        clone_url: String::new(),
        host: String::new(),
        path: String::new(),
        destination_segments: vec![],
        wildcard: false,
    };
    let _ = RepositorySummary {
        reference: reference.clone(),
        archived: false,
    };
    let candidate = SelectionCandidate {
        reference,
        display: String::new(),
        archived: false,
        exact_path: None,
    };
    let _ = CandidateReport {
        candidates: vec![candidate],
        provider_errors: vec![],
    };
    let row = ListRow {
        identity: String::new(),
        clone_url: String::new(),
        source: String::new(),
        destination: None,
        hook: false,
        archived: false,
        state: None,
        pattern: None,
        exclusions: vec![],
    };
    let _ = ListReport {
        schema_version: 1,
        repositories: vec![row],
        provider_errors: vec![],
    };
    let _ = BatchOutcome {
        outcomes: vec![RepositoryOutcome {
            reference: String::new(),
            status: OperationStatus::Noop,
        }],
        provider_errors: vec![ProviderError {
            provider: String::new(),
            error: String::new(),
        }],
        cancelled: false,
    };
    let _ = RemovalBatchOutcome {
        outcomes: vec![RemovalOutcome {
            reference: String::new(),
            status: RemovalStatus::Absent,
        }],
        cancelled: false,
    };
}

fn exhaustive_original_enums(
    status: OperationStatus,
    removal: RemovalStatus,
    selection: SelectionError,
    registration: RegistrationError,
    state: LocalState,
    mutation: Mutation,
) {
    match status {
        OperationStatus::Cloned
        | OperationStatus::Noop
        | OperationStatus::Hooked
        | OperationStatus::Failed(_) => {}
    }
    match removal {
        RemovalStatus::Removed
        | RemovalStatus::Absent
        | RemovalStatus::Skipped
        | RemovalStatus::Failed(_) => {}
    }
    match selection {
        SelectionError::Cancelled | SelectionError::Unavailable(_) | SelectionError::Failed(_) => {}
    }
    match registration {
        RegistrationError::Cancelled
        | RegistrationError::Interaction(_)
        | RegistrationError::Store(_) => {}
    }
    match state {
        LocalState::Missing
        | LocalState::Cloned
        | LocalState::Conflict
        | LocalState::Unreadable => {}
    }
    match mutation {
        Mutation::Changed | Mutation::Noop => {}
    }
}

fn exhaustive_original_events(
    event: EnsureEvent<'_>,
    context: PickerContext,
    interaction: InteractionError,
    reference: RepositoryRefError,
) {
    match event {
        EnsureEvent::CloneStarted {
            reference: _,
            destination: _,
        }
        | EnsureEvent::CloneSucceeded { reference: _ }
        | EnsureEvent::CloneFailed {
            reference: _,
            error: _,
        } => {}
    }
    match context {
        PickerContext::Register
        | PickerContext::Add
        | PickerContext::Unregister
        | PickerContext::Hook => {}
    }
    match interaction {
        InteractionError::Cancelled | InteractionError::Failed(_) => {}
    }
    match reference {
        RepositoryRefError::Empty | RepositoryRefError::Malformed(_) => {}
    }
}
