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

use crate::asr::EndpointRules;

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
    /// Optional substring matched against the cpal input device name.
    /// Empty string means "default device".
    pub input_device: String,
    /// Audio chunk size in milliseconds. Clamp: [40, 2000]. Smaller chunks
    /// give the streaming recognizer tighter latency feedback at the cost of
    /// slightly more FFI overhead — 120 ms is a good middle ground.
    pub chunk_ms: u32,
    /// Hard ceiling on a single recording session. Clamp: [10, 3600].
    pub max_recording_seconds: u32,
    /// Auto-stop after this many seconds without new transcript text.
    /// 0 disables the timer. Clamp: [0, 3600].
    pub silence_auto_stop_seconds: u32,
    /// Endpoint rule 1: seconds of trailing silence required to fire an
    /// endpoint when nothing has been decoded yet. Clamp: [0.1, 60.0].
    pub endpoint_rule1_silence_seconds: f32,
    /// Endpoint rule 2: seconds of trailing silence required after non-blank
    /// tokens have been decoded. The primary "user paused" trigger.
    /// Clamp: [0.1, 60.0].
    pub endpoint_rule2_silence_seconds: f32,
    /// Endpoint rule 3: hard ceiling on a single utterance in seconds.
    /// Clamp: [1.0, 600.0].
    pub endpoint_rule3_max_utterance_seconds: f32,
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
            input_device: String::new(),
            chunk_ms: 120,
            max_recording_seconds: 300,
            silence_auto_stop_seconds: 60,
            endpoint_rule1_silence_seconds: 2.4,
            endpoint_rule2_silence_seconds: 1.0,
            endpoint_rule3_max_utterance_seconds: 20.0,
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
    /// Sanitizes the result before returning. Emits a `tracing::warn!` line for
    /// each stale field name found in the file so a user upgrading from an
    /// older scribed knows their previously-tuned values are no longer applied.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let cfg = if path.exists() {
            let bytes = fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.display().to_string(),
                source,
            })?;
            warn_on_stale_keys(&bytes, path);
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

    /// Clamp numeric fields to safe ranges. Out-of-range values are silently
    /// corrected; we don't fail-closed because the user's typo on a single
    /// field shouldn't lock them out of dictation.
    pub fn sanitize(mut self) -> Self {
        self.chunk_ms = self.chunk_ms.clamp(40, 2000);
        self.max_recording_seconds = self.max_recording_seconds.clamp(10, 3600);
        self.silence_auto_stop_seconds = self.silence_auto_stop_seconds.min(3600);
        self.endpoint_rule1_silence_seconds =
            clamp_f32(self.endpoint_rule1_silence_seconds, 0.1, 60.0);
        self.endpoint_rule2_silence_seconds =
            clamp_f32(self.endpoint_rule2_silence_seconds, 0.1, 60.0);
        self.endpoint_rule3_max_utterance_seconds =
            clamp_f32(self.endpoint_rule3_max_utterance_seconds, 5.0, 600.0);
        self
    }

    /// Convenience: the chunk size in samples.
    pub fn chunk_samples(&self, sample_rate_hz: u32) -> usize {
        (self.chunk_ms as f32 / 1000.0 * sample_rate_hz as f32) as usize
    }

    /// Materialize the endpoint-rule values for the sherpa-onnx recognizer.
    pub fn endpoint_rules(&self) -> EndpointRules {
        EndpointRules {
            rule1_min_trailing_silence: self.endpoint_rule1_silence_seconds,
            rule2_min_trailing_silence: self.endpoint_rule2_silence_seconds,
            rule3_max_utterance_seconds: self.endpoint_rule3_max_utterance_seconds,
        }
    }
}

/// Fields that previous scribed releases honored but the streaming pipeline no
/// longer reads. We don't error on them (serde silently ignores unknown keys)
/// but we warn so an upgrading user knows their tuning has been dropped.
const STALE_SCRIBED_KEYS: &[(&str, &str)] = &[
    (
        "silence_threshold_dbfs",
        "energy-based silence gate removed; endpoint detection now lives inside sherpa-onnx (see endpoint_rule1/2/3_*)",
    ),
    (
        "silence_reset_seconds",
        "replaced by endpoint_rule2_silence_seconds",
    ),
    (
        "context_seconds",
        "rolling buffer removed; the streaming recognizer holds its own context",
    ),
    (
        "model",
        "model selection is now driven by the cached bundle directory; this field no longer has any effect",
    ),
];

