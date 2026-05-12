//! The press-tracking aggregator. Independent of the underlying key source
//! (evdev on Linux, rdev on macOS) — it consumes [`KeyEvent`]s and emits
//! [`RecordingIntent`]s based on the configured trigger mode.
//!
//! Testable in isolation: feed synthetic events, assert intents.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::config::TriggerMode;
use crate::input::{KeyChord, RecordingIntent};

/// A normalized key name. Lowercase, no angle brackets — matches what
/// [`KeyChord::parse`] produces. Source-specific code maps platform key codes
/// to this representation.
pub type KeyName = String;

/// What a key just did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyName,
    pub state: KeyState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Press,
    Release,
}

/// Maintains pressed-key set; fires intents when the chord is hit.
pub struct Aggregator {
    chord: KeyChord,
    mode: TriggerMode,
    pressed: BTreeSet<KeyName>,
    /// In toggle mode, true when we've already started this press cycle —
    /// prevents firing repeatedly while keys are held.
    chord_active: bool,
    /// In toggle mode, current recording state. In push-to-talk mode this is
    /// always derived from `chord_active`.
    recording: bool,
    last_toggle: Option<Instant>,
    debounce: Duration,
}

impl Aggregator {
    pub fn new(chord: KeyChord, mode: TriggerMode) -> Self {
        Self {
            chord,
            mode,
            pressed: BTreeSet::new(),
            chord_active: false,
            recording: false,
            last_toggle: None,
            debounce: Duration::from_secs(1),
        }
    }

