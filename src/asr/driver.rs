//! The streaming driver — engine-agnostic logic that turns a stream of audio
//! chunks into evolving transcripts.
//!
//! Mirrors the rolling-buffer algorithm in `claude_stt/engines/nemo.py:174-253`.
//! Every chunk:
//!
//! 1. Compute energy. If below `silence_threshold_dbfs`, increment the silent
//!    counter; otherwise reset it.
//! 2. If the silent counter reaches `silence_reset_chunks`, commit the current
//!    live tail as a `Segment` and clear the rolling buffer (the "silence
//!    reset"). Skip transcription this round.
//! 3. Otherwise, append the chunk to the rolling buffer (dropping the oldest
//!    frames if past capacity) and run inference on the full buffer.
//! 4. Emit the new transcript (committed segments + live tail).
//!
//! Decoupling this from the actual model lets us unit-test the rolling-buffer
//! / silence semantics with a fake transcriber.

use crate::asr::{AsrError, Segment, Transcript};
use crate::audio::{rms_dbfs, RollingBuffer, SAMPLE_RATE_HZ};

/// Stateless transcription. Implementations: `SherpaEngine` (real), `FakeTranscriber` (tests).
pub trait Transcriber: Send {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, AsrError>;
}

/// Configuration the driver needs. A subset of `Config`; the orchestration
/// layer translates one into the other.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub rolling_buffer_samples: usize,
    pub silence_threshold_dbfs: f32,
    pub silence_reset_chunks: u32,
}

impl DriverConfig {
    pub fn from_config(c: &crate::config::Config) -> Self {
        Self {
            rolling_buffer_samples: c.rolling_buffer_samples(SAMPLE_RATE_HZ),
            silence_threshold_dbfs: c.silence_threshold_dbfs,
            silence_reset_chunks: c.silence_reset_chunks(),
        }
    }
}

/// The streaming driver. Owns the rolling buffer and the committed-segment
/// list for the current session. Reset between sessions via [`StreamingDriver::reset`].
pub struct StreamingDriver {
    config: DriverConfig,
    rolling: RollingBuffer,
    committed: Vec<Segment>,
    consecutive_silent_chunks: u32,
    speech_started: bool,
}

impl StreamingDriver {
    pub fn new(config: DriverConfig) -> Self {
        let capacity = config.rolling_buffer_samples;
        Self {
            config,
            rolling: RollingBuffer::new(capacity),
            committed: Vec::new(),
            consecutive_silent_chunks: 0,
            speech_started: false,
        }
    }

    pub fn reset(&mut self) {
        self.rolling.clear();
        self.committed.clear();
        self.consecutive_silent_chunks = 0;
        self.speech_started = false;
    }

    pub fn current_transcript(&self, live_tail: String) -> Transcript {
        Transcript {
            committed: self.committed.clone(),
            live_tail,
        }
    }

    /// Ingest one audio chunk. Returns:
    /// - `Some(transcript)` if the transcript changed
    /// - `None` if the chunk was silent before speech started (no transcription run)
    pub fn ingest(
        &mut self,
        chunk: &[f32],
        transcriber: &mut dyn Transcriber,
    ) -> Result<Option<Transcript>, AsrError> {
        let energy = rms_dbfs(chunk);
        let is_silent = energy < self.config.silence_threshold_dbfs;

        if is_silent {
            self.consecutive_silent_chunks += 1;
            if !self.speech_started {
                // Leading silence: don't even buffer it. Prevents hallucinations.
                return Ok(None);
            }
            // Mid-utterance silence: append to the buffer (we may still be in a pause)
            // but check for the reset threshold.
            self.rolling.push(chunk);
            if self.consecutive_silent_chunks >= self.config.silence_reset_chunks {
                self.commit_current_pass(transcriber)?;
                self.rolling.clear();
                self.speech_started = false;
                self.consecutive_silent_chunks = 0;
                return Ok(Some(self.current_transcript(String::new())));
            }
        } else {
            self.consecutive_silent_chunks = 0;
            self.speech_started = true;
            self.rolling.push(chunk);
        }

        if self.rolling.is_empty() {
            return Ok(None);
        }
        let audio = self.rolling.to_vec();
        let text = transcriber.transcribe(&audio)?;
        Ok(Some(self.current_transcript(text)))
    }

    /// Flush any pending audio and return the final transcript. Called at
    /// session end (hotkey release or auto-stop).
    pub fn finalize(&mut self, transcriber: &mut dyn Transcriber) -> Result<Transcript, AsrError> {
        if self.rolling.is_empty() {
            return Ok(self.current_transcript(String::new()));
        }
        let audio = self.rolling.to_vec();
        let text = transcriber.transcribe(&audio)?;
        if !text.trim().is_empty() {
            self.committed.push(Segment { text });
        }
        self.rolling.clear();
        Ok(self.current_transcript(String::new()))
    }

