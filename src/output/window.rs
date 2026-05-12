//! Active-window capture and restore.
//!
//! Captured at the moment the recording starts (so we know which window the
//! transcript should land in), and restored just before typing (so the user
//! can `Alt+Tab` mid-recording without the transcript ending up in the wrong
//! place).
//!
//! Read side uses [`active-win-pos-rs`]. Restore side is platform-specific:
//!
//! - **X11**: `xdotool windowactivate <window_id>`
//! - **Wayland**: not supported (no compositor-agnostic API)
//! - **macOS**: `osascript` to set the frontmost process
//!
//! Mirrors `claude_stt/window.py`.

use std::process::Command;

/// What we capture at recording start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub platform: Platform,
    pub window_id: String,
    pub app_name: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    LinuxX11,
    LinuxWayland,
    Macos,
}

impl Platform {
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            return Platform::Macos;
        }
        if std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY").is_ok()
        {
            return Platform::LinuxWayland;
        }
        Platform::LinuxX11
    }
}

/// Capture the active window. Returns `None` if it can't be determined (e.g.
/// no window is focused, or the platform doesn't permit reading it).
pub fn capture() -> Option<WindowSnapshot> {
    let win = active_win_pos_rs::get_active_window().ok()?;
    Some(WindowSnapshot {
        platform: Platform::detect(),
        window_id: win.window_id.to_string(),
        app_name: win.app_name,
        title: win.title,
    })
}

/// Attempt to restore focus to the snapshotted window. Returns `true` if the
/// operation was issued (does not guarantee it succeeded).
pub fn restore(snap: &WindowSnapshot) -> bool {
    match snap.platform {
        Platform::LinuxX11 => x11_restore(&snap.window_id),
        Platform::LinuxWayland => {
            // No reliable cross-compositor primitive. The caller will fall
            // through to clipboard output if restore matters.
            false
        }
        Platform::Macos => macos_restore(&snap.app_name),
    }
}

fn x11_restore(window_id: &str) -> bool {
    Command::new("xdotool")
        .args(["windowactivate", window_id])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn macos_restore(app_name: &str) -> bool {
    // Escape any double quotes for the AppleScript string literal.
    let safe = app_name.replace('"', "");
    let script = format!(r#"tell application "{safe}" to activate"#);
    Command::new("osascript")
        .args(["-e", &script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Convenience: does the snapshot's app match an excluded-apps substring list?
pub fn is_excluded(snap: &WindowSnapshot, excluded: &[String]) -> bool {
    let needle = snap.app_name.to_ascii_lowercase();
    excluded
        .iter()
        .any(|s| needle.contains(&s.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(app: &str) -> WindowSnapshot {
        WindowSnapshot {
            platform: Platform::LinuxX11,
            window_id: "0x123".into(),
            app_name: app.into(),
            title: "doc.txt".into(),
        }
    }

    #[test]
    fn excluded_match_is_case_insensitive_and_substring() {
        let s = snap("KeePassXC");
        assert!(is_excluded(&s, &["keepass".into()]));
        assert!(is_excluded(&s, &["XC".into()]));
        assert!(!is_excluded(&s, &["firefox".into()]));
    }

    #[test]
    fn excluded_empty_list_never_matches() {
        let s = snap("anything");
        assert!(!is_excluded(&s, &[]));
    }

    #[test]
    fn platform_detect_returns_something() {
        let _ = Platform::detect();
    }

    #[test]
    fn capture_does_not_panic() {
        // In CI there may not be a windowing system; we just want this to
        // return None gracefully rather than panic.
        let _ = capture();
    }
}
