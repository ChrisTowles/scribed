//! Sherpa-onnx Parakeet-TDT-0.6B-v2 backend.
//!
//! Implements [`Transcriber`] by wrapping `sherpa_rs::transducer::TransducerRecognizer`.
//! The recognizer is "offline" in sherpa-onnx terminology — meaning it
//! transcribes a complete audio buffer in one shot. We get the streaming feel
//! by re-running it on the rolling buffer every chunk (see [`crate::asr::driver`]).
//!
//! This module compiles only with `--features asr` because sherpa-rs links
//! against native libraries.

#![cfg(feature = "asr")]

use std::path::{Path, PathBuf};

use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

use crate::asr::{driver::Transcriber, AsrError};
use crate::audio::SAMPLE_RATE_HZ;

/// Where on disk the Parakeet bundle lives. The bundle is the four-file
/// directory sherpa-onnx ships:
///
/// ```text
/// sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/
///   ├── encoder.onnx
///   ├── decoder.onnx
///   ├── joiner.onnx
///   └── tokens.txt
/// ```
#[derive(Debug, Clone)]
pub struct ModelBundle {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelBundle {
    /// Standard layout under a directory.
    pub fn from_dir(dir: &Path) -> Self {
        Self {
            encoder: dir.join("encoder.onnx"),
            decoder: dir.join("decoder.onnx"),
            joiner: dir.join("joiner.onnx"),
            tokens: dir.join("tokens.txt"),
        }
    }

    pub fn validate(&self) -> Result<(), AsrError> {
        for (label, p) in [
            ("encoder.onnx", &self.encoder),
            ("decoder.onnx", &self.decoder),
            ("joiner.onnx", &self.joiner),
            ("tokens.txt", &self.tokens),
        ] {
            if !p.exists() {
                return Err(AsrError::Load(format!(
                    "missing {label} at {}",
                    p.display()
                )));
            }
        }
        Ok(())
    }
}

/// A `Transcriber` backed by a sherpa-onnx offline Parakeet recognizer.
pub struct SherpaTranscriber {
    inner: TransducerRecognizer,
}

impl SherpaTranscriber {
    /// Load the recognizer. May take a few seconds.
    ///
    /// `provider` is a sherpa-onnx execution-provider string. Common values:
    /// `"cpu"` (default), `"cuda"`, `"coreml"`. If the provider isn't compiled
    /// in, sherpa-onnx falls back to CPU.
    pub fn load(bundle: &ModelBundle, provider: &str, num_threads: i32) -> Result<Self, AsrError> {
        bundle.validate()?;
        let config = TransducerConfig {
            encoder: bundle.encoder.to_string_lossy().into_owned(),
            decoder: bundle.decoder.to_string_lossy().into_owned(),
            joiner: bundle.joiner.to_string_lossy().into_owned(),
            tokens: bundle.tokens.to_string_lossy().into_owned(),
            // The Parakeet bundle's README specifies "nemo_transducer" as the
            // model type so sherpa-onnx applies the correct decoding path.
            model_type: "nemo_transducer".to_string(),
            num_threads,
            sample_rate: SAMPLE_RATE_HZ as i32,
            feature_dim: 80,
            decoding_method: "greedy_search".to_string(),
            provider: Some(provider.to_string()),
            ..Default::default()
        };
        let inner =
            TransducerRecognizer::new(config).map_err(|e| AsrError::Load(format!("{e:?}")))?;
        Ok(Self { inner })
    }
}

impl Transcriber for SherpaTranscriber {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, AsrError> {
        let text = self.inner.transcribe(SAMPLE_RATE_HZ, audio);
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn from_dir_lays_out_canonical_filenames() {
        let dir = std::path::Path::new("/tmp/parakeet");
        let b = ModelBundle::from_dir(dir);
        assert_eq!(b.encoder, dir.join("encoder.onnx"));
        assert_eq!(b.tokens, dir.join("tokens.txt"));
    }

    #[test]
    fn validate_complains_about_missing_files() {
        let dir = tempdir().unwrap();
        let b = ModelBundle::from_dir(dir.path());
        let err = b.validate().unwrap_err();
        assert!(matches!(err, AsrError::Load(_)));
    }

    #[test]
    fn validate_passes_with_all_files_present() {
        let dir = tempdir().unwrap();
        for name in ["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"] {
            fs::write(dir.path().join(name), b"stub").unwrap();
        }
        let b = ModelBundle::from_dir(dir.path());
        b.validate().unwrap();
    }
}
