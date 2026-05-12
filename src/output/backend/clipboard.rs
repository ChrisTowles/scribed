//! Fallback backend: write the transcript to the clipboard. Useful in
//! environments where neither ydotool nor enigo can be used (sandboxed
//! containers, restricted compositors, etc.).
//!
//! Mirrors `claude_stt/keyboard.py:310-340`.

use std::io;

use crate::output::retype::{KeyboardSink, RetypeStep};

pub struct ClipboardBackend {
    buffer: String,
}

impl ClipboardBackend {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }
}

impl Default for ClipboardBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardSink for ClipboardBackend {
    fn apply(&mut self, step: RetypeStep<'_>) -> io::Result<()> {
        // Locally apply the step to a string so we can mirror the visible
        // window state and push that to the clipboard each time. This way
        // the user can paste at any moment and get the current best transcript.
        for _ in 0..step.backspaces {
            self.buffer.pop();
        }
        self.buffer.push_str(step.insert);

        match arboard::Clipboard::new() {
            Ok(mut cb) => cb
                .set_text(&self.buffer)
                .map_err(|e| io::Error::other(format!("clipboard: {e:?}"))),
            Err(e) => Err(io::Error::other(format!("clipboard init: {e:?}"))),
        }
    }
}
