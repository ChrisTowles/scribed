//! Notification bounded context — sound effects + desktop notifications.

use std::path::{Path, PathBuf};

pub mod sound;

pub mod desktop;

pub use sound::SoundPlayer;

/// Semantic audio cue. Maps to an .ogg in the assets directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    /// Recording started.
    Start,
    /// Recording stopped.
    Stop,
    /// 30 s left in the max-recording timer.
    Warning,
    /// Model loaded, daemon ready for first hotkey press.
    Ready,
    /// Error during recording or inference.
    Error,
    /// Daemon shutting down.
    Shutdown,
}

impl SoundEvent {
    /// The asset file name (under the assets directory).
    pub fn asset(&self) -> &'static str {
        match self {
            SoundEvent::Start => "start.ogg",
            SoundEvent::Stop => "stop.ogg",
            SoundEvent::Warning => "warning.ogg",
            SoundEvent::Ready => "ready.ogg",
            SoundEvent::Error => "error.ogg",
            SoundEvent::Shutdown => "shutdown.ogg",
        }
    }

    /// Resolve the asset path under `assets_dir`.
    pub fn asset_path(&self, assets_dir: &Path) -> PathBuf {
        assets_dir.join(self.asset())
    }
}

/// A notifier that plays sound events and posts desktop notifications.
pub trait Notifier: Send + Sync {
    fn play(&self, event: SoundEvent);
    fn notify(&self, title: &str, body: &str);
}

#[derive(Debug, Default)]
pub struct NullNotifier;

impl Notifier for NullNotifier {
    fn play(&self, _event: SoundEvent) {}
    fn notify(&self, _title: &str, _body: &str) {}
}

/// The shipping notifier — sound via rodio + desktop notifications via
/// notify-rust on Linux. Constructed once at daemon startup.
pub struct PlatformNotifier {
    player: Option<SoundPlayer>,
    sound_enabled: bool,
}

impl PlatformNotifier {
    pub fn new(assets_dir: PathBuf, sound_enabled: bool) -> Self {
        let player = if sound_enabled {
            match SoundPlayer::new(assets_dir) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(?e, "sound player init failed; sounds disabled");
                    None
                }
            }
        } else {
            None
        };
        Self {
            player,
            sound_enabled,
        }
    }
}

impl Notifier for PlatformNotifier {
    fn play(&self, event: SoundEvent) {
        if !self.sound_enabled {
            return;
        }
        if let Some(p) = &self.player {
            p.play(event);
        }
    }

    fn notify(&self, title: &str, body: &str) {
        desktop::post(title, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_name_per_event() {
        assert_eq!(SoundEvent::Start.asset(), "start.ogg");
        assert_eq!(SoundEvent::Stop.asset(), "stop.ogg");
        assert_eq!(SoundEvent::Warning.asset(), "warning.ogg");
        assert_eq!(SoundEvent::Ready.asset(), "ready.ogg");
        assert_eq!(SoundEvent::Error.asset(), "error.ogg");
        assert_eq!(SoundEvent::Shutdown.asset(), "shutdown.ogg");
    }

    #[test]
    fn asset_path_joins_under_assets_dir() {
        let p = SoundEvent::Start.asset_path(Path::new("/var/lib/scribed"));
        assert_eq!(p, Path::new("/var/lib/scribed/start.ogg"));
    }

    #[test]
    fn null_notifier_does_not_panic() {
        let n = NullNotifier;
        n.play(SoundEvent::Start);
        n.notify("hi", "there");
    }

    #[test]
    fn platform_notifier_with_sound_disabled_is_a_noop() {
        let n = PlatformNotifier::new(PathBuf::from("/tmp/nowhere"), false);
        n.play(SoundEvent::Start);
    }
}
