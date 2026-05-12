//! Sound playback via [`rodio`]. The cpal `OutputStream` underlying rodio is
//! not `Send + Sync` on Linux, so we put the playback engine behind a
//! dedicated worker thread: callers send [`SoundEvent`]s over a channel, the
//! worker decodes and plays them.
//!
//! Missing asset files are tolerated silently — the daemon still works without
//! sound assets installed.

use std::io::BufReader;
use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use rodio::{Decoder, OutputStream, Sink};

use super::SoundEvent;

pub struct SoundPlayer {
    tx: Sender<SoundEvent>,
}

impl SoundPlayer {
    pub fn new(assets_dir: PathBuf) -> Result<Self, String> {
        // Probe the output stream once on the main thread to surface init
        // errors synchronously. The actual sustained stream is owned by the
        // worker thread below.
        let _probe = OutputStream::try_default().map_err(|e| format!("output stream: {e:?}"))?;
        drop(_probe);

        let (tx, rx) = crossbeam_channel::bounded::<SoundEvent>(8);
        let dir = assets_dir;
        thread::Builder::new()
            .name("scribed-sound".into())
            .spawn(move || worker_loop(dir, rx))
            .map_err(|e| format!("sound worker spawn: {e}"))?;
        Ok(Self { tx })
    }

    pub fn play(&self, event: SoundEvent) {
        // Drop on overflow rather than block: missing one start-sound is
        // better than blocking the calling thread.
        let _ = self.tx.try_send(event);
    }
}

fn worker_loop(assets_dir: PathBuf, rx: Receiver<SoundEvent>) {
    let (stream, handle) = match OutputStream::try_default() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(?e, "sound worker: output stream init failed");
            return;
        }
    };
    // Hold the stream alive for the lifetime of this thread.
    let _stream_guard = stream;

    while let Ok(event) = rx.recv() {
        let path = event.asset_path(&assets_dir);
        if !path.exists() {
            tracing::debug!(?path, "sound asset missing; skipping");
            continue;
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(?path, ?e, "sound asset open failed");
                continue;
            }
        };
        let source = match Decoder::new(BufReader::new(file)) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(?path, ?e, "sound decode failed");
                continue;
            }
        };
        let sink = match Sink::try_new(&handle) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(?e, "sink alloc failed");
                continue;
            }
        };
        sink.append(source);
        sink.sleep_until_end();
    }
}
