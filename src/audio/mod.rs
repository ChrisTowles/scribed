//! Audio bounded context — capture, DSP, device resolution.

use thiserror::Error;

pub mod capture;
pub mod device;
pub mod dsp;

pub use capture::{AudioChunk, CaptureStream};
pub use device::{list_names as list_device_names, resolve as resolve_device, ResolvedInput};
pub use dsp::rms_dbfs;

/// Canonical sample rate. Every streaming ASR model scribed targets is
/// trained on 16 kHz mono.
pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const SAMPLE_RATE_HZ_I32: i32 = SAMPLE_RATE_HZ as i32;

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
