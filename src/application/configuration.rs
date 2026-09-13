use std::path::Path;

use crate::application::ports::{
    ConfigStore, ConfigurationFilesystem, Interaction, InteractionError, Mutation, ToolInspector,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitChoices {
    pub root: String,
    pub create_root: bool,
    pub github: bool,
}

pub fn resolve_init_choices(
    root: Option<&str>,
    create_root: Option<bool>,
    github: Option<bool>,
    interactive: bool,
    interaction: &mut dyn Interaction,
) -> Result<InitChoices, InteractionError> {
    if !interactive && (root.is_none() || create_root.is_none() || github.is_none()) {
        return Err(InteractionError::Failed(
            "init requires one root, create-root choice, and GitHub choice outside a TTY"
                .to_owned(),
        ));
    }
    let root = match root {
        Some(root) => root.to_owned(),
        None => interaction.input(
            "Where should repositories be stored?",
            "repos",
            Some("repos"),
        )?,
    };
    let create_root = match create_root {
        Some(create_root) => create_root,
        None => interaction.confirm("Create the repository root if it is missing?", true)?,
    };
    let github = match github {
        Some(github) => github,
        None => interaction.confirm("Configure GitHub repository discovery?", true)?,
    };
    Ok(InitChoices {
        root,
        create_root,
        github,
    })
}
use crate::domain::config::resolve_portable_path;

pub fn init_diagnostics(tools: &dyn ToolInspector, github: bool) -> Vec<&'static str> {
    let mut messages = Vec::new();
    if !tools.command_available("git") {
        messages.push("git is not available; install git before adding repositories");
    }
    if !tools.command_available("fzf") {
        messages.push("fzf is not available; install fzf for interactive repository selection");
    }
    if github && !tools.github_authenticated() {
        messages.push("authenticated gh is not available; install gh and run `gh auth login`");
    }
    messages
}

pub fn init<S: ConfigStore, F: ConfigurationFilesystem>(
    store: &S,
    filesystem: &F,
    path: &Path,
    root: &str,
    create_root: bool,
    github: bool,
    home: &Path,
) -> Result<(), String> {
    let persisted_root = portable_root(root, home)?;
    resolve_portable_path(&persisted_root, home).map_err(|error| error.to_string())?;
    if store.exists(path) {
        return Err("config already exists".to_owned());
    }
    let root_path =
        resolve_portable_path(&persisted_root, home).map_err(|error| error.to_string())?;
    if create_root {
        filesystem
            .create_dir_all(&root_path)
            .map_err(|error| format!("could not create root: {error}"))?;
    }
    store
        .init(path, &persisted_root, github)
        .map_err(|error| error.to_string())
}

pub fn unregister<S: ConfigStore>(
    store: &S,
    path: &Path,
    references: &[String],
) -> Result<Vec<Mutation>, String> {
    store
        .remove(path, references)
        .map_err(|error| error.to_string())
}

fn portable_root(value: &str, home: &Path) -> Result<String, String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Ok(value.to_owned());
    }
    let relative = path
        .strip_prefix(home)
        .map_err(|_| "--root must be under HOME when persisted".to_owned())?;
    if relative.as_os_str().is_empty() {
        Ok("~".to_owned())
    } else {
        Ok(format!("~/{}", relative.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use super::*;
    use std::collections::VecDeque;

    use crate::application::ports::{
        ConfigStore, ConfigurationFilesystem, Interaction, InteractionError, ToolInspector,
    };
    use crate::domain::config::Config;

    struct Store;

    impl ConfigStore for Store {
        type Error = String;

        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn load(&self, _path: &Path) -> Result<Config, Self::Error> {
            unreachable!()
        }
        fn init(&self, _path: &Path, _root: &str, _github: bool) -> Result<(), Self::Error> {
            Ok(())
        }
        fn register(
            &self,
            _path: &Path,
            _references: &[String],
            _post_clone: Option<&str>,
        ) -> Result<Vec<Mutation>, Self::Error> {
            unreachable!()
        }
        fn remove(
            &self,
            _path: &Path,
            _references: &[String],
        ) -> Result<Vec<Mutation>, Self::Error> {
            unreachable!()
        }
    }

    #[derive(Default)]
    struct RecordingFilesystem(RefCell<Vec<PathBuf>>);

    impl ConfigurationFilesystem for RecordingFilesystem {
        type Error = String;

        fn create_dir_all(&self, path: &Path) -> Result<(), Self::Error> {
            self.0.borrow_mut().push(path.to_path_buf());
            Ok(())
        }
    }

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
    fn interactive_init_asks_only_for_omitted_choices() {
        let mut interaction = FakeInteraction {
            confirms: [Ok(true), Ok(false)].into(),
            inputs: [Ok("work/repos".to_owned())].into(),
            ..FakeInteraction::default()
        };
        let choices = resolve_init_choices(None, None, None, true, &mut interaction).unwrap();
        assert_eq!(choices.root, "work/repos");
        assert!(choices.create_root);
        assert!(!choices.github);
        assert_eq!(
            interaction.calls,
            [
                "input:Where should repositories be stored?:repos:Some(\"repos\")",
                "confirm:Create the repository root if it is missing?:true",
                "confirm:Configure GitHub repository discovery?:true"
            ]
        );
    }

    #[test]
    fn non_interactive_init_rejects_an_omitted_choice_without_prompting() {
        let mut interaction = FakeInteraction::default();
        let error = resolve_init_choices(Some("repos"), Some(true), None, false, &mut interaction)
            .unwrap_err();
        assert!(matches!(
            error,
            InteractionError::Failed(message) if message.contains("outside a TTY")
        ));
        assert!(interaction.calls.is_empty());
    }

    #[test]
    fn init_cancellation_is_preserved() {
        let mut interaction = FakeInteraction {
            inputs: [Err(InteractionError::Cancelled)].into(),
            ..FakeInteraction::default()
        };
        assert_eq!(
            resolve_init_choices(None, None, None, true, &mut interaction),
            Err(InteractionError::Cancelled)
        );
        assert!(interaction.confirms.is_empty());
    }

    struct MissingTools;

    impl ToolInspector for MissingTools {
        fn command_available(&self, _name: &str) -> bool {
            false
        }

        fn github_authenticated(&self) -> bool {
            false
        }
    }

    #[test]
    fn init_diagnostics_report_missing_tools_without_becoming_an_error() {
        assert_eq!(
            init_diagnostics(&MissingTools, true),
            [
                "git is not available; install git before adding repositories",
                "fzf is not available; install fzf for interactive repository selection",
                "authenticated gh is not available; install gh and run `gh auth login`"
            ]
        );
        assert_eq!(init_diagnostics(&MissingTools, false).len(), 2);
    }

    #[test]
    fn init_routes_root_creation_through_the_filesystem_port() {
        let filesystem = RecordingFilesystem::default();
        init(
            &Store,
            &filesystem,
            Path::new("config.toml"),
            "repos/team",
            true,
            false,
            Path::new("/home/test"),
        )
        .unwrap();
        assert_eq!(
            filesystem.0.into_inner(),
            [PathBuf::from("/home/test/repos/team")]
        );
    }
}
