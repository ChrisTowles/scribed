//! X11 + macOS keyboard backend via the [`enigo`] crate.
//!
//! Mirrors `claude_stt/keyboard.py:78-127` (the pynput backend).

use std::io;

use enigo::{Direction, Enigo, Keyboard, Settings};

use crate::output::retype::{KeyboardSink, RetypeStep};

pub struct EnigoBackend {
    inner: Enigo,
}

impl EnigoBackend {
    pub fn new() -> Result<Self, String> {
        let inner = Enigo::new(&Settings::default()).map_err(|e| format!("{e:?}"))?;
        Ok(Self { inner })
    }
}

impl KeyboardSink for EnigoBackend {
    fn apply(&mut self, step: RetypeStep<'_>) -> io::Result<()> {
        for _ in 0..step.backspaces {
            self.inner
                .key(enigo::Key::Backspace, Direction::Click)
                .map_err(|e| io::Error::other(format!("enigo: {e:?}")))?;
        }
        if !step.insert.is_empty() {
            self.inner
                .text(step.insert)
                .map_err(|e| io::Error::other(format!("enigo: {e:?}")))?;
        }
        Ok(())
    }
}
