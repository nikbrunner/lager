//! Escaping at human-output boundaries only, never in stored or operational data.

use std::fmt::{Display, Write};

pub(crate) fn escape(value: impl Display) -> String {
    let mut escaped = String::new();
    for character in value.to_string().chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                write!(escaped, "\\x{:02x}", character as u32).expect("write to string");
            }
            _ => escaped.push(character),
        }
    }
    escaped
}
