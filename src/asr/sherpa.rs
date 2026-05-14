//! Streaming sherpa-onnx backend.
//!
//! RAII wrappers around the `SherpaOnnxOnlineRecognizer` C API. `sherpa-rs`
//! 0.6 has no high-level Rust binding for the online recognizer, so we call
//! `sherpa_rs_sys` directly (re-exported via the `sherpa-rs/sys` feature).

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use sherpa_rs::sherpa_rs_sys as sys;

use crate::asr::driver::{StreamingTranscriber, StreamingUpdate};
use crate::asr::{AsrError, EndpointRules};
use crate::audio::{SAMPLE_RATE_HZ_I32};

/// File layout for a sherpa-onnx streaming Zipformer transducer bundle.
/// `from_dir` auto-detects `encoder*.onnx` / `decoder*.onnx` / `joiner*.onnx`
/// (preferring `.int8.onnx`), so it works for both canonical-named bundles
/// and k2-fsa's epoch-suffixed releases.
#[derive(Debug, Clone)]
pub struct ModelBundle {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelBundle {
    pub fn from_dir(dir: &Path) -> Self {
        Self {
            encoder: find_onnx(dir, "encoder"),
            decoder: find_onnx(dir, "decoder"),
            joiner: find_onnx(dir, "joiner"),
            tokens: dir.join("tokens.txt"),
        }
    }

    pub fn validate(&self) -> Result<(), AsrError> {
        for (label, p) in [
            ("encoder", &self.encoder),
            ("decoder", &self.decoder),
            ("joiner", &self.joiner),
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

/// Pick the best matching `<role>*.onnx` file in `dir`. Quantized
/// (`.int8.onnx`) wins when both are present.
fn find_onnx(dir: &Path, role: &str) -> PathBuf {
    let mut quantized: Option<PathBuf> = None;
    let mut plain: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.starts_with(role) {
                continue;
            }
            if name.ends_with(".int8.onnx") {
                quantized = Some(path);
            } else if name.ends_with(".onnx") {
                plain = Some(path);
            }
        }
    }
    quantized
        .or(plain)
        // Fall back to the canonical name so `validate()` produces a useful
        // error message rather than silently pointing at the directory.
        .unwrap_or_else(|| dir.join(format!("{role}.onnx")))
}

/// Load-time configuration for [`SherpaStreamingTranscriber`].
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub provider: String,
    pub num_threads: i32,
    pub endpoint_rules: EndpointRules,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            provider: "cpu".to_string(),
            num_threads: 1,
            endpoint_rules: EndpointRules::default(),
        }
    }
}

/// RAII handle: holds the heavy ONNX session. One recognizer mints many streams.
struct OnlineRecognizer {
    ptr: *const sys::SherpaOnnxOnlineRecognizer,
}

// Safety: reentrancy-safe as long as a given stream is touched from one
// thread at a time, which scribed honors (one `&mut SherpaStreamingTranscriber`
// per session thread).
unsafe impl Send for OnlineRecognizer {}
unsafe impl Sync for OnlineRecognizer {}

impl Drop for OnlineRecognizer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { sys::SherpaOnnxDestroyOnlineRecognizer(self.ptr) };
        }
    }
}

/// RAII handle to a sherpa-onnx online stream (one per utterance).
struct OnlineStream {
    ptr: *const sys::SherpaOnnxOnlineStream,
}

unsafe impl Send for OnlineStream {}

impl Drop for OnlineStream {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { sys::SherpaOnnxDestroyOnlineStream(self.ptr) };
        }
    }
}

/// Streaming Zipformer transducer backed by sherpa-onnx. Implements
/// [`StreamingTranscriber`].
//
// Field order matters: Rust drops fields in declaration order, and sherpa-onnx
// requires every OnlineStream to be destroyed BEFORE the OnlineRecognizer that
// minted it. `stream` must precede `recognizer` here.
pub struct SherpaStreamingTranscriber {
    stream: Option<OnlineStream>,
    recognizer: OnlineRecognizer,
    /// Last hypothesis text as raw C bytes. Sherpa may emit non-UTF-8 byte
    /// sequences (e.g. CJK punctuation models); comparing raw bytes against
    /// the next poll's `CStr::to_bytes()` keeps the dedup cache correct
    /// regardless of how UTF-8-lossy decoding rewrites the string we expose.
    last_partial_bytes: Vec<u8>,
}

