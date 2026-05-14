//! The orchestration aggregate. Wires the Audio, ASR, Input, Output, and
//! Notification contexts together. This is the only module allowed to depend
//! on more than one bounded context — everything else stays in its lane.
//!
//! The runtime wiring (audio thread, inference thread, hotkey listener,
//! output sink) lives behind feature gates in the daemon's main loop; this
//! module ships the engine-agnostic state machine pieces that other modules
//! can compose.

pub mod timer;

pub use timer::{RecordingTimer, TimerConfig, TimerEvent};

#[cfg(feature = "asr")]
pub mod runtime;

#[cfg(feature = "asr")]
pub use runtime::Runtime;