fn warn_on_stale_keys(raw_toml: &str, path: &Path) {
    let Ok(table) = raw_toml.parse::<toml::Table>() else {
        return; // a real parse error will surface from from_str below
    };
    let Some(scribed) = table.get("scribed").and_then(|v| v.as_table()) else {
        return;
    };
    for (key, why) in STALE_SCRIBED_KEYS {
        if scribed.contains_key(*key) {
            tracing::warn!(
                config = %path.display(),
                key = %key,
                hint = %why,
                "config key is obsolete and ignored — remove it from your config to silence this warning"
            );
        }
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
    fn default_values_are_documented() {
        let c = Config::default();
        assert_eq!(c.hotkey, "ctrl+shift+space");
        assert_eq!(c.mode, TriggerMode::Toggle);
        assert_eq!(c.chunk_ms, 120);
        assert_eq!(c.endpoint_rule1_silence_seconds, 2.4);
        assert_eq!(c.endpoint_rule2_silence_seconds, 1.0);
        assert_eq!(c.endpoint_rule3_max_utterance_seconds, 20.0);
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
            endpoint_rule1_silence_seconds: -1.0,
            endpoint_rule2_silence_seconds: 9999.0,
            endpoint_rule3_max_utterance_seconds: 0.0,
            max_recording_seconds: 0,
            ..Config::default()
        }
        .sanitize();
        assert_eq!(c.chunk_ms, 40);
        assert_eq!(c.endpoint_rule1_silence_seconds, 0.1);
        assert_eq!(c.endpoint_rule2_silence_seconds, 60.0);
        assert_eq!(c.endpoint_rule3_max_utterance_seconds, 5.0);
        assert_eq!(c.max_recording_seconds, 10);
    }

    #[test]
    fn sanitize_handles_nan() {
        let c = Config {
            endpoint_rule1_silence_seconds: f32::NAN,
            ..Config::default()
        }
        .sanitize();
        assert_eq!(c.endpoint_rule1_silence_seconds, 0.1);
    }

    #[test]
    fn sanitize_handles_nan_on_every_endpoint_rule() {
        let c = Config {
            endpoint_rule1_silence_seconds: f32::NAN,
            endpoint_rule2_silence_seconds: f32::NAN,
            endpoint_rule3_max_utterance_seconds: f32::NAN,
            ..Config::default()
        }
        .sanitize();
        assert_eq!(c.endpoint_rule1_silence_seconds, 0.1, "rule1 floor");
        assert_eq!(c.endpoint_rule2_silence_seconds, 0.1, "rule2 floor");
        assert_eq!(c.endpoint_rule3_max_utterance_seconds, 5.0, "rule3 floor");
    }

    #[test]
    fn load_warns_on_stale_keys_but_still_succeeds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy.toml");
        fs::write(
            &path,
            r#"
[scribed]
hotkey = "ctrl+shift+space"
silence_threshold_dbfs = -45.0
silence_reset_seconds = 1.5
context_seconds = 30.0
model = "nvidia/parakeet-tdt-0.6b-v2"
"#,
        )
        .unwrap();
        // No assertion on the warn output (it goes through tracing); the
        // contract here is "load succeeds, stale keys are ignored, defaults apply".
        let c = Config::load(&path).unwrap();
        assert_eq!(c.hotkey, "ctrl+shift+space");
        assert_eq!(c.endpoint_rule3_max_utterance_seconds, 20.0);
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
    fn chunk_samples_uses_chunk_ms() {
        let c = Config::default();
        // 120 ms * 16 kHz = 1920 samples.
        assert_eq!(c.chunk_samples(16_000), 1_920);
    }

    #[test]
    fn endpoint_rules_round_trip_to_engine_struct() {
        let c = Config {
            endpoint_rule1_silence_seconds: 3.0,
            endpoint_rule2_silence_seconds: 0.7,
            endpoint_rule3_max_utterance_seconds: 30.0,
            ..Config::default()
        };
        let rules = c.endpoint_rules();
        assert_eq!(rules.rule1_min_trailing_silence, 3.0);
        assert_eq!(rules.rule2_min_trailing_silence, 0.7);
        assert_eq!(rules.rule3_max_utterance_seconds, 30.0);
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
