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
pub fn auto_detect() -> Box<dyn KeyboardSink + Send> {
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
    if is_wayland() {
        // On Wayland the only reliable injection path is ydotool, and ydotool
        // only works if `ydotoold` is running. Enigo can't talk to a Wayland
        // compositor, so if the socket is missing we go straight to clipboard.
        return if ydotool_available() && ydotool_socket_path().is_some() {
            BackendKind::Ydotool
        } else {
            BackendKind::Clipboard
        };
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

/// Returns the ydotoold socket path if one exists on disk and is actually a
/// socket file. Respects `$YDOTOOL_SOCKET`; otherwise checks ydotool's
/// compiled-in default at `/tmp/.ydotool_socket`.
///
/// Note: existence implies the daemon was running at some point. A stale
/// socket file (daemon died, file left behind) will still pass this check;
/// keystroke calls will fail at runtime in that case. We accept the false-
/// positive risk because the diagnostic value of catching the common case
/// ("user never started ydotoold") outweighs the rare stale-socket case.
pub fn ydotool_socket_path() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::FileTypeExt;
    let candidate = std::env::var_os("YDOTOOL_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/.ydotool_socket"));
    let meta = std::fs::metadata(&candidate).ok()?;
    if meta.file_type().is_socket() {
        Some(candidate)
    } else {
        None
    }
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

    // The three socket-detection assertions share the `YDOTOOL_SOCKET` env var,
    // which is process-global. Keeping them in a single test serializes them
    // and avoids races under nextest's parallel runner.
    #[test]
    fn socket_path_detects_only_real_sockets() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();

        // Missing path -> None.
        let missing = dir.path().join("nope");
        std::env::set_var("YDOTOOL_SOCKET", &missing);
        assert!(ydotool_socket_path().is_none(), "missing file should be None");

        // Regular file -> None.
        let regular = dir.path().join("plain");
        std::fs::write(&regular, b"").unwrap();
        std::env::set_var("YDOTOOL_SOCKET", &regular);
        assert!(
            ydotool_socket_path().is_none(),
            "regular file should be None"
        );

        // Real AF_UNIX socket -> Some.
        let sock = dir.path().join("real.sock");
        let _listener = UnixListener::bind(&sock).unwrap();
        std::env::set_var("YDOTOOL_SOCKET", &sock);
        let got = ydotool_socket_path();
        assert_eq!(got, Some(sock), "real socket should be Some");

        std::env::remove_var("YDOTOOL_SOCKET");
    }
}
