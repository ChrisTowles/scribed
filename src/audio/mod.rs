//! Audio bounded context — capture, DSP, rolling buffer.
//!
//! Phase 2 lands the cpal stream + ring buffer; Phase 1 ships the error type
//! and the DSP primitives (`rms_dbfs`) used by tests in other modules.

use thiserror::Error;

pub mod capture;
pub mod device;
pub mod dsp;
pub mod rolling;

pub use capture::{AudioChunk, CaptureStream};
pub use device::{list_names as list_device_names, resolve as resolve_device, ResolvedInput};
pub use dsp::rms_dbfs;
pub use rolling::RollingBuffer;

/// Canonical sample rate. Parakeet's encoder is trained on 16 kHz mono.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no input device available")]
    NoInputDevice,
    #[error("input device not found: substring '{0}' matched no device")]
    DeviceNotFound(String),
    #[error("cpal: {0}")]
    Cpal(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
