//! Keyboard backends. The orchestration layer auto-detects which one to use
//! based on the session type:
//!
//! - **Wayland session** (`XDG_SESSION_TYPE=wayland`) + `ydotool` on `PATH`:
//!   use the ydotool subprocess backend. Required because no userspace API
//!   gives reliable synthetic input on Wayland.
//! - **X11 session**: use [`enigo`](https://crates.io/crates/enigo).
//! - **macOS**: use [`enigo`](https://crates.io/crates/enigo).
//! - **Fallback** (no working synthetic-input backend): clipboard. Puts the
//!   transcript on the clipboard so the user can paste it manually.

use std::io;

use super::retype::{KeyboardSink, RetypeStep};

/// What kind of backend we ended up with. Mostly used by `status` to tell the
/// user what they're getting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Ydotool,
    Enigo,
    Clipboard,
    Null,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Ydotool => "ydotool",
            BackendKind::Enigo => "enigo",
            BackendKind::Clipboard => "clipboard",
            BackendKind::Null => "null",
        }
    }
}

pub mod ydotool;

pub mod enigo;

pub mod clipboard;

/// Pick a backend appropriate for the current environment.
pub fn auto_detect() -> Box<dyn KeyboardSink> {
    match select_backend_kind() {
        BackendKind::Ydotool => Box::new(ydotool::YdotoolBackend::new()),
        BackendKind::Enigo => match enigo::EnigoBackend::new() {
            Ok(b) => Box::new(b),
            Err(e) => {
                tracing::warn!(?e, "enigo init failed; falling back to clipboard");
                Box::new(clipboard::ClipboardBackend::new())
            }
        },
        BackendKind::Clipboard => Box::new(clipboard::ClipboardBackend::new()),
        BackendKind::Null => Box::new(NullBackend),
    }
}

pub fn select_backend_kind() -> BackendKind {
    if is_wayland() && ydotool_available() {
        return BackendKind::Ydotool;
    }
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        return BackendKind::Enigo;
    }
    BackendKind::Clipboard
}

pub fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|s| s.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

pub fn ydotool_available() -> bool {
    which("ydotool").is_some()
}

fn which(prog: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(prog);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Discards every step. Used in tests that need a `Box<dyn KeyboardSink>`.
#[derive(Debug, Default)]
pub struct NullBackend;

impl KeyboardSink for NullBackend {
    fn apply(&mut self, _step: RetypeStep<'_>) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_kind_does_not_panic() {
        let _ = select_backend_kind();
    }

    #[test]
    fn as_str_for_each_kind() {
        assert_eq!(BackendKind::Ydotool.as_str(), "ydotool");
        assert_eq!(BackendKind::Enigo.as_str(), "enigo");
        assert_eq!(BackendKind::Clipboard.as_str(), "clipboard");
        assert_eq!(BackendKind::Null.as_str(), "null");
    }
}
