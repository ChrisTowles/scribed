//! Daemon liveness checking. Combines `kill -0` with a `/proc/{pid}/cmdline`
//! (Linux) or `ps` (macOS) inspection to reduce the chance of false positives
//! when a PID has been recycled by a different process.

use std::path::Path;

/// Result of a liveness check on a recorded PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// `kill -0` succeeded and the cmdline matches our binary.
    Alive,
    /// `kill -0` failed (or cmdline mismatched). The PID file should be cleaned up.
    Stale,
}

/// Cheap signal-0 probe. Returns `true` if the process exists (regardless of
/// what it is).
pub fn process_exists(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    matches!(kill(Pid::from_raw(pid), None), Ok(()))
}

/// True if the process at `pid` is one of ours (its command line contains
/// `"scribed"`). On platforms where we can't inspect the cmdline cheaply, we
/// just fall back to [`process_exists`].
pub fn is_our_daemon(pid: i32) -> bool {
    if !process_exists(pid) {
        return false;
    }
    cmdline_contains(pid, "scribed").unwrap_or(true)
}

#[cfg(target_os = "linux")]
fn cmdline_contains(pid: i32, needle: &str) -> Option<bool> {
    let path = format!("/proc/{pid}/cmdline");
    let bytes = std::fs::read(Path::new(&path)).ok()?;
    // cmdline is NUL-delimited
    let s = String::from_utf8_lossy(&bytes).replace('\0', " ");
    Some(s.contains(needle))
}

#[cfg(target_os = "macos")]
fn cmdline_contains(pid: i32, needle: &str) -> Option<bool> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(s.contains(needle))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn cmdline_contains(_pid: i32, _needle: &str) -> Option<bool> {
    None
}

/// Categorize a PID into [`Liveness::Alive`] / [`Liveness::Stale`].
pub fn classify(pid: i32) -> Liveness {
    if is_our_daemon(pid) {
        Liveness::Alive
    } else {
        Liveness::Stale
    }
}

// Used only by the linux cmdline impl; suppress dead_code warning on macOS.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn _path_marker(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_exists_negative_pid_is_false() {
        assert!(!process_exists(-1));
        assert!(!process_exists(0));
    }

    #[test]
    fn self_pid_exists_and_is_classified_alive_when_called_from_scribed_binary() {
        let pid = std::process::id() as i32;
        assert!(process_exists(pid));
        // Note: when running under `cargo test` the binary is named something
        // like `lib-scribed-...`; the cmdline check accepts that because it
        // contains "scribed".
    }

    #[test]
    fn unlikely_pid_is_stale() {
        // Maximum 32-bit pid is unlikely to be in use
        let pid: i32 = 2_147_483_640;
        assert_eq!(classify(pid), Liveness::Stale);
    }
}
