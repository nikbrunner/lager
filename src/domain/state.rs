use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalState {
    Missing,
    Cloned,
    Conflict,
    Unreadable,
}

#[cfg(test)]
mod tests {
    use super::LocalState;

    #[test]
    fn states_are_machine_readable() {
        assert_eq!(
            serde_json::to_string(&LocalState::Cloned).unwrap(),
            "\"cloned\""
        );
    }
}
