//! Wayland keyboard backend: shell out to `ydotool`.
//!
//! Why subprocess? `ydotool` already does the hard work of writing to
//! `/dev/uinput` and is the de-facto standard on Wayland. A Rust wrapper would
//! be ~200 lines of `uinput` code with the same operational requirements (must
//! be in `input` group, must have `ydotoold` running) — the cost is one fork
//! per emission, which is invisible compared to typing latency.
//!
//! Mirrors `claude_stt/keyboard.py:58-156`.

use std::io;
use std::process::{Command, Stdio};

use crate::output::retype::{KeyboardSink, RetypeStep};

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
        let status = Command::new("ydotool")
            .args(["type", "--key-delay", "2", "--", text])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!("ydotool type exited {status}")));
        }
        Ok(())
    }

    fn delete_chars(&self, n: usize) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        // ydotool's `key` accepts a list of keysym specs; one BackSpace press is
        // `14:1 14:0`. Batch all backspaces in one invocation to avoid fork
        // overhead per delete.
        let mut args: Vec<String> = vec!["key".into(), "--key-delay".into(), "2".into()];
        for _ in 0..n {
            args.push("14:1".into());
            args.push("14:0".into());
        }
        let status = Command::new("ydotool")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!("ydotool key exited {status}")));
        }
        Ok(())
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