    fn commit_current_pass(&mut self, transcriber: &mut dyn Transcriber) -> Result<(), AsrError> {
        if self.rolling.is_empty() {
            return Ok(());
        }
        let audio = self.rolling.to_vec();
        let text = transcriber.transcribe(&audio)?;
        if !text.trim().is_empty() {
            self.committed.push(Segment { text });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns whatever string was last set on it. Useful for asserting on
    /// the driver's behavior without running a real model.
    #[derive(Default)]
    struct FakeTranscriber {
        next_text: String,
        calls: Vec<usize>,
    }

    impl Transcriber for FakeTranscriber {
        fn transcribe(&mut self, audio: &[f32]) -> Result<String, AsrError> {
            self.calls.push(audio.len());
            Ok(self.next_text.clone())
        }
    }

    fn cfg() -> DriverConfig {
        DriverConfig {
            rolling_buffer_samples: 16_000 * 30, // 30s
            silence_threshold_dbfs: -45.0,
            silence_reset_chunks: 3,
        }
    }

    /// Generate a chunk of audio at the given amplitude. `amp=0.0` is silence;
    /// `amp=0.1` is well above the -45 dBFS gate (`20*log10(0.1) = -20 dBFS`).
    fn chunk(samples: usize, amp: f32) -> Vec<f32> {
        (0..samples).map(|i| amp * (i as f32 * 0.1).sin()).collect()
    }

    #[test]
    fn leading_silence_is_skipped() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber::default();
        let silence = chunk(1600, 0.0);
        let result = d.ingest(&silence, &mut t).unwrap();
        assert!(result.is_none());
        assert!(t.calls.is_empty(), "should not transcribe leading silence");
    }

    #[test]
    fn first_speech_chunk_triggers_transcription() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "hello".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        let result = d.ingest(&speech, &mut t).unwrap().unwrap();
        assert_eq!(result.live_tail, "hello");
        assert!(result.committed.is_empty());
        assert_eq!(t.calls.len(), 1);
    }

    #[test]
    fn silence_reset_commits_segment_and_clears_buffer() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "hello world".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        let silence = chunk(1600, 0.0);
        d.ingest(&speech, &mut t).unwrap();
        // Three silent chunks should trigger the silence reset
        d.ingest(&silence, &mut t).unwrap();
        d.ingest(&silence, &mut t).unwrap();
        let result = d.ingest(&silence, &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "hello world");
        assert_eq!(result.live_tail, "");
    }

    #[test]
    fn post_reset_speech_starts_a_new_pass() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "first segment".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        let silence = chunk(1600, 0.0);
        d.ingest(&speech, &mut t).unwrap();
        for _ in 0..3 {
            d.ingest(&silence, &mut t).unwrap();
        }
        t.next_text = "second segment".into();
        let result = d.ingest(&speech, &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "first segment");
        assert_eq!(result.live_tail, "second segment");
    }

    #[test]
    fn finalize_commits_pending_live_tail() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "final".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        d.ingest(&speech, &mut t).unwrap();
        let result = d.finalize(&mut t).unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "final");
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn finalize_on_empty_buffer_returns_empty_transcript() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber::default();
        let result = d.finalize(&mut t).unwrap();
        assert!(result.committed.is_empty());
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn finalize_drops_empty_transcription() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        d.ingest(&speech, &mut t).unwrap();
        let result = d.finalize(&mut t).unwrap();
        assert!(
            result.committed.is_empty(),
            "empty transcription should not be committed"
        );
    }

    #[test]
    fn reset_returns_driver_to_pristine_state() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "a".into(),
            ..Default::default()
        };
        d.ingest(&chunk(1600, 0.2), &mut t).unwrap();
        d.reset();
        // After reset, leading silence should once again be skipped
        let result = d.ingest(&chunk(1600, 0.0), &mut t).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn brief_mid_utterance_silence_does_not_reset() {
        let mut d = StreamingDriver::new(cfg());
        let mut t = FakeTranscriber {
            next_text: "ongoing".into(),
            ..Default::default()
        };
        let speech = chunk(1600, 0.2);
        let silence = chunk(1600, 0.0);
        d.ingest(&speech, &mut t).unwrap();
        // Only 2 silent chunks; threshold is 3 in test config
        d.ingest(&silence, &mut t).unwrap();
        let result = d.ingest(&silence, &mut t).unwrap().unwrap();
        assert!(result.committed.is_empty(), "no reset yet");
        assert_eq!(result.live_tail, "ongoing");
    }
}
