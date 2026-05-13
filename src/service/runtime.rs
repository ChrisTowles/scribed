//! Per-session recording runtime.
//!
//! The daemon loads one [`Runtime`] at startup (model + backend), then calls
//! [`Runtime::start_session`] / [`Runtime::stop_session`] every time the user
//! toggles. Each session captures audio into a buffer on a dedicated thread;
//! at stop the buffer is sent through the loaded `SherpaTranscriber` and the
//! resulting text is pushed through the [`KeyboardSink`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::asr::driver::Transcriber;
use crate::asr::sherpa::{ModelBundle, SherpaTranscriber};
use crate::asr::AsrError;
use crate::audio::{self, AudioChunk, SAMPLE_RATE_HZ};
use crate::config::Config;
use crate::output::backend;
use crate::output::retype::{KeyboardSink, RetypeStep};

pub struct Runtime {
    transcriber: Arc<Mutex<SherpaTranscriber>>,
    backend: Arc<Mutex<Box<dyn KeyboardSink + Send>>>,
    input_device: String,
    chunk_samples: usize,
    current_stop: Option<Arc<AtomicBool>>,
}

impl Runtime {
    /// Load the ASR model and resolve the keyboard backend. Blocks for a few
    /// seconds on first call (model load).
    pub fn load(config: &Config, model_dir: PathBuf) -> Result<Self, AsrError> {
        let bundle = ModelBundle::from_dir(&model_dir);
        let t = Instant::now();
        let transcriber = SherpaTranscriber::load(&bundle, "cpu", 4)?;
        tracing::info!(elapsed = ?t.elapsed(), dir = %model_dir.display(), "asr model loaded");

        let backend = backend::auto_detect();
        tracing::info!(kind = %backend::select_backend_kind().as_str(), "keyboard backend ready");

        Ok(Self {
            transcriber: Arc::new(Mutex::new(transcriber)),
            backend: Arc::new(Mutex::new(backend)),
            input_device: config.input_device.clone(),
            chunk_samples: config.chunk_samples(SAMPLE_RATE_HZ),
            current_stop: None,
        })
    }

    pub fn is_recording(&self) -> bool {
        self.current_stop.is_some()
    }

    /// Idempotent: a second call while a session is active is a no-op.
    pub fn start_session(&mut self) {
        if self.current_stop.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let transcriber = self.transcriber.clone();
        let backend = self.backend.clone();
        let input_device = self.input_device.clone();
        let chunk_samples = self.chunk_samples;
        thread::Builder::new()
            .name("scribed-session".into())
            .spawn(move || {
                record_and_transcribe(
                    input_device,
                    chunk_samples,
                    stop_thread,
                    transcriber,
                    backend,
                );
            })
            .expect("spawn session thread");
        self.current_stop = Some(stop);
    }

    /// Signals the in-flight session to wind down (capture stops, final
    /// transcription runs, text is typed). Returns immediately; the worker
    /// thread is detached.
    pub fn stop_session(&mut self) {
        if let Some(flag) = self.current_stop.take() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

fn record_and_transcribe(
    input_device: String,
    chunk_samples: usize,
    stop: Arc<AtomicBool>,
    transcriber: Arc<Mutex<SherpaTranscriber>>,
    backend: Arc<Mutex<Box<dyn KeyboardSink + Send>>>,
) {
    let input = match audio::resolve_device(&input_device) {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(?e, "input device resolve failed");
            return;
        }
    };
    let device_name = input.name.clone();
    let (tx, rx) = crossbeam_channel::bounded::<AudioChunk>(64);
    let stream = match audio::capture::start(input, chunk_samples, tx) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(?e, "audio capture start failed");
            return;
        }
    };
    tracing::info!(
        device = %device_name,
        native_rate = stream.native_sample_rate,
        native_channels = stream.native_channels,
        "recording started"
    );

    let mut buffer: Vec<f32> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => buffer.extend(chunk),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    while let Ok(chunk) = rx.try_recv() {
        buffer.extend(chunk);
    }
    drop(stream);

    if buffer.is_empty() {
        tracing::warn!("recording produced no audio");
        return;
    }
    let seconds = buffer.len() as f32 / SAMPLE_RATE_HZ as f32;
    tracing::info!(samples = buffer.len(), seconds, "transcribing");

    let t = Instant::now();
    let text = {
        let mut tr = transcriber.lock();
        match tr.transcribe(&buffer) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(?e, "transcription failed");
                return;
            }
        }
    };
    let elapsed = t.elapsed();
    let trimmed = text.trim();
    tracing::info!(?elapsed, chars = trimmed.chars().count(), text = %trimmed, "transcribed");
    if trimmed.is_empty() {
        return;
    }
    let mut be = backend.lock();
    if let Err(e) = be.apply(RetypeStep {
        backspaces: 0,
        insert: trimmed,
    }) {
        tracing::error!(?e, "backend apply failed");
    }
}
