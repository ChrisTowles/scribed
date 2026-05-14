//! Lazy model download. Streaming Zipformer English transducer is ~100 MB;
//! we fetch it once and cache under [`crate::paths::Paths::cache_dir`].
//!
//! Sherpa-onnx publishes prebuilt streaming models as `.tar.bz2` archives on
//! GitHub Releases; we point at one well-known English bundle and let
//! advanced users swap in their own by dropping files into the cache dir.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};

use crate::asr::AsrError;

/// A named model bundle.
#[derive(Debug, Clone)]
pub struct ModelArchive {
    pub name: &'static str,
    pub url: &'static str,
    pub extracted_dir: &'static str,
}

/// Streaming Zipformer English transducer (k2-fsa, LibriSpeech, RNN-T, 16 kHz).
pub const STREAMING_ZIPFORMER_EN: ModelArchive = ModelArchive {
    name: "streaming-zipformer-en-2023-06-26",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2",
    extracted_dir: "sherpa-onnx-streaming-zipformer-en-2023-06-26",
};

/// Ensure the named model exists locally. Returns the directory containing the
/// extracted files. Downloads + extracts on first call.
pub fn ensure(archive: &ModelArchive, cache_dir: &Path) -> Result<PathBuf, AsrError> {
    let target = cache_dir.join(archive.extracted_dir);
    if target.exists() {
        tracing::info!(?target, "model cache hit");
        return Ok(target);
    }
    fs::create_dir_all(cache_dir).map_err(|e| AsrError::Load(e.to_string()))?;
    let archive_path = cache_dir.join(format!("{}.tar.bz2", archive.name));
    download(archive.url, &archive_path)?;
    extract(&archive_path, cache_dir)?;
    let _ = fs::remove_file(&archive_path);
    Ok(target)
}

fn download(url: &str, dest: &Path) -> Result<(), AsrError> {
    tracing::info!(%url, "downloading model");
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .build()
        .map_err(|e| AsrError::Load(e.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| AsrError::Load(e.to_string()))?;
    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    let mut out = fs::File::create(dest).map_err(|e| AsrError::Load(e.to_string()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| AsrError::Load(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| AsrError::Load(e.to_string()))?;
        pb.inc(n as u64);
    }
    pb.finish_and_clear();
    Ok(())
}

fn extract(archive: &Path, into: &Path) -> Result<(), AsrError> {
    // We shell out to `tar` rather than pulling in tar+bzip2 crates; tar is
    // universally available on Linux + macOS and saves us a hundred KB of
    // dependencies.
    let status = std::process::Command::new("tar")
        .arg("-xjf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|e| AsrError::Load(format!("failed to spawn tar: {e}")))?;
    if !status.success() {
        return Err(AsrError::Load(format!(
            "tar extraction failed (exit {status})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ensure_returns_cached_path_without_network() {
        let dir = tempdir().unwrap();
        let prebuilt = dir.path().join(STREAMING_ZIPFORMER_EN.extracted_dir);
        fs::create_dir_all(&prebuilt).unwrap();
        let result = ensure(&STREAMING_ZIPFORMER_EN, dir.path()).unwrap();
        assert_eq!(result, prebuilt);
    }
}
