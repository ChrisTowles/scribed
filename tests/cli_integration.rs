//! Integration tests for the CLI surface. Black-box: spawn `scribed` and
//! assert on its stdout/stderr/exit code.

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::str::contains;
use tempfile::tempdir;

#[test]
fn version_flag_prints_version() {
    let mut cmd = Command::cargo_bin("scribed").unwrap();
    cmd.arg("--version");
    cmd.assert().success().stdout(contains("scribed"));
}

#[test]
fn help_flag_lists_subcommands() {
    let mut cmd = Command::cargo_bin("scribed").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(contains("start"))
        .stdout(contains("stop"))
        .stdout(contains("status"))
        .stdout(contains("toggle"));
}

#[test]
fn status_renders_paths_and_default_hotkey() {
    let dir = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("scribed").unwrap();
    cmd.arg("--config-dir").arg(dir.path()).arg("status");
    cmd.assert()
        .success()
        .stdout(contains("config dir"))
        .stdout(contains("ctrl+shift+space"))
        .stdout(contains("not running"));
}

#[test]
fn print_config_returns_toml() {
    let dir = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("scribed").unwrap();
    cmd.arg("--config-dir").arg(dir.path()).arg("print-config");
    cmd.assert()
        .success()
        .stdout(contains("[scribed]"))
        .stdout(contains("hotkey = \"ctrl+shift+space\""));
}

// `start` (foreground) blocks until SIGTERM and is exercised by the
// daemon_lifecycle integration test via `start --background` instead.
