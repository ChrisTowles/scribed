//! ASR bounded context — engines that turn audio chunks into transcripts.
//!
//! The [`driver`] submodule houses the engine-agnostic streaming logic
//! (rolling buffer, silence gate, silence reset). The [`sherpa`] submodule
//! (gated behind `--features asr`) is the production backend.

use std::sync::Arc;

use thiserror::Error;

pub mod download;
pub mod driver;

#[cfg(feature = "asr")]
pub mod sherpa;

pub use driver::{DriverConfig, StreamingDriver, Transcriber};

/// A committed transcript fragment. Once a [`Segment`] is produced (by a
/// silence reset), it is immutable: subsequent passes will not rewrite it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
}

/// The full transcript visible to the user for the current recording session.
/// Equal to `committed.join(" ")` + `" " + live_tail` (when non-empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub committed: Vec<Segment>,
    pub live_tail: String,
}

impl Transcript {
    /// Render the transcript as a single string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.committed.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&seg.text);
        }
        if !self.live_tail.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&self.live_tail);
        }
        out
    }
}

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("model load failed: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("model not yet loaded")]
    NotLoaded,
}

/// A callback the engine fires whenever the transcript changes.
pub type TranscriptCallback = Arc<dyn Fn(&Transcript) + Send + Sync>;

/// Implemented by every ASR backend (`SherpaEngine`, eventual `ParakeetRsEngine`).
pub trait AsrEngine: Send {
    /// Eagerly load the model. May block for tens of seconds.
    fn load(&mut self) -> Result<(), AsrError>;

    /// Begin a recording session. The engine starts emitting transcripts via
    /// the callback set at construction.
    fn start(&mut self) -> Result<(), AsrError>;

    /// End the recording session. The engine flushes any pending audio and
    /// emits a final transcript before returning.
    fn stop(&mut self) -> Result<(), AsrError>;

    /// Describe the input device this engine is using. Informational.
    fn input_device_name(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_transcript_renders_empty() {
        let t = Transcript::default();
        assert_eq!(t.render(), "");
    }

    #[test]
    fn segments_only() {
        let t = Transcript {
            committed: vec![
                Segment {
                    text: "hello".into(),
                },
                Segment {
                    text: "world".into(),
                },
            ],
            live_tail: String::new(),
        };
        assert_eq!(t.render(), "hello world");
    }

    #[test]
    fn segments_plus_tail() {
        let t = Transcript {
            committed: vec![Segment {
                text: "hello".into(),
            }],
            live_tail: "there friend".into(),
        };
        assert_eq!(t.render(), "hello there friend");
    }

    #[test]
    fn tail_only() {
        let t = Transcript {
            committed: vec![],
            live_tail: "fresh".into(),
        };
        assert_eq!(t.render(), "fresh");
    }
}
