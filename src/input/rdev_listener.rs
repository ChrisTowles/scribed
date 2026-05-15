//! macOS global-hotkey listener via [`rdev`]. Installs a CGEventTap on a
//! spawned thread.
//!
//! ## Permissions
//!
//! Reading global key events on macOS requires Accessibility:
//!
//! > System Settings → Privacy & Security → Accessibility → enable scribed
//!
//! Without it, `rdev::listen` returns no events and `CGEventTapCreate` fails
//! silently. We probe `AXIsProcessTrustedWithOptions` up front and surface a
//! `Permission` error so the orchestration layer can log a clear setup hint.
//! macOS itself shows a one-shot system prompt the first time the tap is
//! installed, so our diagnostic complements the OS prompt rather than
//! replacing it.

use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::thread;

use rdev::{Event, EventType, Key};
use thiserror::Error;

use crate::input::{
    aggregator::{Aggregator, KeyEvent, KeyState},
    KeyChord, RecordingIntent,
};

#[derive(Debug, Error)]
pub enum RdevError {
    #[error("Accessibility permission not granted (System Settings → Privacy & Security → Accessibility)")]
    Permission,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Live rdev listener. Holds the spawned reader thread; rdev::listen blocks
/// forever and has no documented stop, so the thread runs until the process
/// exits. Mirrors `EvdevListener`'s best-effort drop posture.
pub struct RdevListener {
    _thread: thread::JoinHandle<()>,
}

impl RdevListener {
    /// Start listening. Spawns one thread that owns the CGEventTap. Every
    /// time the aggregator emits an intent, `on_intent` is called from that
    /// thread.
    pub fn start<F>(
        chord: KeyChord,
        mode: crate::config::TriggerMode,
        on_intent: F,
    ) -> Result<Self, RdevError>
    where
        F: Fn(RecordingIntent) + Send + Sync + 'static,
    {
        if !is_process_trusted() {
            return Err(RdevError::Permission);
        }

        let aggregator = Arc::new(parking_lot::Mutex::new(Aggregator::new(chord, mode)));
        let on_intent: Arc<dyn Fn(RecordingIntent) + Send + Sync> = Arc::new(on_intent);

        let handle = thread::Builder::new()
            .name("scribed-rdev".into())
            .spawn(move || {
                let callback = move |event: Event| {
                    let (key, state) = match event.event_type {
                        EventType::KeyPress(k) => (k, KeyState::Press),
                        EventType::KeyRelease(k) => (k, KeyState::Release),
                        _ => return,
                    };
                    let Some(name) = key_name(key) else {
                        return;
                    };
                    let intent = {
                        let mut a = aggregator.lock();
                        a.observe(KeyEvent { key: name, state })
                    };
                    if let Some(intent) = intent {
                        (on_intent)(intent);
                    }
                };
                if let Err(e) = rdev::listen(callback) {
                    tracing::warn!(?e, "rdev::listen exited with error");
                }
            })?;

        Ok(Self { _thread: handle })
    }
}

/// Translate `rdev::Key` to our normalized [`crate::input::KeyName`].
/// Mirrors the subset that [`KeyChord::parse`] understands plus all letter
/// and digit keys. Unsupported keys return `None` so the aggregator ignores
/// them.
fn key_name(key: Key) -> Option<String> {
    let n = match key {
        Key::ControlLeft | Key::ControlRight => "ctrl",
        Key::ShiftLeft | Key::ShiftRight => "shift",
        // rdev does not distinguish left/right Option on macOS.
        Key::Alt | Key::AltGr => "alt",
        // MetaLeft/Right correspond to the Command keys on macOS.
        Key::MetaLeft | Key::MetaRight => "meta",
        Key::Space => "space",
        Key::Return => "enter",
        Key::Tab => "tab",
        Key::Escape => "escape",
        Key::Backspace => "backspace",
        Key::F1 => "f1",
        Key::F2 => "f2",
        Key::F3 => "f3",
        Key::F4 => "f4",
        Key::F5 => "f5",
        Key::F6 => "f6",
        Key::F7 => "f7",
        Key::F8 => "f8",
        Key::F9 => "f9",
        Key::F10 => "f10",
        Key::F11 => "f11",
        Key::F12 => "f12",
        other => {
            let s = format!("{other:?}");
            if let Some(suffix) = s.strip_prefix("Key") {
                if suffix.len() == 1 {
                    return Some(suffix.to_ascii_lowercase());
                }
            } else if let Some(suffix) = s.strip_prefix("Num") {
                if suffix.len() == 1 && suffix.chars().all(|c| c.is_ascii_digit()) {
                    return Some(suffix.to_string());
                }
            }
            return None;
        }
    };
    Some(n.to_string())
}

/// Silent diagnostic check for macOS Accessibility. Returns `true` if the
/// current process is trusted to receive global key events.
///
/// Passing a null options dictionary skips the system prompt; the prompt is
/// triggered separately when `rdev::listen` installs its CGEventTap.
fn is_process_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }
    unsafe { AXIsProcessTrustedWithOptions(ptr::null()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_name_translates_modifiers() {
        assert_eq!(key_name(Key::ControlLeft), Some("ctrl".into()));
        assert_eq!(key_name(Key::ShiftRight), Some("shift".into()));
        assert_eq!(key_name(Key::MetaLeft), Some("meta".into()));
        assert_eq!(key_name(Key::Alt), Some("alt".into()));
    }

    #[test]
    fn key_name_translates_alphas() {
        assert_eq!(key_name(Key::KeyA), Some("a".into()));
        assert_eq!(key_name(Key::KeyQ), Some("q".into()));
        assert_eq!(key_name(Key::KeyZ), Some("z".into()));
    }

    #[test]
    fn key_name_translates_digits() {
        assert_eq!(key_name(Key::Num0), Some("0".into()));
        assert_eq!(key_name(Key::Num1), Some("1".into()));
        assert_eq!(key_name(Key::Num9), Some("9".into()));
    }

    #[test]
    fn key_name_translates_space_and_enter() {
        assert_eq!(key_name(Key::Space), Some("space".into()));
        assert_eq!(key_name(Key::Return), Some("enter".into()));
        assert_eq!(key_name(Key::Tab), Some("tab".into()));
    }

    #[test]
    fn key_name_translates_function_keys() {
        assert_eq!(key_name(Key::F1), Some("f1".into()));
        assert_eq!(key_name(Key::F12), Some("f12".into()));
    }

    #[test]
    fn key_name_ignores_unsupported_keys() {
        assert_eq!(key_name(Key::LeftArrow), None);
        assert_eq!(key_name(Key::CapsLock), None);
    }
}
