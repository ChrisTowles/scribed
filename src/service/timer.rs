//! Recording timers. Pure value-object: no clocks, no tokio. The
//! orchestration layer polls [`RecordingTimer::tick`] from a tokio interval
//! and acts on the returned [`TimerEvent`].
//!
//! Mirrors `claude_stt/daemon_service.py:173-199`.

use std::time::{Duration, Instant};

/// Reasons the orchestrator may want to take action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    /// Maximum recording time reached. Stop now.
    MaxRecordingReached,
    /// User has been silent (no new transcript text) past the idle threshold.
    /// Stop now.
    IdleTimeout,
    /// Approaching the max recording time. Play a warning sound. Fires once.
    MaxRecordingWarning,
}

#[derive(Debug, Clone)]
pub struct TimerConfig {
    /// Maximum total session length (seconds). 0 disables.
    pub max_recording_seconds: u32,
    /// Auto-stop if no new transcript text for this long (seconds). 0 disables.
    pub idle_seconds: u32,
    /// Fire the warning this far before max_recording_seconds (seconds).
    pub warning_lead_seconds: u32,
}

impl TimerConfig {
    pub fn from_config(c: &crate::config::Config) -> Self {
        Self {
            max_recording_seconds: c.max_recording_seconds,
            idle_seconds: c.silence_auto_stop_seconds,
            warning_lead_seconds: 30,
        }
    }
}

#[derive(Debug)]
pub struct RecordingTimer {
    config: TimerConfig,
    started_at: Instant,
    last_text_at: Instant,
    warning_fired: bool,
}

impl RecordingTimer {
    pub fn start(config: TimerConfig) -> Self {
        let now = Instant::now();
        Self {
            config,
            started_at: now,
            last_text_at: now,
            warning_fired: false,
        }
    }

    pub fn start_at(config: TimerConfig, now: Instant) -> Self {
        Self {
            config,
            started_at: now,
            last_text_at: now,
            warning_fired: false,
        }
    }

    pub fn mark_text(&mut self) {
        self.last_text_at = Instant::now();
    }

    pub fn mark_text_at(&mut self, now: Instant) {
        self.last_text_at = now;
    }

    pub fn elapsed_since_start(&self) -> Duration {
        Instant::now().saturating_duration_since(self.started_at)
    }

    pub fn elapsed_since_text(&self) -> Duration {
        Instant::now().saturating_duration_since(self.last_text_at)
    }

    /// Examine the timer state. Returns a `TimerEvent` if something needs to
    /// happen at this moment, else `None`. Idempotent for warnings: only fires
    /// once per session.
    pub fn tick(&mut self) -> Option<TimerEvent> {
        self.tick_at(Instant::now())
    }

    pub fn tick_at(&mut self, now: Instant) -> Option<TimerEvent> {
        let elapsed = now.saturating_duration_since(self.started_at);
        let max = self.config.max_recording_seconds;
        if max > 0 && elapsed >= Duration::from_secs(max as u64) {
            return Some(TimerEvent::MaxRecordingReached);
        }
        if max > 0 && !self.warning_fired {
            let warning_at =
                Duration::from_secs(max.saturating_sub(self.config.warning_lead_seconds) as u64);
            if elapsed >= warning_at {
                self.warning_fired = true;
                return Some(TimerEvent::MaxRecordingWarning);
            }
        }
        let idle = self.config.idle_seconds;
        if idle > 0 {
            let idle_elapsed = now.saturating_duration_since(self.last_text_at);
            if idle_elapsed >= Duration::from_secs(idle as u64) {
                return Some(TimerEvent::IdleTimeout);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TimerConfig {
        TimerConfig {
            max_recording_seconds: 300,
            idle_seconds: 60,
            warning_lead_seconds: 30,
        }
    }

    #[test]
    fn nothing_fires_immediately() {
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(config(), t0);
        assert_eq!(timer.tick_at(t0), None);
    }

    #[test]
    fn warning_fires_at_t_minus_warning_lead() {
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(config(), t0);
        // Simulate steady speech so idle doesn't shadow the warning.
        timer.mark_text_at(t0 + Duration::from_secs(269));
        // 270 s after start: 30 s before max
        let warning_time = t0 + Duration::from_secs(270);
        assert_eq!(
            timer.tick_at(warning_time),
            Some(TimerEvent::MaxRecordingWarning)
        );
        // Second tick shouldn't refire the warning (idle is also fresh).
        timer.mark_text_at(warning_time);
        let later = warning_time + Duration::from_secs(1);
        assert_eq!(timer.tick_at(later), None);
    }

    #[test]
    fn max_recording_fires_at_max() {
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(config(), t0);
        // Keep idle fresh and consume the warning so we test the max event.
        timer.mark_text_at(t0 + Duration::from_secs(270));
        let _ = timer.tick_at(t0 + Duration::from_secs(270));
        timer.mark_text_at(t0 + Duration::from_secs(299));
        let max_time = t0 + Duration::from_secs(300);
        assert_eq!(
            timer.tick_at(max_time),
            Some(TimerEvent::MaxRecordingReached)
        );
    }

    #[test]
    fn idle_timeout_fires_after_silence() {
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(config(), t0);
        let later = t0 + Duration::from_secs(61);
        assert_eq!(timer.tick_at(later), Some(TimerEvent::IdleTimeout));
    }

    #[test]
    fn mark_text_resets_idle_window() {
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(config(), t0);
        timer.mark_text_at(t0 + Duration::from_secs(55));
        // At t0+90: 55 + 60 = 115, so still under threshold from last text
        let later = t0 + Duration::from_secs(90);
        assert_eq!(timer.tick_at(later), None);
        // At t0+120: it's now 65 since text -> trigger
        let way_later = t0 + Duration::from_secs(120);
        assert_eq!(timer.tick_at(way_later), Some(TimerEvent::IdleTimeout));
    }

    #[test]
    fn idle_zero_disables_idle_check() {
        let cfg = TimerConfig {
            idle_seconds: 0,
            ..config()
        };
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(cfg, t0);
        let later = t0 + Duration::from_secs(10_000);
        // Max recording will fire at 300; we test the idle path doesn't even
        // get a chance by limiting elapsed time below max:
        let t = t0 + Duration::from_secs(200);
        assert_eq!(timer.tick_at(t), None);
        let _ = later; // appease unused
    }

    #[test]
    fn warning_at_zero_max_does_not_fire() {
        let cfg = TimerConfig {
            max_recording_seconds: 0,
            idle_seconds: 0,
            warning_lead_seconds: 30,
        };
        let t0 = Instant::now();
        let mut timer = RecordingTimer::start_at(cfg, t0);
        assert_eq!(timer.tick_at(t0 + Duration::from_secs(10_000)), None);
    }
}