    pub fn with_debounce(mut self, d: Duration) -> Self {
        self.debounce = d;
        self
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Inject an event. Returns `Some(intent)` if the orchestrator should act.
    pub fn observe(&mut self, ev: KeyEvent) -> Option<RecordingIntent> {
        self.observe_at(ev, Instant::now())
    }

    pub fn observe_at(&mut self, ev: KeyEvent, now: Instant) -> Option<RecordingIntent> {
        match ev.state {
            KeyState::Press => {
                let newly = self.pressed.insert(ev.key.clone());
                if !newly {
                    return None;
                }
                if self.is_chord_matched() {
                    if self.chord_active {
                        return None;
                    }
                    self.chord_active = true;
                    return self.on_chord_press(now);
                }
                None
            }
            KeyState::Release => {
                self.pressed.remove(&ev.key);
                let was_active = self.chord_active;
                let still_matched = self.is_chord_matched();
                if was_active && !still_matched {
                    self.chord_active = false;
                    return self.on_chord_release();
                }
                None
            }
        }
    }

    fn is_chord_matched(&self) -> bool {
        if !self.pressed.contains(&self.chord.trigger) {
            return false;
        }
        self.chord
            .modifiers
            .iter()
            .all(|m| self.pressed.contains(m))
    }

    fn on_chord_press(&mut self, now: Instant) -> Option<RecordingIntent> {
        match self.mode {
            TriggerMode::Toggle => {
                if let Some(last) = self.last_toggle {
                    if now.duration_since(last) < self.debounce {
                        return None;
                    }
                }
                self.last_toggle = Some(now);
                self.recording = !self.recording;
                Some(if self.recording {
                    RecordingIntent::Start
                } else {
                    RecordingIntent::Stop
                })
            }
            TriggerMode::PushToTalk => {
                if self.recording {
                    None
                } else {
                    self.recording = true;
                    Some(RecordingIntent::Start)
                }
            }
        }
    }

    fn on_chord_release(&mut self) -> Option<RecordingIntent> {
        match self.mode {
            TriggerMode::Toggle => None,
            TriggerMode::PushToTalk => {
                if self.recording {
                    self.recording = false;
                    Some(RecordingIntent::Stop)
                } else {
                    None
                }
            }
        }
    }

    /// Force the recording flag (e.g., when the auto-stop timer fires and we
    /// want the aggregator to behave as though the user toggled off).
    pub fn force_recording(&mut self, recording: bool) {
        self.recording = recording;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn press(k: &str) -> KeyEvent {
        KeyEvent {
            key: k.into(),
            state: KeyState::Press,
        }
    }
    fn release(k: &str) -> KeyEvent {
        KeyEvent {
            key: k.into(),
            state: KeyState::Release,
        }
    }
    fn chord(s: &str) -> KeyChord {
        KeyChord::parse(s).unwrap()
    }

    #[test]
    fn toggle_alternates_on_each_full_press() {
        let mut a = Aggregator::new(chord("ctrl+shift+space"), TriggerMode::Toggle)
            .with_debounce(Duration::ZERO);
        // 1st press
        assert_eq!(a.observe(press("ctrl")), None);
        assert_eq!(a.observe(press("shift")), None);
        assert_eq!(a.observe(press("space")), Some(RecordingIntent::Start));
        // Release without firing more
        assert_eq!(a.observe(release("space")), None);
        assert_eq!(a.observe(release("shift")), None);
        assert_eq!(a.observe(release("ctrl")), None);
        // 2nd press
        assert_eq!(a.observe(press("ctrl")), None);
        assert_eq!(a.observe(press("shift")), None);
        assert_eq!(a.observe(press("space")), Some(RecordingIntent::Stop));
    }

    #[test]
    fn toggle_debounce_blocks_rapid_repeats() {
        let mut a = Aggregator::new(chord("ctrl+space"), TriggerMode::Toggle)
            .with_debounce(Duration::from_secs(1));
        let t0 = Instant::now();
        // First press fires
        assert_eq!(a.observe_at(press("ctrl"), t0), None);
        assert_eq!(
            a.observe_at(press("space"), t0),
            Some(RecordingIntent::Start)
        );
        a.observe_at(release("space"), t0);
        a.observe_at(release("ctrl"), t0);
        // Second press at t0 + 100ms is within debounce -> swallowed
        let t1 = t0 + Duration::from_millis(100);
        a.observe_at(press("ctrl"), t1);
        assert_eq!(a.observe_at(press("space"), t1), None);
    }

    #[test]
    fn push_to_talk_fires_start_on_press_and_stop_on_release() {
        let mut a = Aggregator::new(chord("ctrl+space"), TriggerMode::PushToTalk);
        a.observe(press("ctrl"));
        assert_eq!(a.observe(press("space")), Some(RecordingIntent::Start));
        // Held: no further intents while pressed
        assert_eq!(a.observe(press("ctrl")), None); // already pressed; no-op
                                                    // Release any chord key
        assert_eq!(a.observe(release("space")), Some(RecordingIntent::Stop));
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        let mut a = Aggregator::new(chord("ctrl+shift+space"), TriggerMode::Toggle)
            .with_debounce(Duration::ZERO);
        for k in ["a", "b", "c", "enter"] {
            assert_eq!(a.observe(press(k)), None);
            assert_eq!(a.observe(release(k)), None);
        }
    }

    #[test]
    fn partial_chord_does_not_fire() {
        let mut a = Aggregator::new(chord("ctrl+shift+space"), TriggerMode::Toggle)
            .with_debounce(Duration::ZERO);
        a.observe(press("ctrl"));
        a.observe(press("space")); // missing shift
        assert!(!a.is_recording());
    }

    #[test]
    fn holding_the_chord_does_not_fire_repeatedly() {
        let mut a =
            Aggregator::new(chord("ctrl+space"), TriggerMode::Toggle).with_debounce(Duration::ZERO);
        let t = Instant::now();
        a.observe_at(press("ctrl"), t);
        assert_eq!(
            a.observe_at(press("space"), t),
            Some(RecordingIntent::Start)
        );
        // Imagine auto-repeat fires a second press of space
        assert_eq!(a.observe_at(press("space"), t), None);
    }
}