impl SherpaStreamingTranscriber {
    pub fn load(bundle: &ModelBundle, config: &StreamingConfig) -> Result<Self, AsrError> {
        bundle.validate()?;

        // CStrings must live until SherpaOnnxCreateOnlineRecognizer returns —
        // sherpa-onnx copies them into C++ std::string internally.
        let encoder = path_cstring(&bundle.encoder)?;
        let decoder = path_cstring(&bundle.decoder)?;
        let joiner = path_cstring(&bundle.joiner)?;
        let tokens = path_cstring(&bundle.tokens)?;
        let provider = CString::new(config.provider.as_str())
            .map_err(|e| AsrError::Load(format!("provider has NUL: {e}")))?;
        let decoding_method = CString::new("greedy_search").unwrap();

        // Zero-init: pointer fields default to NULL, ints to 0, both meaning
        // "unset" in sherpa-onnx. We overwrite only what we need.
        let mut cfg: sys::SherpaOnnxOnlineRecognizerConfig = unsafe { std::mem::zeroed() };
        cfg.feat_config.sample_rate = SAMPLE_RATE_HZ_I32;
        cfg.feat_config.feature_dim = 80;
        cfg.model_config.transducer.encoder = encoder.as_ptr();
        cfg.model_config.transducer.decoder = decoder.as_ptr();
        cfg.model_config.transducer.joiner = joiner.as_ptr();
        cfg.model_config.tokens = tokens.as_ptr();
        cfg.model_config.num_threads = config.num_threads;
        cfg.model_config.provider = provider.as_ptr();
        // model_type left NULL: sherpa-onnx reads it from the encoder.onnx metadata.
        cfg.decoding_method = decoding_method.as_ptr();
        cfg.max_active_paths = 4;
        cfg.enable_endpoint = 1;
        cfg.rule1_min_trailing_silence = config.endpoint_rules.rule1_min_trailing_silence;
        cfg.rule2_min_trailing_silence = config.endpoint_rules.rule2_min_trailing_silence;
        cfg.rule3_min_utterance_length = config.endpoint_rules.rule3_max_utterance_seconds;

        let ptr = unsafe { sys::SherpaOnnxCreateOnlineRecognizer(&cfg) };
        if ptr.is_null() {
            return Err(AsrError::Load(
                "SherpaOnnxCreateOnlineRecognizer returned null (check model paths and provider)"
                    .to_string(),
            ));
        }

        let mut me = Self {
            stream: None,
            recognizer: OnlineRecognizer { ptr },
            last_partial_bytes: Vec::new(),
        };
        me.open_stream()?;
        Ok(me)
    }

    fn open_stream(&mut self) -> Result<(), AsrError> {
        let s = unsafe { sys::SherpaOnnxCreateOnlineStream(self.recognizer.ptr) };
        if s.is_null() {
            return Err(AsrError::Inference(
                "SherpaOnnxCreateOnlineStream returned null".to_string(),
            ));
        }
        self.stream = Some(OnlineStream { ptr: s });
        self.last_partial_bytes.clear();
        Ok(())
    }

    fn stream_ptr(&self) -> Result<*const sys::SherpaOnnxOnlineStream, AsrError> {
        self.stream
            .as_ref()
            .map(|s| s.ptr)
            .ok_or(AsrError::NotLoaded)
    }

    fn poll_once(&mut self) -> Result<StreamingUpdate, AsrError> {
        let rec = self.recognizer.ptr;
        let stream = self.stream_ptr()?;

        let mut decoded = false;
        while unsafe { sys::SherpaOnnxIsOnlineStreamReady(rec, stream) } != 0 {
            unsafe { sys::SherpaOnnxDecodeOnlineStream(rec, stream) };
            decoded = true;
        }

        let is_endpoint = unsafe { sys::SherpaOnnxOnlineStreamIsEndpoint(rec, stream) } != 0;

        if is_endpoint {
            let text = self.with_result(|bytes| String::from_utf8_lossy(bytes).into_owned())?;
            // Sherpa requires Reset after consuming an endpoint, otherwise
            // the next decode pass keeps emitting the same committed text.
            unsafe { sys::SherpaOnnxOnlineStreamReset(rec, stream) };
            self.last_partial_bytes.clear();
            return Ok(StreamingUpdate::Endpoint(text));
        }

        if !decoded {
            return Ok(StreamingUpdate::Idle);
        }

        // Compare raw bytes against last_partial_bytes inside the FFI lease so
        // we only allocate when the hypothesis actually changed. Sherpa emits
        // the same text between frames whenever a decode pass produces no new
        // tokens; this skip dominates poll cost at ~10 Hz.
        let last_bytes = self.last_partial_bytes.as_slice();
        let changed: Option<(String, Vec<u8>)> = self.with_result(|bytes| {
            if bytes == last_bytes {
                None
            } else {
                Some((String::from_utf8_lossy(bytes).into_owned(), bytes.to_vec()))
            }
        })?;
        match changed {
            None => Ok(StreamingUpdate::Idle),
            Some((text, bytes)) => {
                self.last_partial_bytes = bytes;
                Ok(StreamingUpdate::Partial(text))
            }
        }
    }

