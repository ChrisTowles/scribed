//! Configuration — the [Configuration] bounded context.
//!
//! Loaded from `~/.config/scribed/config.toml`. Validation clamps numeric
//! fields to safe ranges; out-of-range values are warned about but never
//! rejected, matching the Python project's tolerant behavior.
//!
//! [Configuration]: ../DOMAIN.md

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How the hotkey is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum TriggerMode {
    /// Press to start, press again to stop.
    #[default]
    Toggle,
    /// Hold to record, release to stop.
    PushToTalk,
}

/// How transcripts reach the focused application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum OutputMode {
    /// Auto-detect: ydotool on Wayland, enigo on X11/macOS, clipboard fallback.
    #[default]
    Auto,
    /// Force live keystroke injection.
    Injection,
    /// Force clipboard-only output.
    Clipboard,
}

/// The full configuration tree. All numeric fields are clamped by [`Config::sanitize`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Config {
    /// Hotkey expression, parsed by the input context.
    pub hotkey: String,
    /// Trigger mode.
    pub mode: TriggerMode,
    /// ASR model identifier (engine-specific).
    pub model: String,
    /// Optional substring matched against the cpal input device name.
    /// Empty string means "default device".
    pub input_device: String,
    /// Audio chunk size in milliseconds. Clamp: [80, 2000].
    pub chunk_ms: u32,
    /// Rolling buffer length in seconds. Clamp: [1.0, 60.0].
    pub context_seconds: f32,
    /// Silence gate threshold in dBFS. Clamp: [-120.0, 0.0].
    pub silence_threshold_dbfs: f32,
    /// After this much consecutive silence, freeze the current pass into a
    /// committed segment and clear the rolling buffer. Clamp: [0.1, 10.0].
    pub silence_reset_seconds: f32,
    /// Hard ceiling on a single recording session. Clamp: [10, 3600].
    pub max_recording_seconds: u32,
    /// Auto-stop after this many seconds without new transcript text.
    /// 0 disables the timer. Clamp: [0, 3600].
    pub silence_auto_stop_seconds: u32,
    /// Output strategy.
    pub output_mode: OutputMode,
    /// Play start/stop/warning sounds.
    pub sound_effects: bool,
    /// Use Shift+Enter for intermediate newlines (soft break in many editors)
    /// and a real Enter only at the end.
    pub soft_newlines: bool,
    /// App names (substring match) for which the hotkey is ignored.
    pub excluded_apps: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "ctrl+shift+space".to_string(),
            mode: TriggerMode::Toggle,
            model: "nvidia/parakeet-tdt-0.6b-v2".to_string(),
            input_device: String::new(),
            chunk_ms: 320,
            context_seconds: 30.0,
            silence_threshold_dbfs: -45.0,
            silence_reset_seconds: 1.5,
            max_recording_seconds: 300,
            silence_auto_stop_seconds: 60,
            output_mode: OutputMode::Auto,
            sound_effects: true,
            soft_newlines: true,
            excluded_apps: Vec::new(),
        }
    }
}

