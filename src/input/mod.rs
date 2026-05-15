//! Input bounded context — global hotkey listening, key-chord parsing,
//! recording intents.

use std::collections::BTreeSet;
use std::fmt;

use thiserror::Error;

pub mod aggregator;

#[cfg(target_os = "linux")]
pub mod evdev_listener;

#[cfg(target_os = "macos")]
pub mod rdev_listener;

pub use aggregator::{Aggregator, KeyEvent, KeyName, KeyState};

/// What the orchestration layer should do in response to a hotkey event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingIntent {
    Start,
    Stop,
    Toggle,
}

/// A normalized key chord, e.g. `Ctrl+Shift+Space`. Order-independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    /// Normalized modifier names. Always lowercase: "ctrl", "shift", "alt", "meta".
    pub modifiers: BTreeSet<String>,
    /// The non-modifier trigger key. Always lowercase; multi-char keys are
    /// stripped of `<>` brackets ("space", "enter", "f1", "a").
    pub trigger: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HotkeyParseError {
    #[error("empty hotkey expression")]
    Empty,
    #[error("hotkey must include exactly one non-modifier key; got {0:?}")]
    AmbiguousTrigger(Vec<String>),
    #[error("hotkey '{0}' has no trigger key (only modifiers)")]
    NoTrigger(String),
}

impl KeyChord {
    /// Parse expressions like `"ctrl+shift+space"`, `"<ctrl>+<shift>+<space>"`,
    /// `"control + shift + space"`. Modifier aliases match the Python project:
    /// - `control` → `ctrl`
    /// - `command`, `cmd`, `super`, `win`, `meta` → `meta`
    /// - `option`, `opt` → `alt`
    /// - `return` → `enter`
    pub fn parse(expr: &str) -> Result<Self, HotkeyParseError> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return Err(HotkeyParseError::Empty);
        }
        let mut modifiers = BTreeSet::new();
        let mut triggers = Vec::new();
        for raw in trimmed.split('+') {
            let token = normalize_token(raw.trim());
            if token.is_empty() {
                continue;
            }
            if is_modifier(&token) {
                modifiers.insert(canonical_modifier(&token));
            } else {
                triggers.push(token);
            }
        }
        match triggers.len() {
            0 => Err(HotkeyParseError::NoTrigger(expr.to_string())),
            1 => Ok(KeyChord {
                modifiers,
                trigger: triggers.into_iter().next().unwrap(),
            }),
            _ => Err(HotkeyParseError::AmbiguousTrigger(triggers)),
        }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in &self.modifiers {
            write!(f, "{m}+")?;
        }
        write!(f, "{}", self.trigger)
    }
}

fn normalize_token(token: &str) -> String {
    let t = token.trim().trim_start_matches('<').trim_end_matches('>');
    t.to_ascii_lowercase()
}

fn is_modifier(token: &str) -> bool {
    matches!(
        token,
        "ctrl"
            | "control"
            | "shift"
            | "alt"
            | "option"
            | "opt"
            | "cmd"
            | "command"
            | "super"
            | "win"
            | "meta"
    )
}

fn canonical_modifier(token: &str) -> String {
    match token {
        "control" => "ctrl".into(),
        "option" | "opt" => "alt".into(),
        "cmd" | "command" | "super" | "win" => "meta".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modset<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_default_hotkey() {
        let c = KeyChord::parse("ctrl+shift+space").unwrap();
        assert_eq!(c.modifiers, modset(["ctrl", "shift"]));
        assert_eq!(c.trigger, "space");
    }

    #[test]
    fn whitespace_and_angle_brackets_tolerated() {
        let c = KeyChord::parse("<ctrl> + <shift> + <space>").unwrap();
        assert_eq!(c.modifiers, modset(["ctrl", "shift"]));
        assert_eq!(c.trigger, "space");
    }

    #[test]
    fn aliases_normalize() {
        let c = KeyChord::parse("control+option+cmd+r").unwrap();
        assert_eq!(c.modifiers, modset(["alt", "ctrl", "meta"]));
        assert_eq!(c.trigger, "r");
    }

    #[test]
    fn empty_is_error() {
        assert_eq!(KeyChord::parse("   "), Err(HotkeyParseError::Empty));
    }

    #[test]
    fn no_trigger_is_error() {
        assert!(matches!(
            KeyChord::parse("ctrl+shift"),
            Err(HotkeyParseError::NoTrigger(_))
        ));
    }

    #[test]
    fn two_triggers_is_error() {
        assert!(matches!(
            KeyChord::parse("ctrl+a+b"),
            Err(HotkeyParseError::AmbiguousTrigger(_))
        ));
    }

    #[test]
    fn display_round_trips_normalized() {
        let c = KeyChord::parse("Shift+Control+space").unwrap();
        assert_eq!(c.to_string(), "ctrl+shift+space");
    }
}
