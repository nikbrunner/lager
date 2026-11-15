use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::infrastructure::config::InventoryKeyOverrides;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Mode {
    #[default]
    Normal,
    Menu,
    Search,
    Input,
    Confirmation,
    Inspection,
    Worktrees,
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
    Acknowledge,
    Worktrees,
    Rename,
    Archive,
    Visibility,
    DeleteRemote,
    CreateRemote,
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
            Self::Acknowledge => "acknowledge",
            Self::Worktrees => "worktrees",
            Self::Rename => "rename",
            Self::Archive => "archive",
            Self::Visibility => "visibility",
            Self::DeleteRemote => "delete_remote",
            Self::CreateRemote => "create_remote",
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
                    | Self::Inspect
                    | Self::Menu
                    | Self::Mark
                    | Self::Add
                    | Self::Remove
                    | Self::Register
                    | Self::Unregister
                    | Self::Clone
                    | Self::CloneRegister
                    | Self::RemoveUnregister
                    | Self::Copy
                    | Self::Open
                    | Self::Remote
                    | Self::Retry
                    | Self::Archives
                    | Self::Worktrees
                    | Self::Rename
                    | Self::Archive
                    | Self::Visibility
                    | Self::DeleteRemote
                    | Self::CreateRemote
            ),
            Mode::Menu => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::PageUp
                    | Self::PageDown
                    | Self::Accept
                    | Self::Cancel
                    | Self::Help
                    | Self::Quit
                    | Self::Search
                    | Self::Inspect
                    | Self::Refresh
                    | Self::ClearSearch
            ),
            Mode::Search | Mode::Input => {
                matches!(self, Self::Accept | Self::Cancel | Self::Help | Self::Quit)
            }
            Mode::Confirmation => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::PageUp
                    | Self::PageDown
                    | Self::Accept
                    | Self::Cancel
                    | Self::Help
                    | Self::Quit
                    | Self::Acknowledge
            ),
            Mode::Inspection => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::PageUp
                    | Self::PageDown
                    | Self::Copy
                    | Self::Accept
                    | Self::Cancel
                    | Self::Help
                    | Self::Quit
            ),
            Mode::Worktrees => matches!(
                self,
                Self::Up
                    | Self::Down
                    | Self::PageUp
                    | Self::PageDown
                    | Self::Add
                    | Self::Remove
                    | Self::Copy
                    | Self::Accept
                    | Self::Cancel
                    | Self::Help
                    | Self::Quit
            ),
        }
    }
}

/// Defaults that yield to an explicit key of the same mode, as (mode, action, key).
const LATE_DEFAULTS: [(Mode, Action, &str); 8] = [
    (Mode::Normal, Action::Worktrees, "w"),
    (Mode::Worktrees, Action::Add, "a"),
    (Mode::Worktrees, Action::Remove, "r"),
    (Mode::Worktrees, Action::Copy, "c"),
    (Mode::Inspection, Action::Copy, "c"),
    (Mode::Confirmation, Action::Acknowledge, "Space"),
    (Mode::Confirmation, Action::Cancel, "q"),
    (Mode::Confirmation, Action::Quit, "Ctrl+q"),
];

