//! Wayland keyboard backend: shell out to `ydotool`.
//!
//! Why subprocess? `ydotool` does the hard work of writing to `/dev/uinput`
//! and is the de-facto Wayland standard. A Rust wrapper would be ~200 lines
//! of `uinput` code with the same operational requirements (input group,
//! `ydotoold` running) — one fork per emission is invisible against typing
//! latency.
//!
//! All `Command::status()` calls would block indefinitely on a wedged
//! `ydotoold` socket, freezing the audio-consuming session thread and
//! overflowing the cpal channel. Every invocation is wrapped in
//! [`wait_with_timeout`] so a single hang surfaces as an error instead of
//! locking up the daemon.

use std::io;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::output::retype::{KeyboardSink, RetypeStep};

/// Hard ceiling on how long we wait for a single ydotool invocation. Well
/// above the expected ~10–30 ms cost of even a long backspace batch; the only
/// way to exceed it is for `ydotoold` to be stuck or unreachable.
const YDOTOOL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct YdotoolBackend;

impl YdotoolBackend {
    pub fn new() -> Self {
        Self
    }

    fn type_text(&self, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let child = Command::new("ydotool")
            .args(["type", "--key-delay", "2", "--", text])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        finish("ydotool type", child)
    }

    fn delete_chars(&self, n: usize) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        // ydotool's `key` accepts a list of keysym specs; one BackSpace press
        // is `14:1 14:0`. Batch in one invocation to avoid fork overhead.
        let mut args: Vec<String> = vec!["key".into(), "--key-delay".into(), "2".into()];
        for _ in 0..n {
            args.push("14:1".into());
            args.push("14:0".into());
        }
        let child = Command::new("ydotool")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        finish("ydotool key", child)
    }
}

/// Wait for `child` to exit, or kill it and return an error after
/// [`YDOTOOL_TIMEOUT`]. We poll with `try_wait` because `Child::wait` has no
/// timeout in std; the polling cost is negligible against the 2 s budget.
fn finish(label: &str, mut child: Child) -> io::Result<()> {
    let deadline = Instant::now() + YDOTOOL_TIMEOUT;
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(io::Error::other(format!("{label} exited {status}")));
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::other(format!(
                        "{label} timed out after {:?} — is ydotoold running?",
                        YDOTOOL_TIMEOUT
                    )));
                }
                sleep(Duration::from_millis(10));
            }
        }
    }
}

impl KeyboardSink for YdotoolBackend {
    fn apply(&mut self, step: RetypeStep<'_>) -> io::Result<()> {
        if step.backspaces > 0 {
            self.delete_chars(step.backspaces)?;
        }
        if !step.insert.is_empty() {
            self.type_text(step.insert)?;
        }
        Ok(())
    }
}