/// Wrapper struct so the TOML file looks like:
///
/// ```toml
/// [scribed]
/// hotkey = "ctrl+shift+space"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    scribed: Config,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file at {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in config file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("write error at {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl Config {
    /// Load from a TOML file, or return defaults if the file does not exist.
    /// Sanitizes the result before returning.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let cfg = if path.exists() {
            let bytes = fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.display().to_string(),
                source,
            })?;
            let file: ConfigFile = toml::from_str(&bytes)?;
            file.scribed
        } else {
            Self::default()
        };
        Ok(cfg.sanitize())
    }

    /// Serialize this config as a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        let file = ConfigFile {
            scribed: self.clone(),
        };
        Ok(toml::to_string_pretty(&file)?)
    }

    /// Write this config to the given path, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let toml = self.to_toml()?;
        fs::write(path, toml).map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })?;
        Ok(())
    }

    /// Clamp numeric fields to safe ranges, mirroring claude-stt's `validate()`.
    /// Out-of-range values are silently corrected; we don't fail-closed because
    /// the user's typo on a single field shouldn't lock them out of dictation.
    pub fn sanitize(mut self) -> Self {
        self.chunk_ms = self.chunk_ms.clamp(80, 2000);
        self.context_seconds = clamp_f32(self.context_seconds, 1.0, 60.0);
        self.silence_threshold_dbfs = clamp_f32(self.silence_threshold_dbfs, -120.0, 0.0);
        self.silence_reset_seconds = clamp_f32(self.silence_reset_seconds, 0.1, 10.0);
        self.max_recording_seconds = self.max_recording_seconds.clamp(10, 3600);
        self.silence_auto_stop_seconds = self.silence_auto_stop_seconds.min(3600);
        self
    }

    /// Convenience: the rolling-buffer capacity in samples.
    pub fn rolling_buffer_samples(&self, sample_rate_hz: u32) -> usize {
        (self.context_seconds * sample_rate_hz as f32) as usize
    }

    /// Convenience: the chunk size in samples.
    pub fn chunk_samples(&self, sample_rate_hz: u32) -> usize {
        (self.chunk_ms as f32 / 1000.0 * sample_rate_hz as f32) as usize
    }

    /// Convenience: number of consecutive silent chunks that trigger a reset.
    pub fn silence_reset_chunks(&self) -> u32 {
        ((self.silence_reset_seconds * 1000.0) / self.chunk_ms as f32).ceil() as u32
    }
}

fn clamp_f32(value: f32, min: f32, max: f32) -> f32 {
    if value.is_nan() {
        return min;
    }
    value.max(min).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_match_python_claude_stt() {
        let c = Config::default();
        assert_eq!(c.hotkey, "ctrl+shift+space");
        assert_eq!(c.mode, TriggerMode::Toggle);
        assert_eq!(c.chunk_ms, 320);
        assert_eq!(c.context_seconds, 30.0);
        assert_eq!(c.silence_threshold_dbfs, -45.0);
        assert_eq!(c.silence_reset_seconds, 1.5);
        assert_eq!(c.max_recording_seconds, 300);
        assert_eq!(c.silence_auto_stop_seconds, 60);
        assert!(c.sound_effects);
        assert!(c.soft_newlines);
        assert!(c.excluded_apps.is_empty());
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let c = Config {
            chunk_ms: 10,
            context_seconds: 500.0,
            silence_threshold_dbfs: 10.0,
            silence_reset_seconds: -1.0,
            max_recording_seconds: 0,
            ..Config::default()
        }
        .sanitize();
        assert_eq!(c.chunk_ms, 80);
        assert_eq!(c.context_seconds, 60.0);
        assert_eq!(c.silence_threshold_dbfs, 0.0);
        assert_eq!(c.silence_reset_seconds, 0.1);
        assert_eq!(c.max_recording_seconds, 10);
    }

    #[test]
    fn sanitize_handles_nan() {
        let c = Config {
            context_seconds: f32::NAN,
            ..Config::default()
        }
        .sanitize();
        assert_eq!(c.context_seconds, 1.0);
    }

    #[test]
    fn toml_round_trip_preserves_defaults() {
        let original = Config::default();
        let toml = original.to_toml().unwrap();
        let parsed: ConfigFile = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.scribed, original);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let c = Config::load(&path).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn save_and_load_round_trips_full_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = Config {
            hotkey: "alt+r".into(),
            excluded_apps: vec!["keepassxc".into(), "1password".into()],
            ..Config::default()
        };
        original.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn rolling_buffer_samples_uses_context_seconds() {
        let c = Config::default();
        assert_eq!(c.rolling_buffer_samples(16_000), 480_000);
    }

    #[test]
    fn chunk_samples_uses_chunk_ms() {
        let c = Config::default();
        assert_eq!(c.chunk_samples(16_000), 5_120);
    }

    #[test]
    fn silence_reset_chunks_rounds_up() {
        // default: 1.5s / 320ms = 4.6875 -> ceil = 5
        assert_eq!(Config::default().silence_reset_chunks(), 5);
    }

    #[test]
    fn rejects_garbage_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(&path, "this is not [valid toml").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
}
