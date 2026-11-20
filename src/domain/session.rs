//! The `[session]` section: which terminal hosts sessions, and the tabs and panes a new
//! session starts with.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub host: Host,
    #[serde(default)]
    pub layouts: BTreeMap<String, LayoutConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Rex,
}

/// Each tab starts as its `root` pane and grows one split at a time beside, below or above a
/// named pane. The new pane gets `ratio` of the room.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    pub focus: Option<String>,
    #[serde(default)]
    pub tabs: BTreeMap<String, TabConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabConfig {
    #[serde(default)]
    pub order: i64,
    pub root: PaneConfig,
    #[serde(default)]
    pub splits: Vec<SplitConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneConfig {
    pub pane: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitConfig {
    pub source: String,
    pub pane: String,
    pub direction: Direction,
    #[serde(default = "half")]
    pub ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Right,
    Left,
    #[serde(alias = "below")]
    Down,
    #[serde(alias = "above")]
    Up,
}

fn half() -> f64 {
    0.5
}

/// The tabs of a new session in the order they open.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub name: String,
    pub focus: bool,
    pub panes: Panes,
}

/// A pane, or two areas side by side (`Row`) or stacked (`Column`); the ratio is the share of
/// the first area.
#[derive(Debug, Clone, PartialEq)]
pub enum Panes {
    Pane(String),
    Row(f64, Box<Panes>, Box<Panes>),
    Column(f64, Box<Panes>, Box<Panes>),
}

impl Panes {
    /// Pane names in layout order, each split's first area ahead of its second.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Panes::Pane(name) => vec![name.as_str()],
            Panes::Row(_, first, second) | Panes::Column(_, first, second) => {
                let mut names = first.names();
                names.extend(second.names());
                names
            }
        }
    }
}

impl SessionConfig {
    /// The `default` layout, resolved; the error names the setting at fault.
    pub fn layout(&self) -> Result<Layout, String> {
        let name = "default";
        let prefix = |message: String| format!("session.layouts.{name}: {message}");
        self.layouts
            .get(name)
            .ok_or_else(|| prefix("is missing".to_owned()))?
            .layout()
            .map_err(prefix)
    }
}

impl LayoutConfig {
    pub fn layout(&self) -> Result<Layout, String> {
        if self.tabs.is_empty() {
            return Err("has no tabs".to_owned());
        }
        if let Some(focus) = &self.focus
            && !self.tabs.contains_key(focus)
        {
            return Err(format!("focus names no tab {focus:?}"));
        }
        let mut tabs: Vec<_> = self.tabs.iter().collect();
        tabs.sort_by_key(|(name, tab)| (tab.order, name.as_str()));
        let tabs = tabs
            .into_iter()
            .map(|(name, tab)| {
                let mut panes = Panes::Pane(tab.root.pane.clone());
                for step in &tab.splits {
                    if !(step.ratio > 0.0 && step.ratio < 1.0) {
                        return Err(format!("ratio {} is not between 0 and 1", step.ratio));
                    }
                    if panes.names().contains(&step.pane.as_str()) {
                        return Err(format!("tab {name}: pane {:?} is named twice", step.pane));
                    }
                    panes = grow(&panes, step)
                        .ok_or_else(|| format!("tab {name}: no pane {:?}", step.source))?;
                }
                Ok(Tab {
                    name: name.clone(),
                    focus: self.focus.as_ref() == Some(name),
                    panes,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Layout { tabs })
    }
}

/// Splits the pane named `step.source`, or returns `None` when there is none.
fn grow(panes: &Panes, step: &SplitConfig) -> Option<Panes> {
    match panes {
        Panes::Pane(name) if *name == step.source => {
            let old = Box::new(panes.clone());
            let new = Box::new(Panes::Pane(step.pane.clone()));
            let ratio = step.ratio;
            Some(match step.direction {
                Direction::Right => Panes::Row(1.0 - ratio, old, new),
                Direction::Left => Panes::Row(ratio, new, old),
                Direction::Down => Panes::Column(1.0 - ratio, old, new),
                Direction::Up => Panes::Column(ratio, new, old),
            })
        }
        Panes::Pane(_) => None,
        Panes::Row(ratio, first, second) => {
            grow_pair(first, second, step).map(|(a, b)| Panes::Row(*ratio, a, b))
        }
        Panes::Column(ratio, first, second) => {
            grow_pair(first, second, step).map(|(a, b)| Panes::Column(*ratio, a, b))
        }
    }
}

fn grow_pair(
    first: &Panes,
    second: &Panes,
    step: &SplitConfig,
) -> Option<(Box<Panes>, Box<Panes>)> {
    if let Some(grown) = grow(first, step) {
        return Some((Box::new(grown), Box::new(second.clone())));
    }
    grow(second, step).map(|grown| (Box::new(first.clone()), Box::new(grown)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(name: &str) -> Box<Panes> {
        Box::new(Panes::Pane(name.to_owned()))
    }

    fn layout(toml: &str) -> Result<Layout, String> {
        toml_edit::de::from_str::<LayoutConfig>(toml)
            .unwrap()
            .layout()
    }

    #[test]
    fn each_direction_puts_the_new_pane_on_its_side_with_its_ratio() {
        for (direction, expected) in [
            ("right", Panes::Row(0.75, pane("A"), pane("B"))),
            ("left", Panes::Row(0.25, pane("B"), pane("A"))),
            ("down", Panes::Column(0.75, pane("A"), pane("B"))),
            ("below", Panes::Column(0.75, pane("A"), pane("B"))),
            ("up", Panes::Column(0.25, pane("B"), pane("A"))),
            ("above", Panes::Column(0.25, pane("B"), pane("A"))),
        ] {
            let resolved = layout(&format!(
                "[tabs.T]\nroot = {{ pane = \"A\" }}\nsplits = [{{ source = \"A\", pane = \"B\", direction = \"{direction}\", ratio = 0.25 }}]\n"
            ))
            .unwrap();
            assert_eq!(resolved.tabs[0].panes, expected, "{direction}");
        }
    }

    #[test]
    fn a_split_grows_the_named_pane_inside_earlier_splits() {
        let resolved = layout(
            r#"
focus = "Shells"
[tabs.Shells]
order = 2
root = { pane = "I" }
splits = [
  { source = "I", pane = "II", direction = "right" },
  { source = "I", pane = "III", direction = "down" },
]
[tabs.Agents]
order = 1
root = { pane = "Agent" }
"#,
        )
        .unwrap();
        assert_eq!(
            resolved,
            Layout {
                tabs: vec![
                    Tab {
                        name: "Agents".to_owned(),
                        focus: false,
                        panes: Panes::Pane("Agent".to_owned()),
                    },
                    Tab {
                        name: "Shells".to_owned(),
                        focus: true,
                        panes: Panes::Row(
                            0.5,
                            Box::new(Panes::Column(0.5, pane("I"), pane("III"))),
                            pane("II"),
                        ),
                    },
                ],
            }
        );
        assert_eq!(resolved.tabs[1].panes.names(), ["I", "III", "II"]);
    }
}
