//! Per-session recording runtime.
//!
//! The daemon loads one [`Runtime`] at startup (model + backend), then calls
//! [`Runtime::start_session`] / [`Runtime::stop_session`] every time the user
//! toggles. Each session captures audio on a dedicated thread; every chunk is
//! fed to a streaming recognizer and the resulting partial / endpoint events
//! are diffed through the [`KeyboardSink`] so text appears in the focused
//! window as the user speaks.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::asr::sherpa::{ModelBundle, SherpaStreamingTranscriber, StreamingConfig};
use crate::asr::{AsrError, StreamingDriver, StreamingTranscriber, Transcript};
use crate::audio::{self, AudioChunk, SAMPLE_RATE_HZ};
use crate::config::Config;
use crate::output::backend;
use crate::output::retype::{KeyboardSink, RetypeState};

/// Minimum wall-clock gap between successive backend.apply() calls. Coalesces
/// the rapid Partial(_) updates streaming RNN-T produces (often several per
/// audio chunk) into one keystroke burst.
const PARTIAL_DEBOUNCE: Duration = Duration::from_millis(100);

/// Abort the session after this many consecutive FFI failures in a row.
/// Prevents an error storm if the recognizer ends up in a permanently bad
/// state (e.g. dropped GPU context).
const MAX_CONSECUTIVE_INGEST_ERRORS: u32 = 50;

type SharedTranscriber = Arc<Mutex<dyn StreamingTranscriber + Send>>;
type SharedBackend = Arc<Mutex<Box<dyn KeyboardSink + Send>>>;

struct SessionHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// Snapshot of the user-tunable durations that govern a single session.
#[derive(Debug, Clone, Copy)]
struct SessionLimits {
    /// Hard ceiling on wall-clock recording time. 0 disables.
    max_recording: Duration,
    /// Stop after this much wall-clock time without a new committed segment.
    /// 0 disables.
    silence_auto_stop: Duration,
}

impl SessionLimits {
    fn from_config(c: &Config) -> Self {
        Self {
            max_recording: Duration::from_secs(c.max_recording_seconds as u64),
            silence_auto_stop: Duration::from_secs(c.silence_auto_stop_seconds as u64),
        }
    }
}

pub struct Runtime {
    transcriber: SharedTranscriber,
    backend: SharedBackend,
    input_device: String,
    chunk_samples: usize,
    limits: SessionLimits,
    current_session: Option<SessionHandle>,
}

impl Runtime {
    /// Load the ASR model and resolve the keyboard backend. Blocks for a few
    /// seconds on first call (model load).
    pub fn load(config: &Config, model_dir: PathBuf) -> Result<Self, AsrError> {
        let bundle = ModelBundle::from_dir(&model_dir);
        let streaming_cfg = StreamingConfig {
            endpoint_rules: config.endpoint_rules(),
            ..StreamingConfig::default()
        };
        let t = Instant::now();
        let transcriber = SherpaStreamingTranscriber::load(&bundle, &streaming_cfg)?;
        tracing::info!(elapsed = ?t.elapsed(), dir = %model_dir.display(), "asr model loaded");

        let backend = backend::auto_detect();
        tracing::info!(kind = %backend::select_backend_kind().as_str(), "keyboard backend ready");

        Ok(Self {
            transcriber: Arc::new(Mutex::new(transcriber)),
            backend: Arc::new(Mutex::new(backend)),
            input_device: config.input_device.clone(),
            chunk_samples: config.chunk_samples(SAMPLE_RATE_HZ),
            limits: SessionLimits::from_config(config),
            current_session: None,
        })
    }

