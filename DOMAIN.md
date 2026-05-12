# Domain Model

`scribed` is structured around Domain-Driven Design vocabulary so the codebase, docs, commit messages, and conversations all use the same words for the same things. When in doubt about a name, consult this file.

## Bounded Contexts

| Context | Responsibility | Key types |
|---|---|---|
| **Audio** | Capture frames from the microphone and shape them into chunks. Compute energy. Maintain the rolling context buffer. | `AudioFrame`, `AudioChunk`, `RollingBuffer`, `SilenceGate`, `InputDevice` |
| **ASR** (Automatic Speech Recognition) | Turn audio chunks into transcripts. Owns model lifecycle. | `AsrEngine` (trait), `SherpaEngine`, `Transcript`, `Segment`, `EngineState` |
| **Input** | Listen for global hotkey presses. Translate raw key events into recording intents. | `Hotkey`, `HotkeyListener`, `KeyChord`, `RecordingIntent`, `TriggerMode` |
| **Output** | Deliver transcripts to the active application. Track and restore window focus. | `KeyboardBackend`, `RetypeState`, `WindowSnapshot`, `OutputSink` |
| **Lifecycle** | Process-level concerns: daemon spawn, PID file, IPC, shutdown signals. | `PidFile`, `ControlSocket`, `DaemonCommand`, `DaemonStatus` |
| **Configuration** | Load, validate, and expose user preferences. | `Config`, `ConfigError`, `OutputMode`, `TriggerMode` |
| **Notification** | Side-channel feedback to the user: sound effects, desktop notifications. | `SoundEvent`, `Notifier` |
| **Orchestration** | The application service that wires every context together. | `Service` (the aggregate root) |

The `Service` aggregate is the only place contexts cross. Every other module talks to its neighbours through traits or message-passing channels.

## Ubiquitous Language

### Lifecycle

| Term | Meaning |
|---|---|
| **Daemon** | The long-running `scribed` process. There is at most one per user. |
| **Foreground run** | The daemon started without `--background`, log-to-stdout, dies with the terminal. Used for development and by `--background` after it forks. |
| **Background spawn** | `scribed start --background` — forks, writes the PID file, exits the parent. |
| **PID file** | `~/.config/scribed/daemon.pid` (JSON). Encodes the live daemon's process id and the command that started it. |
| **Control socket** | `~/.config/scribed/daemon.sock` (Unix domain). The transport the CLI uses to issue commands to the running daemon. |
| **Daemon command** | A typed message the CLI sends over the control socket: `Status`, `Stop`, `Toggle`. Returns a typed reply. |

### Audio

| Term | Meaning |
|---|---|
| **Frame** | A single `f32` sample at 16 kHz mono. The atomic unit of audio. |
| **Chunk** | A contiguous `Vec<f32>` of frames produced by one cpal callback. Default 320 ms = 5,120 frames. |
| **Rolling buffer** | A `VecDeque<f32>` holding the most recent `context_seconds` of frames. Older frames are dropped from the front when capacity is exceeded. |
| **Context window** | Synonym for the rolling buffer's capacity. Configured by `context_seconds` (default 30 s). |
| **dBFS** | Decibels relative to full scale. 0 dBFS = full-amplitude sine wave; silence ≈ -∞ dBFS. We add `+1e-12` before `log10` to avoid NaN. |
| **Silence gate** | The dBFS threshold below which a chunk is considered silent. Default `silence_threshold_dbfs = -45.0`. Chunks below the gate are not transcribed (they make Parakeet hallucinate). |
| **Silence reset** | After `silence_reset_seconds` (1.5 s) of consecutive silent chunks, freeze the current pass into a committed segment and clear the rolling buffer. Prevents drift when the user pauses mid-utterance. |
| **Input device** | A cpal `Device`. Configured by an optional `input_device` substring; the default device is used otherwise. |

### ASR

| Term | Meaning |
|---|---|
| **Engine** | A backend implementation of the `AsrEngine` trait. Owns the model and the inference thread. |
| **Pass** | One execution of `engine.transcribe(&rolling_buffer)`. Produces a `Transcript`. |
| **Transcript** | The full text the engine has produced so far for the current recording session: `committed_text + " " + current_pass_text`. |
| **Segment** | A committed transcript fragment, frozen by a silence reset. Once frozen, segments do not change. |
| **Live tail** | The non-segment, currently-revisable suffix of the transcript. May change between passes; the Output context handles this via retype. |
| **Inference thread** | A long-lived `std::thread` that owns the model. The cpal callback is not allowed to call `transcribe`; chunks reach the inference thread via a channel. |

### Input

| Term | Meaning |
|---|---|
| **Key chord** | A combination of modifier keys + one trigger key, e.g. `Ctrl+Shift+Space`. Parsed from a `&str`. |
| **Hotkey** | A registered key chord that fires recording intents. |
| **Trigger mode** | How the hotkey is interpreted: `Toggle` (press = start, press = stop) or `PushToTalk` (press = start, release = stop). |
| **Recording intent** | A typed signal — `Start`, `Stop`, `Toggle` — from the Input context to the Orchestration context. |
| **Excluded app** | A substring; if the focused window's app name matches, the hotkey is ignored. |

### Output

| Term | Meaning |
|---|---|
| **Keyboard backend** | An implementation of the `KeyboardBackend` trait: ydotool (Wayland), enigo (X11 + macOS), or clipboard (fallback). |
| **Retype state** | The `RetypeState` value object that remembers what's been typed so far for the current recording. On each transcript update it computes the prefix-diff against the previous transcript and emits the minimum keystrokes (some `Backspace`s, then some characters) to reach the new state. |
| **Prefix-diff** | The algorithm: find the longest common prefix between old and new transcripts (in *characters*, not bytes), delete the old tail, type the new tail. |
| **Window snapshot** | A captured handle to the active window at the moment the recording started. Used to restore focus before typing. |
| **Output sink** | A keyboard backend + the captured window snapshot bundled into one consumer of transcripts. |

### Notification

| Term | Meaning |
|---|---|
| **Sound event** | A semantic audio cue: `Start`, `Stop`, `Warning`, `Ready`, `Error`. Maps to an .ogg in `assets/`. |
| **Notifier** | Plays sound events and posts desktop notifications. Honours `sound_effects` config. |

## Invariants

These hold across the whole system and are tested:

1. **One daemon per user.** The PID file is the source of truth. A liveness check (`kill -0` + cmdline match) gates startup.
2. **Inference never blocks the audio callback.** The cpal thread only pushes onto a channel.
3. **Tokio is initialized after `Daemonize::start()`.** Forking with a live tokio runtime leaves dangling state.
4. **Retype operates on `char` counts.** Keystroke arithmetic is character-based; string slicing uses byte offsets resolved from `char_indices()`.
5. **Committed segments are immutable.** Once a silence reset commits a segment, no subsequent pass rewrites it.
6. **Sessions are atomic.** A recording session has a single `WindowSnapshot`; it does not migrate to a different window mid-session.

## Naming conventions

- Types use the language above verbatim: `RetypeState`, `KeyChord`, `RollingBuffer`.
- Module names are the bounded-context name in `snake_case`: `audio`, `asr`, `input`, `output`, `lifecycle`, `config`, `notification`.
- Trait names take the `*Engine`, `*Backend`, `*Sink`, `*Listener`, `*Snapshot` suffixes consistently.
- Avoid model-specific names in public APIs. `SherpaEngine` is fine; do not name a struct `ParakeetTdt` — the engine is the abstraction.

If you need a word that isn't here, **add it here first**, then use it.
