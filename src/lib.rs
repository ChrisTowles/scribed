//! `scribed` — a streaming dictation daemon.
//!
//! See `DOMAIN.md` at the repo root for the ubiquitous language used here.
//! Modules below correspond 1:1 to bounded contexts:
//!
//! | Module | Bounded context |
//! |---|---|
//! | [`audio`] | Microphone capture, DSP, rolling buffer |
//! | [`asr`] | Speech recognition engines |
//! | [`input`] | Global hotkey listening |
//! | [`output`] | Keyboard injection + window focus |
//! | [`lifecycle`] | Daemon process, PID file, control socket |
//! | [`notification`] | Sound effects + desktop notifications |
//! | [`config`] | TOML configuration |
//! | [`service`] | Orchestration aggregate (the only place contexts cross) |

pub mod asr;
pub mod audio;
pub mod cli;
pub mod config;
pub mod errors;
pub mod input;
pub mod lifecycle;
pub mod notification;
pub mod output;
pub mod paths;
pub mod service;

pub use errors::{Error, Result};