    /// True if a session thread is currently active. Reaps finished handles
    /// as a side effect so a session that died on its own (capture error,
    /// FFI panic ride-out, etc.) doesn't block future starts.
    pub fn is_recording(&mut self) -> bool {
        if let Some(handle) = self.current_session.as_mut() {
            if handle.join.as_ref().map_or(true, |j| j.is_finished()) {
                if let Some(j) = handle.join.take() {
                    let _ = j.join();
                }
                self.current_session = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Idempotent: a second call while a session is active is a no-op.
    pub fn start_session(&mut self) {
        if self.is_recording() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let transcriber = self.transcriber.clone();
        let backend = self.backend.clone();
        let input_device = self.input_device.clone();
        let chunk_samples = self.chunk_samples;
        let limits = self.limits;
        let join = thread::Builder::new()
            .name("scribed-session".into())
            .spawn(move || {
                record_and_stream(
                    input_device,
                    chunk_samples,
                    limits,
                    stop_thread,
                    transcriber,
                    backend,
                );
            })
            .expect("spawn session thread");
        self.current_session = Some(SessionHandle {
            stop,
            join: Some(join),
        });
    }

    /// Signal the in-flight session to wind down and wait briefly for it to
    /// finish. Returns whether the thread actually joined within the grace
    /// period; on timeout the handle is dropped (best-effort cleanup) and
    /// future `is_recording()` calls will reap it once it does finish.
    pub fn stop_session(&mut self) -> bool {
        let Some(mut handle) = self.current_session.take() else {
            return true;
        };
        handle.stop.store(true, Ordering::SeqCst);
        let Some(join) = handle.join.take() else {
            return true;
        };
        // Brief poll loop — finalize() + apply_transcript usually completes
        // in <50 ms. Anything longer probably means the backend (ydotool)
        // is hung; we don't want to block the hotkey thread forever.
        let deadline = Instant::now() + Duration::from_millis(500);
        while !join.is_finished() {
            if Instant::now() >= deadline {
                tracing::warn!("session thread did not finish within 500ms; detaching");
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = join.join();
        true
    }
}

fn record_and_stream(
    input_device: String,
    chunk_samples: usize,
    limits: SessionLimits,
    stop: Arc<AtomicBool>,
    transcriber: SharedTranscriber,
    backend: SharedBackend,
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

    // Each session gets a fresh decoder state so a previous session's
    // dangling tokens never bleed into the new one.
    if let Err(e) = transcriber.lock().reset() {
        tracing::error!(?e, "transcriber reset failed");
        return;
    }

    let mut driver = StreamingDriver::new();
    let mut retype = RetypeState::new();
    let mut pending: Option<Transcript> = None;
    let mut last_apply: Option<Instant> = None;
    let mut consecutive_errors: u32 = 0;
    let session_started = Instant::now();
    let mut last_committed_change = session_started;
    let mut last_committed_count: usize = 0;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if stream.errored() {
            tracing::error!("cpal capture stream reported a fatal error; ending session");
            break;
        }
        if limits.max_recording > Duration::ZERO
            && session_started.elapsed() >= limits.max_recording
        {
            tracing::info!(?limits.max_recording, "max_recording_seconds reached, stopping");
            break;
        }
        if limits.silence_auto_stop > Duration::ZERO
            && last_committed_change.elapsed() >= limits.silence_auto_stop
        {
            tracing::info!(?limits.silence_auto_stop, "silence_auto_stop_seconds reached, stopping");
            break;
        }

        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => match ingest_chunk(&chunk, &mut driver, &transcriber) {
                Ok(Some(t)) => {
                    consecutive_errors = 0;
                    if t.committed.len() != last_committed_count {
                        last_committed_count = t.committed.len();
                        last_committed_change = Instant::now();
                    }
                    pending = Some(t);
                }
                Ok(None) => {
                    consecutive_errors = 0;
                }
                Err(()) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_INGEST_ERRORS {
                        tracing::error!(
                            count = consecutive_errors,
                            "consecutive ingest errors exceeded threshold; aborting session"
                        );
                        break;
                    }
                }
            },
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

    // Drain any chunks that landed between the stop signal and stream
    // teardown. We discard `pending` because finalize() will produce the
    // authoritative end-of-session transcript anyway.
    while let Ok(chunk) = rx.try_recv() {
        let _ = ingest_chunk(&chunk, &mut driver, &transcriber);
    }

    let mut tr = transcriber.lock();
    let outcome = driver.finalize(&mut *tr);
    drop(tr);
    match outcome {
        Ok(final_transcript) => {
            apply_transcript(&final_transcript, &mut retype, &backend);
            tracing::info!(text = %final_transcript.render(), "session finalized");
        }
        Err(e) => tracing::error!(?e, "finalize failed"),
    }
}

/// Returns `Ok(Some(t))` if the transcript changed, `Ok(None)` for a clean
/// no-change ingest, `Err(())` if the recognizer failed. The Err signals the
/// caller to increment its consecutive-error counter.
fn ingest_chunk(
    chunk: &[f32],
    driver: &mut StreamingDriver,
    transcriber: &SharedTranscriber,
) -> Result<Option<Transcript>, ()> {
    let mut tr = transcriber.lock();
    match driver.ingest(chunk, &mut *tr) {
        Ok(maybe) => Ok(maybe),
        Err(e) => {
            tracing::error!(?e, "ingest failed");
            Err(())
        }
    }
}

/// Render the transcript and push the keystroke diff to the backend. If the
/// backend fails, reset `retype` so the next non-empty partial types fresh —
/// the window is in an indeterminate state and pretending otherwise would
/// drift typed_text further and further from reality with every subsequent
/// diff.
fn apply_transcript(transcript: &Transcript, retype: &mut RetypeState, backend: &SharedBackend) {
    let text = transcript.render();
    let snapshot = retype.clone();
    let step = retype.diff(&text);
    if step.is_noop() {
        return;
    }
    let mut be = backend.lock();
    if let Err(e) = be.apply(step) {
        tracing::error!(
            ?e,
            "backend apply failed; resetting RetypeState (focused window is in an unknown state)"
        );
        *retype = snapshot;
        retype.reset();
    }
}
