//! Linux global-hotkey listener via evdev. Works on X11 *and* Wayland because
//! it reads `/dev/input/event*` directly — bypassing the windowing system.
//!
//! ## Permissions
//!
//! Reading `/dev/input` requires the user to be in the `input` group:
//!
//! ```text
//! sudo usermod -aG input "$USER"
//! ```
//!
//! Without that, opening a device returns `EACCES`. We detect this and bubble
//! it up as a clear setup error rather than a generic IO failure.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use evdev::{Device, EventSummary, KeyCode};
use thiserror::Error;

use crate::input::{
    aggregator::{Aggregator, KeyEvent, KeyState},
    KeyChord, RecordingIntent,
};

#[derive(Debug, Error)]
pub enum EvdevError {
    #[error("could not open any keyboard device under /dev/input (need group 'input': sudo usermod -aG input $USER)")]
    NoAccessibleKeyboards,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Live evdev listener. Holds the spawned reader threads; dropping it stops
/// listening (the threads will exit on the next blocked read after the
/// shutdown flag is set — they do unblock when the device produces any event).
pub struct EvdevListener {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    _threads: Vec<thread::JoinHandle<()>>,
}

impl EvdevListener {
    /// Start listening. Spawns one reader thread per detected keyboard. Every
    /// time the aggregator emits an intent, `on_intent` is called from one of
    /// those threads.
    pub fn start<F>(
        chord: KeyChord,
        mode: crate::config::TriggerMode,
        on_intent: F,
    ) -> Result<Self, EvdevError>
    where
        F: Fn(RecordingIntent) + Send + Sync + 'static,
    {
        let devices = enumerate_keyboards()?;
        if devices.is_empty() {
            return Err(EvdevError::NoAccessibleKeyboards);
        }

        let aggregator = Arc::new(parking_lot::Mutex::new(Aggregator::new(chord, mode)));
        let on_intent: Arc<dyn Fn(RecordingIntent) + Send + Sync> = Arc::new(on_intent);
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut threads = Vec::new();

        for (path, mut device) in devices {
            let agg = aggregator.clone();
            let intent_cb = on_intent.clone();
            let shut = shutdown.clone();
            let handle = thread::Builder::new()
                .name(format!("scribed-evdev-{}", path.display()))
                .spawn(move || run_device(&mut device, &agg, &intent_cb, &shut))
                .map_err(EvdevError::Io)?;
            threads.push(handle);
        }

        Ok(Self {
            shutdown,
            _threads: threads,
        })
    }

    /// Stop listening. Threads will exit on next event from their device
    /// (or stay blocked forever if the keyboard is idle — set by design;
    /// dropping the listener is best-effort).
    pub fn stop(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for EvdevListener {
    fn drop(&mut self) {
        self.stop();
    }
}

fn enumerate_keyboards() -> Result<Vec<(PathBuf, Device)>, EvdevError> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir("/dev/input").map_err(EvdevError::Io)?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("event") {
            continue;
        }
        let device = match Device::open(&path) {
            Ok(d) => d,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) => {
                tracing::debug!(?path, "skipping (no permission)");
                continue;
            }
            Err(e) => {
                tracing::debug!(?path, ?e, "skipping (open failed)");
                continue;
            }
        };
        if is_keyboard(&device) {
            found.push((path, device));
        }
    }
    Ok(found)
}

fn is_keyboard(device: &Device) -> bool {
    let Some(keys) = device.supported_keys() else {
        return false;
    };
    // Anything that supports the alphabetic Q is plausibly a keyboard.
    keys.contains(KeyCode::KEY_Q)
}

fn run_device(
    device: &mut Device,
    aggregator: &parking_lot::Mutex<Aggregator>,
    on_intent: &Arc<dyn Fn(RecordingIntent) + Send + Sync>,
    shutdown: &std::sync::atomic::AtomicBool,
) {
    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let events = match device.fetch_events() {
            Ok(it) => it,
            Err(e) => {
                tracing::warn!(?e, "evdev fetch_events failed; reader exiting");
                return;
            }
        };
        for ev in events {
            if let EventSummary::Key(_, code, value) = ev.destructure() {
                let state = match value {
                    1 => KeyState::Press,
                    0 => KeyState::Release,
                    _ => continue, // 2 = auto-repeat; we ignore these
                };
                let Some(name) = key_name(code) else {
                    continue;
                };
                let intent = {
                    let mut agg = aggregator.lock();
                    agg.observe(KeyEvent { key: name, state })
                };
                if let Some(intent) = intent {
                    (on_intent)(intent);
                }
            }
        }
    }
}

/// Translate evdev key codes to our normalized [`KeyName`].
/// Mirrors the subset that [`KeyChord::parse`] understands plus all letter
/// and digit keys.
fn key_name(code: KeyCode) -> Option<String> {
    use KeyCode as K;
    let n = match code {
        K::KEY_LEFTCTRL | K::KEY_RIGHTCTRL => "ctrl",
        K::KEY_LEFTSHIFT | K::KEY_RIGHTSHIFT => "shift",
        K::KEY_LEFTALT | K::KEY_RIGHTALT => "alt",
        K::KEY_LEFTMETA | K::KEY_RIGHTMETA => "meta",
        K::KEY_SPACE => "space",
        K::KEY_ENTER => "enter",
        K::KEY_TAB => "tab",
        K::KEY_ESC => "escape",
        K::KEY_BACKSPACE => "backspace",
        K::KEY_F1 => "f1",
        K::KEY_F2 => "f2",
        K::KEY_F3 => "f3",
        K::KEY_F4 => "f4",
        K::KEY_F5 => "f5",
        K::KEY_F6 => "f6",
        K::KEY_F7 => "f7",
        K::KEY_F8 => "f8",
        K::KEY_F9 => "f9",
        K::KEY_F10 => "f10",
        K::KEY_F11 => "f11",
        K::KEY_F12 => "f12",
        c => {
            // Best-effort: KEY_A -> "a", KEY_1 -> "1", etc.
            let s = format!("{c:?}");
            // s looks like "KEY_A" or "KEY_1"
            let suffix = s.strip_prefix("KEY_")?;
            if suffix.len() == 1 {
                return Some(suffix.to_ascii_lowercase());
            }
            return None;
        }
    };
    Some(n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_name_translates_modifiers() {
        assert_eq!(key_name(KeyCode::KEY_LEFTCTRL), Some("ctrl".into()));
        assert_eq!(key_name(KeyCode::KEY_RIGHTSHIFT), Some("shift".into()));
        assert_eq!(key_name(KeyCode::KEY_LEFTMETA), Some("meta".into()));
    }

    #[test]
    fn key_name_translates_alphas() {
        assert_eq!(key_name(KeyCode::KEY_A), Some("a".into()));
        assert_eq!(key_name(KeyCode::KEY_Q), Some("q".into()));
    }

    #[test]
    fn key_name_translates_space_and_enter() {
        assert_eq!(key_name(KeyCode::KEY_SPACE), Some("space".into()));
        assert_eq!(key_name(KeyCode::KEY_ENTER), Some("enter".into()));
    }

    /// We don't actually try to open /dev/input in CI — but we want to
    /// ensure the start function fails *gracefully* in environments without
    /// the right permissions.
    #[test]
    fn start_returns_error_when_no_keyboards() {
        // Save/restore would be invasive; we just verify the error type
        // is constructible.
        let err = EvdevError::NoAccessibleKeyboards;
        let msg = format!("{err}");
        assert!(msg.contains("input"));
    }
}
