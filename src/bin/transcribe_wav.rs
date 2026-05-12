//! End-to-end smoke test: load Parakeet via sherpa-rs and transcribe a WAV.
//!
//! Usage:
//!
//! ```text
//! transcribe_wav <wav-path> [--model-dir DIR]
//! ```
//!
//! If no model directory is supplied, defaults to the cached
//! `parakeet-tdt-0.6b-v2` bundle under `~/.cache/scribed/`.

#[cfg(not(feature = "asr"))]
fn main() {
    eprintln!("transcribe_wav requires --features asr at build time");
    std::process::exit(2);
}

#[cfg(feature = "asr")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use scribed::asr::driver::Transcriber;
    use scribed::asr::sherpa::{ModelBundle, SherpaTranscriber};
    use scribed::paths::Paths;
    use std::path::PathBuf;
    use std::time::Instant;

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: transcribe_wav <wav-path> [--model-dir DIR]");
        std::process::exit(2);
    }
    let wav_path = PathBuf::from(&args[1]);
    let model_dir = if let Some(i) = args.iter().position(|a| a == "--model-dir") {
        PathBuf::from(args.get(i + 1).ok_or("missing model-dir value")?)
    } else {
        let paths = Paths::from_env();
        paths
            .cache_dir
            .join("sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8")
    };

    println!("Model dir: {}", model_dir.display());
    println!("WAV: {}", wav_path.display());

    let bundle = ModelBundle::from_dir(&model_dir);
    println!("Loading model (provider=cpu)...");
    let t = Instant::now();
    let mut transcriber = SherpaTranscriber::load(&bundle, "cpu", 4)?;
    println!("  loaded in {:?}", t.elapsed());

    let samples = read_wav_to_16khz_mono_f32(&wav_path)?;
    println!(
        "Audio: {} samples ({:.2} s at 16 kHz)",
        samples.len(),
        samples.len() as f32 / 16_000.0
    );

    let t = Instant::now();
    let text = transcriber.transcribe(&samples)?;
    let elapsed = t.elapsed();
    let audio_s = samples.len() as f32 / 16_000.0;
    let rtf = elapsed.as_secs_f32() / audio_s;
    println!();
    println!("Transcript: {text}");
    println!("Inference: {:?} ({:.3}x realtime)", elapsed, 1.0 / rtf);
    Ok(())
}

#[cfg(feature = "asr")]
fn read_wav_to_16khz_mono_f32(
    path: &std::path::Path,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let sample_rate = spec.sample_rate;
    let raw_mono: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1u64 << (spec.bits_per_sample - 1)) as f32;
            let samples: Vec<i32> = reader.samples::<i32>().collect::<Result<_, _>>()?;
            chan_avg(&samples, channels)
                .into_iter()
                .map(|s| s as f32 / max)
                .collect()
        }
        hound::SampleFormat::Float => {
            let samples: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>()?;
            chan_avg_f32(&samples, channels)
        }
    };
    if sample_rate == 16_000 {
        Ok(raw_mono)
    } else {
        Ok(resample(&raw_mono, sample_rate, 16_000))
    }
}

#[cfg(feature = "asr")]
fn chan_avg(samples: &[i32], channels: usize) -> Vec<i32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * channels;
        let sum: i64 = samples[start..start + channels]
            .iter()
            .map(|&x| x as i64)
            .sum();
        out.push((sum / channels as i64) as i32);
    }
    out
}

#[cfg(feature = "asr")]
fn chan_avg_f32(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * channels;
        let sum: f32 = samples[start..start + channels].iter().sum();
        out.push(sum / channels as f32);
    }
    out
}

#[cfg(feature = "asr")]
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    (0..out_len)
        .map(|i| {
            let src = ((i as f64) / ratio).round() as usize;
            samples[src.min(samples.len() - 1)]
        })
        .collect()
}
