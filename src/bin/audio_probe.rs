//! Development tool. Lists available cpal input devices and prints rolling
//! dBFS from the resolved mic at ~10 Hz until killed with Ctrl+C.
//!
//! Usage:
//!
//! ```text
//! audio_probe                    # list devices, then sample the default mic
//! audio_probe "Yeti"             # match a device by substring
//! audio_probe --list             # list devices and exit
//! ```

use std::time::{Duration, Instant};

use scribed::audio::{self, AudioChunk};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list_only = args.iter().any(|a| a == "--list" || a == "-l");
    let substring = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_default();

    println!("Input devices:");
    for (i, name) in audio::list_device_names()?.iter().enumerate() {
        println!("  [{i}] {name}");
    }
    if list_only {
        return Ok(());
    }
    println!();

    let resolved = audio::resolve_device(&substring)?;
    println!("Sampling from: {} (Ctrl+C to stop)", resolved.name);

    let chunk_samples = 1600; // 100 ms at 16 kHz
    let (tx, rx) = crossbeam_channel::bounded::<AudioChunk>(64);
    let stream = audio::capture::start(resolved, chunk_samples, tx)?;
    println!(
        "  native rate: {} Hz, channels: {}",
        stream.native_sample_rate, stream.native_channels
    );
    println!();

    let mut last_print = Instant::now();
    while let Ok(chunk) = rx.recv() {
        if last_print.elapsed() >= Duration::from_millis(100) {
            let db = audio::rms_dbfs(&chunk);
            let bar = bar_for_dbfs(db, 50);
            println!("{db:>+7.1} dBFS  {bar}");
            last_print = Instant::now();
        }
    }
    Ok(())
}

fn bar_for_dbfs(db: f32, width: usize) -> String {
    // Map -60..0 dBFS onto 0..width.
    let normalized = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let n = (normalized * width as f32).round() as usize;
    "#".repeat(n)
}
