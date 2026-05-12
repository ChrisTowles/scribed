//! Top-level error type. Per-context modules define their own `thiserror` enums
//! and convert into [`Error`] via `From`.

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("lifecycle: {0}")]
    Lifecycle(#[from] crate::lifecycle::LifecycleError),

    #[error("audio: {0}")]
    Audio(#[from] crate::audio::AudioError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
