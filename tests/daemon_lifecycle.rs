//! End-to-end daemon lifecycle test.
//!
//! Spawns `scribed start --background`, waits for it to come up, verifies
//! `status` reports it running, sends `toggle`, then `stop`. Asserts the
//! daemon is gone and the PID file is cleaned up.
//!
//! **Model independence.** The toggle path checks `recording : true` against
//! the IPC state flag, which the daemon flips before (and independently of)
//! starting an ASR session — see `IpcHandler::handle` for `DaemonCommand::Toggle`
//! at src/lifecycle/mod.rs around line 445. This means the test passes whether
//! or not the streaming Zipformer bundle is on disk in CI: a missing model
//! makes `run_loop` log a warning and leave `runtime = None`, but state
//! toggling still works. If a future change ever moves the state flip *after*
//! a successful `Runtime::start_session`, this test would gain an implicit
//! model dependency and must either ship its own model fixture or gate that
//! single assertion behind a `cfg(model_present)` probe.

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use predicates::str::contains;
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

#[test]
fn start_background_status_toggle_stop_round_trip() {
    let dir = tempdir().unwrap();
    let cd = dir.path();
    let pid_file = cd.join("daemon.pid");
    let sock = cd.join("daemon.sock");

    // 1. start --background
    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .success();

    // 2. wait for pid file to appear
    assert!(
        wait_until(Duration::from_secs(3), || pid_file.exists()
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
    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .success();
    assert!(wait_until(Duration::from_secs(3), || cd
        .join("daemon.pid")
        .exists()));

    scribed(cd)
        .args(["start", "--background"])
        .assert()
        .failure()
        .stderr(contains("already running"));

    scribed(cd).arg("stop").assert().success();
}
