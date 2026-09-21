use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::infrastructure::config::InventoryKeyOverrides;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Mode {
    Normal,
    Menu,
    Search,
    Input,
    Confirmation,
    Inspection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Accept,
    Cancel,
    Help,
    Quit,
    Refresh,
    Search,
    ClearSearch,
    Inspect,
    Menu,
    Add,
    Remove,
    Mark,
    Register,
    Unregister,
    Clone,
    CloneRegister,
    RemoveUnregister,
    Copy,
    Open,
    Remote,
    Retry,
    Archives,
}

impl Action {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
            Self::Accept => "accept",
            Self::Cancel => "cancel",
            Self::Help => "help",
            Self::Quit => "quit",
            Self::Refresh => "refresh",
            Self::Search => "search",
            Self::ClearSearch => "clear_search",
            Self::Inspect => "inspect",
            Self::Menu => "menu",
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Mark => "mark",
            Self::Register => "register",
            Self::Unregister => "unregister",
            Self::Clone => "clone",
            Self::CloneRegister => "clone_register",
            Self::RemoveUnregister => "remove_unregister",
            Self::Copy => "copy",
            Self::Open => "open",
            Self::Remote => "remote",
            Self::Retry => "retry",
            Self::Archives => "archives",
        }
    }

    pub(super) fn available(self, mode: Mode) -> bool {
        match mode {
            Mode::Normal => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::Refresh
                    | Self::Help
                    | Self::Quit
                    | Self::Search
                    | Self::ClearSearch
            ),
            Mode::Search => matches!(self, Self::Accept | Self::Cancel | Self::Help | Self::Quit),
            Mode::Inspection => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::PageUp
                    | Self::PageDown
                    | Self::Accept
                    | Self::Cancel
                    | Self::Help
                    | Self::Quit
            ),
            _ => false,
        }
    }
}

pub(super) struct Bindings(BTreeMap<Mode, BTreeMap<Action, Vec<(KeyEvent, String)>>>);

impl Bindings {
    pub(super) fn load(overrides: InventoryKeyOverrides) -> Result<Self, String> {
        use Action::*;
        let modes = [
            ("normal", Mode::Normal),
            ("menu", Mode::Menu),
            ("search", Mode::Search),
            ("input", Mode::Input),
            ("confirmation", Mode::Confirmation),
            ("inspection", Mode::Inspection),
        ];
        for mode in overrides.keys() {
            if !modes.iter().any(|(name, _)| name == mode) {
                return Err(format!("unknown inventory key mode `{mode}`"));
            }
        }
        let mut maps = BTreeMap::new();
        for (name, mode) in modes {
            let typing = matches!(mode, Mode::Search | Mode::Input);
            let mut defaults: Vec<(Action, Vec<&str>)> = vec![
                (Help, vec![if typing { "F1" } else { "?" }]),
                (Quit, vec![if typing { "Ctrl+q" } else { "q" }]),
            ];
            if mode == Mode::Normal {
                defaults.extend([
                    (Up, vec!["k", "Up"]),
                    (Down, vec!["j", "Down"]),
                    (Quit, vec!["q", "Esc"]),
                    (Refresh, vec!["R"]),
                    (Search, vec!["/"]),
                    (ClearSearch, vec!["Backspace"]),
                    (Inspect, vec!["Enter"]),
                    (Menu, vec!["m"]),
                    (Add, vec!["a"]),
                    (Remove, vec!["r"]),
                    (Mark, vec!["Space"]),
                    (Register, vec![]),
                    (Unregister, vec![]),
                    (Clone, vec![]),
                    (CloneRegister, vec![]),
                    (RemoveUnregister, vec![]),
                    (Copy, vec![]),
                    (Open, vec![]),
                    (Remote, vec![]),
                    (Retry, vec![]),
                    (Archives, vec![]),
                ]);
            } else {
                defaults.extend([(Accept, vec!["Enter"]), (Cancel, vec!["Esc"])]);
                if !typing {
                    defaults.extend([
                        (Up, vec!["k", "Up", "Shift+Tab"]),
                        (Down, vec!["j", "Down", "Tab"]),
                        (PageUp, vec!["PageUp"]),
                        (PageDown, vec!["PageDown"]),
                    ]);
                }
            }
            let mut map: BTreeMap<_, _> = defaults
                .into_iter()
                .map(|(action, keys)| {
                    (
                        action,
                        keys.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    )
                })
                .collect();
            if let Some(custom) = overrides.get(name) {
                for (action_name, keys) in custom {
                    let Some(action) = map
                        .keys()
                        .copied()
                        .find(|action| action.name() == action_name)
                    else {
                        return Err(format!("unknown inventory action `{name}.{action_name}`"));
                    };
                    map.insert(action, keys.clone());
                }
            }
            for required in if mode == Mode::Normal {
                &[Help, Quit][..]
            } else {
                &[Accept, Cancel, Help, Quit][..]
            } {
                if map[required].is_empty() {
                    return Err(format!(
                        "inventory {name}.{} must remain reachable",
                        required.name()
                    ));
                }
            }
            let mut parsed = BTreeMap::new();
            let mut used = Vec::new();
            for (action, keys) in map {
                let mut bindings = Vec::new();
                for key in keys {
                    let event = parse_key(&key)?;
                    if typing
                        && (matches!(event.code, KeyCode::Char(_))
                            && !event.modifiers.contains(KeyModifiers::CONTROL)
                            || matches!(
                                event.code,
                                KeyCode::Backspace
                                    | KeyCode::Delete
                                    | KeyCode::Left
                                    | KeyCode::Right
                                    | KeyCode::Home
                                    | KeyCode::End
                            ))
                    {
                        return Err(format!(
                            "inventory {name} binding `{key}` must preserve ordinary input"
                        ));
                    }
                    if used.contains(&event) {
                        return Err(format!("inventory {name} key collision: `{key}`"));
                    }
                    used.push(event);
                    bindings.push((event, key));
                }
                parsed.insert(action, bindings);
            }
            maps.insert(mode, parsed);
        }
        Ok(Self(maps))
    }