    /// Acquire the current recognizer result, hand its UTF-8 bytes to `f`,
    /// and free the C-side handle whether `f` panics or returns. `f` receives
    /// an empty slice when the result or its text pointer is null.
    fn with_result<T>(&self, f: impl FnOnce(&[u8]) -> T) -> Result<T, AsrError> {
        let rec = self.recognizer.ptr;
        let stream = self.stream_ptr()?;
        let result = unsafe { sys::SherpaOnnxGetOnlineStreamResult(rec, stream) };
        if result.is_null() {
            return Ok(f(&[]));
        }
        // SAFETY: result is non-null and owned by us until DestroyOnlineRecognizerResult.
        let out = unsafe {
            let text_ptr = (*result).text;
            let bytes: &[u8] = if text_ptr.is_null() {
                &[]
            } else {
                CStr::from_ptr(text_ptr).to_bytes()
            };
            f(bytes)
        };
        unsafe { sys::SherpaOnnxDestroyOnlineRecognizerResult(result) };
        Ok(out)
    }
}

impl StreamingTranscriber for SherpaStreamingTranscriber {
    fn accept_waveform(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        if samples.is_empty() {
            return Ok(());
        }
        let stream = self.stream_ptr()?;
        unsafe {
            sys::SherpaOnnxOnlineStreamAcceptWaveform(
                stream,
                SAMPLE_RATE_HZ_I32,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<StreamingUpdate, AsrError> {
        self.poll_once()
    }

    fn input_finished(&mut self) -> Result<(), AsrError> {
        let stream = self.stream_ptr()?;
        unsafe { sys::SherpaOnnxOnlineStreamInputFinished(stream) };
        Ok(())
    }

    fn reset(&mut self) -> Result<(), AsrError> {
        // Destroy + recreate the stream rather than calling OnlineStreamReset:
        // also flushes any feature frames the previous stream had queued.
        self.stream = None;
        self.open_stream()
    }
}

fn path_cstring(p: &Path) -> Result<CString, AsrError> {
    let s = p
        .to_str()
        .ok_or_else(|| AsrError::Load(format!("non-UTF-8 path: {}", p.display())))?;
    CString::new(s).map_err(|e| AsrError::Load(format!("path has NUL: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn from_dir_falls_back_to_plain_onnx_when_no_int8() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("encoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("decoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("joiner.onnx"), b"stub").unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert_eq!(b.encoder, dir.path().join("encoder.onnx"));
        assert_eq!(b.tokens, dir.path().join("tokens.txt"));
    }

    #[test]
    fn from_dir_picks_int8_when_present() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("encoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("encoder.int8.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("decoder.int8.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("joiner.int8.onnx"), b"stub").unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert_eq!(b.encoder, dir.path().join("encoder.int8.onnx"));
        assert_eq!(b.decoder, dir.path().join("decoder.int8.onnx"));
        assert_eq!(b.joiner, dir.path().join("joiner.int8.onnx"));
    }

    #[test]
    fn from_dir_matches_long_filenames() {
        // k2-fsa publishes models with descriptive filenames like
        // `encoder-epoch-99-avg-1-chunk-16-left-128.onnx`.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("encoder-epoch-99-avg-1-chunk-16-left-128.onnx"),
            b"stub",
        )
        .unwrap();
        fs::write(
            dir.path().join("decoder-epoch-99-avg-1-chunk-16-left-128.onnx"),
            b"stub",
        )
        .unwrap();
        fs::write(
            dir.path().join("joiner-epoch-99-avg-1-chunk-16-left-128.onnx"),
            b"stub",
        )
        .unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert!(b.encoder.file_name().unwrap().to_str().unwrap().starts_with("encoder-"));
        assert!(b.decoder.file_name().unwrap().to_str().unwrap().starts_with("decoder-"));
        assert!(b.joiner.file_name().unwrap().to_str().unwrap().starts_with("joiner-"));
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
        for name in [
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ] {
            fs::write(dir.path().join(name), b"stub").unwrap();
        }
        let b = ModelBundle::from_dir(dir.path());
        b.validate().unwrap();
    }
}
