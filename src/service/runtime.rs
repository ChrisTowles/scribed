//! Per-session recording runtime.
//!
//! The daemon loads one [`Runtime`] at startup (model + backend), then calls
//! [`Runtime::start_session`] / [`Runtime::stop_session`] every time the user
//! toggles. Each session captures audio into a buffer on a dedicated thread;
//! every chunk is fed to a [`StreamingDriver`] which transcribes the rolling
//! buffer and emits an evolving transcript. The diff between successive
//! transcripts is pushed through the [`KeyboardSink`] so text appears in the
//! focused window as the user speaks.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::asr::driver::{DriverConfig, StreamingDriver};
use crate::asr::sherpa::{ModelBundle, SherpaTranscriber};
use crate::asr::{AsrError, Transcript};
use crate::audio::{self, AudioChunk, SAMPLE_RATE_HZ};
use crate::config::Config;
use crate::output::backend;
use crate::output::retype::{KeyboardSink, RetypeState};

pub struct Runtime {
    transcriber: Arc<Mutex<SherpaTranscriber>>,
    backend: Arc<Mutex<Box<dyn KeyboardSink + Send>>>,
    input_device: String,
    chunk_samples: usize,
    driver_config: DriverConfig,
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
            driver_config: DriverConfig::from_config(config),
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
        let driver_config = self.driver_config.clone();
        thread::Builder::new()
            .name("scribed-session".into())
            .spawn(move || {
                record_and_stream(
                    input_device,
                    chunk_samples,
                    driver_config,
                    stop_thread,
                    transcriber,
                    backend,
                );
            })
            .expect("spawn session thread");
        self.current_stop = Some(stop);
    }

    /// Signals the in-flight session to wind down (capture stops, pending
    /// audio is drained through one final inference, the diff is typed).
    /// Returns immediately; the worker thread is detached.
    pub fn stop_session(&mut self) {
        if let Some(flag) = self.current_stop.take() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

fn record_and_stream(
    input_device: String,
    chunk_samples: usize,
    driver_config: DriverConfig,
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

    let mut driver = StreamingDriver::new(driver_config);
    let mut retype = RetypeState::new();
    // Coalesce partials so we don't spawn a ydotool subprocess per chunk
    // (~3 Hz at default config). The freshest pending transcript wins.
    let mut pending: Option<Transcript> = None;
    let mut last_apply: Option<Instant> = None;
    const PARTIAL_DEBOUNCE: Duration = Duration::from_millis(200);

    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                if let Some(t) = transcribe_chunk(&chunk, &mut driver, &transcriber) {
                    pending = Some(t);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
        if pending.is_some() && last_apply.map_or(true, |i| i.elapsed() >= PARTIAL_DEBOUNCE) {
            let t = pending.take().unwrap();
            apply_transcript(&t, &mut retype, &backend);
            last_apply = Some(Instant::now());
        }
    }

    drop(stream);

    // Drain any chunks captured between the stop signal and stream teardown.
    // We discard `pending` here — finalize() will produce the authoritative
    // transcript and apply_transcript will diff to it directly.
    while let Ok(chunk) = rx.try_recv() {
        let _ = transcribe_chunk(&chunk, &mut driver, &transcriber);
    }

    let mut tr = transcriber.lock();
    match driver.finalize(&mut *tr) {
        Ok(final_transcript) => {
            drop(tr);
            apply_transcript(&final_transcript, &mut retype, &backend);
            tracing::info!(text = %final_transcript.render(), "session finalized");
        }
        Err(e) => tracing::error!(?e, "finalize failed"),
    }
}

fn transcribe_chunk(
    chunk: &[f32],
    driver: &mut StreamingDriver,
    transcriber: &Arc<Mutex<SherpaTranscriber>>,
) -> Option<Transcript> {
    let mut tr = transcriber.lock();
    let outcome = driver.ingest(chunk, &mut *tr);
    drop(tr);
    match outcome {
        Ok(maybe) => maybe,
        Err(e) => {
            tracing::error!(?e, "ingest failed");
            None
        }
    }
}

fn apply_transcript(
    transcript: &Transcript,
    retype: &mut RetypeState,
    backend: &Arc<Mutex<Box<dyn KeyboardSink + Send>>>,
) {
    let text = transcript.render();
    let step = retype.diff(&text);
    if step.is_noop() {
        return;
    }
    let mut be = backend.lock();
    if let Err(e) = be.apply(step) {
        tracing::error!(?e, "backend apply failed");
    }
}