/// The effective keys per mode and action, and the lists the configuration set explicitly.
pub(super) struct Bindings(
    BTreeMap<Mode, BTreeMap<Action, Vec<(KeyEvent, String)>>>,
    BTreeMap<Mode, BTreeMap<Action, Vec<String>>>,
);

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
            ("worktrees", Mode::Worktrees),
        ];
        for mode in overrides.keys() {
            if !modes.iter().any(|(name, _)| name == mode) {
                return Err(format!("unknown inventory key mode `{mode}`"));
            }
        }
        let mut maps: BTreeMap<Mode, BTreeMap<Action, Vec<(KeyEvent, String)>>> = BTreeMap::new();
        let mut config = BTreeMap::new();
        for (name, mode) in modes {
            let typing = matches!(mode, Mode::Search | Mode::Input);
            let popup = matches!(
                mode,
                Mode::Menu | Mode::Inspection | Mode::Confirmation | Mode::Worktrees
            );
            let mut defaults: Vec<(Action, Vec<&str>)> = vec![
                (Help, vec![if typing { "F1" } else { "?" }]),
                (Quit, vec![if typing || popup { "Ctrl+q" } else { "q" }]),
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
                    (Worktrees, vec!["w"]),
                    (Rename, vec![]),
                    (Archive, vec![]),
                    (Visibility, vec![]),
                    (DeleteRemote, vec![]),
                    (CreateRemote, vec![]),
                ]);
            } else {
                defaults.extend([
                    (Accept, vec!["Enter"]),
                    (Cancel, if popup { vec!["Esc", "q"] } else { vec!["Esc"] }),
                ]);
                if !typing {
                    defaults.extend([
                        (Up, vec!["k", "Up", "Shift+Tab"]),
                        (Down, vec!["j", "Down", "Tab"]),
                        (PageUp, vec!["PageUp"]),
                        (PageDown, vec!["PageDown"]),
                    ]);
                }
                if mode == Mode::Inspection {
                    defaults.push((Copy, vec!["c"]));
                }
                if mode == Mode::Confirmation {
                    defaults.push((Acknowledge, vec!["Space"]));
                }
                if mode == Mode::Worktrees {
                    defaults.extend([(Add, vec!["a"]), (Remove, vec!["r"]), (Copy, vec!["c"])]);
                }
                if mode == Mode::Menu {
                    // These inherit normal-mode shortcuts unless the menu map explicitly
                    // replaces them. They are added after parsing so menu navigation keeps
                    // precedence for any conflicting keys.
                    defaults.extend([
                        (Search, vec![]),
                        (Inspect, vec![]),
                        (Refresh, vec![]),
                        (ClearSearch, vec![]),
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
            let mut explicit = BTreeMap::new();
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
                    explicit.insert(action, keys.clone());
                }
            }
            // These defaults arrived after configurations were written against the earlier maps;
            // they give way to an explicit key so those configurations keep loading.
            let taken = explicit
                .values()
                .flatten()
                .filter_map(|key| parse_key(key).ok())
                .collect::<Vec<_>>();
            for (late_mode, action, key) in LATE_DEFAULTS {
                if late_mode == mode && !explicit.contains_key(&action) {
                    let event = parse_key(key).expect("late defaults parse");
                    map.entry(action)
                        .or_default()
                        .retain(|default| default != key || !taken.contains(&event));
                }
            }
            // A configuration that takes Ctrl-Q keeps confirmation quit reachable: `q` moves from
            // cancel to quit.
            let q = parse_key("q").expect("q parses");
            if mode == Mode::Confirmation
                && !explicit.contains_key(&Quit)
                && map[&Quit].is_empty()
                && !taken.contains(&q)
            {
                if !explicit.contains_key(&Cancel) {
                    map.entry(Cancel).or_default().retain(|key| key != "q");
                }
                map.insert(Quit, vec!["q".to_owned()]);
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
            let mut used: Vec<(KeyEvent, Action, String)> = Vec::new();
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
                    if let Some((_, other, spelling)) =
                        used.iter().find(|(used, ..)| *used == event)
                    {
                        // Name the key as the configuration spells it.
                        let key = if explicit.contains_key(&action) {
                            &key
                        } else {
                            spelling
                        };
                        return Err(format!(
                            "[inventory.keys.{name}] binds {key} to {} and to {}",
                            other.name(),
                            action.name()
                        ));
                    }
                    used.push((event, action, key.clone()));
                    bindings.push((event, key));
                }
                parsed.insert(action, bindings);
            }
            if mode == Mode::Menu {
                let normal = &maps[&Mode::Normal];
                let custom = overrides.get(name);
                for action in [Search, Inspect, Refresh, ClearSearch] {
                    if custom.is_some_and(|bindings| bindings.contains_key(action.name())) {
                        continue;
                    }
                    for (event, label) in &normal[&action] {
                        // Menu-local bindings win so a normal shortcut can never make an
                        // advertised menu action behave as navigation or acceptance instead.
                        if !used.iter().any(|(used, ..)| used == event) {
                            used.push((*event, action, label.clone()));
                            parsed
                                .get_mut(&action)
                                .expect("menu action has an effective binding entry")
                                .push((*event, label.clone()));
                        }
                    }
                }
            }
            maps.insert(mode, parsed);
            config.insert(mode, explicit);
        }
        Ok(Self(maps, config))
    }

    pub(super) fn action(&self, mode: Mode, key: KeyEvent) -> Option<Action> {
        let key = normalized(key);
        self.0[&mode].iter().find_map(|(action, keys)| {
            (action.available(mode) && keys.iter().any(|(event, _)| *event == key))
                .then_some(*action)
        })
    }

    /// Whether `key` is the first key bound to `action`, the one hints show.
    pub(super) fn is_first(&self, mode: Mode, action: Action, key: KeyEvent) -> bool {
        self.0[&mode]
            .get(&action)
            .and_then(|keys| keys.first())
            .is_some_and(|(event, _)| *event == normalized(key))
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

    /// Actions of `mode` that no key reaches.
    pub(super) fn unbound(&self, mode: Mode) -> Vec<Action> {
        self.0[&mode]
            .iter()
            .filter(|(action, keys)| action.available(mode) && keys.is_empty())
            .map(|(action, _)| *action)
            .collect()
    }

    /// The explicit configuration of `action` in `mode`, as written under `[inventory.keys]`.
    pub(super) fn configured(&self, mode: Mode, action: Action) -> Option<&[String]> {
        self.1.get(&mode)?.get(&action).map(Vec::as_slice)
    }

    pub(super) fn keys(&self, mode: Mode, action: Action) -> Vec<&str> {
        self.0[&mode]
            .get(&action)
            .map(|keys| keys.iter().map(|(_, label)| label.as_str()).collect())
            .unwrap_or_default()
    }
}

fn normalized(mut key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(character) = key.code
        && key.modifiers.contains(KeyModifiers::SHIFT)
    {
        key.code = KeyCode::Char(character.to_ascii_uppercase());
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
    if let KeyCode::Char(character) = key.code
        && key.modifiers == KeyModifiers::CONTROL
    {
        let character = character.to_ascii_lowercase();
        let equivalent = match character {
            'i' => Some(KeyCode::Tab),
            'm' => Some(KeyCode::Enter),
            '[' | '3' => Some(KeyCode::Esc),
            '?' | '8' => Some(KeyCode::Backspace),
            _ => None,
        };
        if let Some(code) = equivalent {
            return KeyEvent::new(code, KeyModifiers::NONE);
        }
        key.code = KeyCode::Char(match character {
            '@' | '2' => ' ',
            '\\' => '4',
            ']' => '5',
            '^' => '6',
            '_' | '/' => '7',
            _ => character,
        });
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
    let event = normalized(KeyEvent::new(code, modifiers));
    if event.modifiers == KeyModifiers::CONTROL
        && !matches!(event.code,
            KeyCode::Char('a'..='z' | '4'..='7' | ' ')
                | KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
                | KeyCode::Delete | KeyCode::F(1))
    {
        return Err(format!("unsupported Ctrl binding `{value}`"));
    }
    Ok(event)
}
