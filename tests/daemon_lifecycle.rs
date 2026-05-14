//! End-to-end daemon lifecycle test.
//!
//! Spawns `scribed start --background`, waits for it to come up, verifies
//! `status` reports it running, sends `toggle`, then `stop`. Asserts the
//! daemon is gone and the PID file is cleaned up.
//!
//! **Model dependency.** Production `run_loop` fetches and loads the ASR
//! model on startup and fails if either step fails. This test therefore
//! requires the streaming Zipformer bundle to be reachable (either cached
//! under `~/.cache/scribed/` or downloadable from the network).

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use predicates::str::contains;
use scribed::lifecycle::pidfile;
use tempfile::tempdir;

/// Helper: run `scribed <args>` against a temp config dir.
fn scribed(config_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("scribed").unwrap();
    cmd.arg("--config-dir").arg(config_dir);
    cmd
}

fn wait_until<F: Fn() -> bool>(deadline: Duration, check: F) -> bool {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        if check() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

/// Drop guard: tracks every pid recorded in `pid_file` over the test's
/// lifetime and SIGKILLs each at Drop. Captures pids eagerly so a successful
/// `scribed stop` (which removes pid_file) is still reachable at teardown,
/// AND so a daemon that ignored the Stop IPC (e.g., wedged tokio runtime,
/// stop's 5-second wait_for_exit timing out silently) doesn't outlive the
/// test. Without this, earlier test runs left several orphaned daemons
/// listening on /dev/input and typing into ydotool.
struct DaemonGuard {
    pid_file: std::path::PathBuf,
    seen: std::sync::Arc<parking_lot::Mutex<std::collections::BTreeSet<i32>>>,
    _watcher: std::thread::JoinHandle<()>,
    watcher_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl DaemonGuard {
    fn new(pid_file: std::path::PathBuf) -> Self {
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(std::collections::BTreeSet::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen_for_thread = seen.clone();
        let stop_for_thread = stop.clone();
        let pid_file_for_thread = pid_file.clone();
        let watcher = std::thread::spawn(move || {
            while !stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(Some(record)) = pidfile::read(&pid_file_for_thread) {
                    seen_for_thread.lock().insert(record.pid);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Self {
            pid_file,
            seen,
            _watcher: watcher,
            watcher_stop: stop,
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        self.watcher_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // One last sweep of the pid file in case the watcher missed it.
        if let Ok(Some(record)) = pidfile::read(&self.pid_file) {
            self.seen.lock().insert(record.pid);
        }
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        for pid in self.seen.lock().iter() {
            // SIGKILL — graceful shutdown was the test's job; if we're in
            // Drop the daemon either survived stop or the test panicked.
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
    }
}

#[test]
fn start_background_status_toggle_stop_round_trip() {
    let dir = tempdir().unwrap();
    let cd = dir.path();
    let pid_file = cd.join("daemon.pid");
    let sock = cd.join("daemon.sock");
    let _guard = DaemonGuard::new(pid_file.clone());

    // 1. start --background
    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .success();

    // 2. wait for pid file to appear. Generous deadline because the daemon
    // now blocks on model load (~1-3s on dev hardware, up to ~10s on slow
    // CI runners) before writing the pid file.
    assert!(
        wait_until(Duration::from_secs(30), || pid_file.exists()
            && sock.exists()),
        "daemon never wrote pid file / socket"
    );

    // 3. status reports running
    scribed(cd)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("running"));

    // 4. toggle returns OK
    scribed(cd).arg("toggle").assert().success();

    // 5. status now reports recording = true
    scribed(cd)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("recording  : true"));

    // 6. stop the daemon
    scribed(cd).arg("stop").assert().success();

    // 7. pid file should be gone
    assert!(
        wait_until(Duration::from_secs(3), || !pid_file.exists()),
        "pid file was not cleaned up"
    );
}

#[test]
fn stop_when_not_running_errors() {
    let dir = tempdir().unwrap();
    let cd = dir.path();
    scribed(cd)
        .arg("stop")
        .assert()
        .failure()
        .stderr(contains("not running"));
}

#[test]
fn toggle_when_not_running_errors() {
    let dir = tempdir().unwrap();
    let cd = dir.path();
    scribed(cd)
        .arg("toggle")
        .assert()
        .failure()
        .stderr(contains("not running"));
}

#[test]
fn double_start_fails_with_already_running() {
    let dir = tempdir().unwrap();
    let cd = dir.path();
    let _guard = DaemonGuard::new(cd.join("daemon.pid"));
    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .success();
    assert!(wait_until(Duration::from_secs(30), || cd
        .join("daemon.pid")
        .exists()));

    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .failure()
        .stderr(contains("already running"));

    scribed(cd).arg("stop").assert().success();
}