    pub(super) fn action(&self, mode: Mode, key: KeyEvent) -> Option<Action> {
        let key = normalized(key);
        self.0[&mode].iter().find_map(|(action, keys)| {
            (action.available(mode) && keys.iter().any(|(event, _)| *event == key))
                .then_some(*action)
        })
    }

    pub(super) fn hint(&self, mode: Mode, action: Action) -> String {
        self.0[&mode]
            .get(&action)
            .map(|keys| {
                keys.iter()
                    .map(|(_, label)| label.as_str())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default()
    }

    pub(super) fn help(&self, mode: Mode) -> Vec<String> {
        self.0[&mode]
            .iter()
            .filter(|(action, keys)| action.available(mode) && !keys.is_empty())
            .map(|(action, _)| format!("{} {}", self.hint(mode, *action), action.name()))
            .collect()
    }
}

fn normalized(mut key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(character) = key.code
        && key.modifiers.contains(KeyModifiers::SHIFT)
    {
        key.code = KeyCode::Char(character.to_ascii_uppercase());
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
    KeyEvent::new(key.code, key.modifiers)
}

fn parse_key(value: &str) -> Result<KeyEvent, String> {
    let (value, modifiers) = if let Some(key) = value.strip_prefix("Ctrl+") {
        (key, KeyModifiers::CONTROL)
    } else {
        (value, KeyModifiers::NONE)
    };
    let code = match value {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Tab" => KeyCode::Tab,
        "Shift+Tab" if modifiers.is_empty() => {
            return Ok(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        }
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Space" => KeyCode::Char(' '),
        "F1" => KeyCode::F(1),
        value if value.chars().count() == 1 && !value.chars().any(char::is_control) => {
            KeyCode::Char(value.chars().next().unwrap())
        }
        _ => return Err(format!("invalid inventory key `{value}`")),
    };
    if modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('c' | 'z' | 'C' | 'Z'))
    {
        return Err("native terminal controls cannot be rebound".to_owned());
    }
    Ok(normalized(KeyEvent::new(code, modifiers)))
}
