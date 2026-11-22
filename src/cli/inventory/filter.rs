use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::{choice_line, dim, owner, owner_order};
use crate::application::inventory::{
    InventoryRow, KNOWN_CLEAN, ObservationField, RegistrationState,
};
use crate::presentation::escape;

/// One property value a row can be filtered by. The variants run in the order of the popup's
/// groups: changes, owner, registration, session.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Value {
    Dirty,
    Clean,
    /// The first folder below the root; `None` for a row that has no owner.
    Owner(Option<String>),
    Explicit,
    Wildcard,
    Unregistered,
    Excluded,
    /// The row's primary checkout has a live session.
    SessionOpen,
    SessionNone,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Changes,
    Owner,
    Registration,
    Session,
}

impl Group {
    fn heading(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Owner => "Owner",
            Self::Registration => "Registration",
            Self::Session => "Session",
        }
    }
}

impl Value {
    fn group(&self) -> Group {
        match self {
            Self::Dirty | Self::Clean => Group::Changes,
            Self::Owner(_) => Group::Owner,
            Self::SessionOpen | Self::SessionNone => Group::Session,
            _ => Group::Registration,
        }
    }

    /// The popup's order: by group, owners as the grid sorts its rows, the rest as declared.
    fn order(&self) -> (Group, (bool, String)) {
        match self {
            Self::Owner(owner) => (Group::Owner, owner_order(owner.as_deref())),
            other => (other.group(), (false, String::new())),
        }
    }

    fn line_label(&self) -> String {
        match self.group() {
            Group::Session => format!("session {}", self.label()),
            _ => self.label(),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Dirty => "dirty".to_owned(),
            Self::Clean => "clean".to_owned(),
            Self::Owner(Some(owner)) => escape(owner),
            Self::Owner(None) => "—".to_owned(),
            Self::Explicit => "explicit".to_owned(),
            Self::Wildcard => "wildcard".to_owned(),
            Self::Unregistered => "unregistered".to_owned(),
            Self::Excluded => "excluded".to_owned(),
            Self::SessionOpen => "open".to_owned(),
            Self::SessionNone => "none".to_owned(),
        }
    }
}

/// The value `row` has in each group. A row with no changes observation (missing, remote only,
/// unknown, error) has none in the changes group; one whose changes are stale keeps its last
/// known value. The session group exists once the session list has answered.
fn values(
    row: &InventoryRow,
    root: &Path,
    hosted: Option<&HashSet<PathBuf>>,
) -> [Option<Value>; 4] {
    [
        match &row.changes {
            ObservationField::Dirty(_) | ObservationField::StaleDirty(_) => Some(Value::Dirty),
            ObservationField::Clean => Some(Value::Clean),
            ObservationField::Stale(last) if last == KNOWN_CLEAN => Some(Value::Clean),
            _ => None,
        },
        Some(Value::Owner(owner(row, root))),
        Some(match row.registration {
            RegistrationState::Explicit => Value::Explicit,
            RegistrationState::Wildcard { .. } | RegistrationState::Pattern => Value::Wildcard,
            RegistrationState::Unregistered => Value::Unregistered,
            RegistrationState::Excluded { .. } => Value::Excluded,
        }),
        hosted.map(|hosted| {
            if row
                .observed_path
                .as_ref()
                .is_some_and(|path| hosted.contains(path))
            {
                Value::SessionOpen
            } else {
                Value::SessionNone
            }
        }),
    ]
}

/// The ticked values: a row stays when, in every group with a tick, it has one of them.
#[derive(Default)]
pub(super) struct Filter(BTreeSet<Value>);

impl Filter {
    pub(super) fn is_active(&self) -> bool {
        !self.0.is_empty()
    }

    pub(super) fn toggle(&mut self, value: &Value) {
        if !self.0.remove(value) {
            self.0.insert(value.clone());
        }
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// Session values apply only while `hosted` is known.
    pub(super) fn matches(
        &self,
        row: &InventoryRow,
        root: &Path,
        hosted: Option<&HashSet<PathBuf>>,
    ) -> bool {
        let found = values(row, root, hosted);
        let mut ticked: BTreeMap<Group, bool> = BTreeMap::new();
        for value in self
            .0
            .iter()
            .filter(|value| hosted.is_some() || value.group() != Group::Session)
        {
            let own = found.iter().flatten().any(|have| have == value);
            *ticked.entry(value.group()).or_default() |= own;
        }
        ticked.values().all(|hit| *hit)
    }

    /// The ticked values for the source line, group by group as the popup lists them, with the
    /// owners in the grid's order.
    pub(super) fn label(&self) -> String {
        let mut ticked = self.0.iter().collect::<Vec<_>>();
        ticked.sort_by_key(|value| value.order());
        ticked
            .into_iter()
            .map(Value::line_label)
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A value the popup offers, with the number of rows that have it.
pub(super) struct Item {
    pub(super) value: Value,
    count: usize,
}

/// Every value present among `rows`, in the order of the popup: the grid's order for owners.
pub(super) fn items(
    rows: &[InventoryRow],
    root: &Path,
    hosted: Option<&HashSet<PathBuf>>,
) -> Vec<Item> {
    let mut counts: BTreeMap<Value, usize> = BTreeMap::new();
    for row in rows {
        for value in values(row, root, hosted).into_iter().flatten() {
            *counts.entry(value).or_default() += 1;
        }
    }
    let mut items = counts
        .into_iter()
        .map(|(value, count)| Item { value, count })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.value.order());
    items
}

/// The popup body and the line the cursor is on. The cursor counts items; headings are skipped.
pub(super) fn body(
    items: &[Item],
    filter: &Filter,
    cursor: usize,
    width: u16,
) -> (Paragraph<'static>, usize) {
    let mut lines = Vec::new();
    let mut cursor_line = 0;
    let mut group = None;
    for (index, item) in items.iter().enumerate() {
        if group != Some(item.value.group()) {
            group = Some(item.value.group());
            lines.push(Line::styled(item.value.group().heading(), dim()));
        }
        if index == cursor {
            cursor_line = lines.len();
        }
        lines.push(choice_line(
            &format!(
                "{} [{}] {}",
                if index == cursor { ">" } else { " " },
                if filter.0.contains(&item.value) {
                    "x"
                } else {
                    " "
                },
                item.value.label()
            ),
            &item.count.to_string(),
            true,
            index == cursor,
            usize::from(width),
        ));
    }
    (Paragraph::new(lines), cursor_line)
}
