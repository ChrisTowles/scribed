# Architecture

See [`../DOMAIN.md`](../DOMAIN.md) for the ubiquitous language first. This
document is the implementation map.

## Process layout

scribed is a single binary. There is at most one daemon per user. Other
invocations of `scribed` (the CLI commands) are short-lived clients that
talk to the daemon over a Unix domain socket.

```
┌──────────────────┐         JSON line-protocol         ┌──────────────────┐
│  scribed status  │ ─────────────────────────────────► │                  │
│  scribed stop    │                                    │     daemon       │
│  scribed toggle  │ ◄───────────────────────────────── │  (scribed run)   │
└──────────────────┘                                    └──────────────────┘
                                                                  │
                                                                  ▼
                                              ┌──────────────────────────────────┐
                                              │  /dev/input  ─►  Input           │
                                              │  cpal mic    ─►  Audio           │
                                              │                  ▼               │
                                              │                  ASR             │
                                              │                  ▼               │
                                              │  ydotool   ◄─    Output          │
                                              └──────────────────────────────────┘
```

## Bounded contexts → modules

| Context | Module | Tests |
|---|---|---|
| Audio | `src/audio/` | `audio::dsp`, `audio::rolling`, `audio::device`, `audio::capture` |
| ASR | `src/asr/` | `asr::driver` (full coverage), `asr::sherpa` (feature-gated) |
| Input | `src/input/` | `input` (KeyChord), `input::aggregator`, `input::evdev_listener` |
| Output | `src/output/` | `output::retype` (proptest), `output::backend`, `output::window` |
| Lifecycle | `src/lifecycle/` | `lifecycle::pidfile`, `lifecycle::ipc`, `lifecycle::liveness`, `lifecycle::protocol` |
| Configuration | `src/config.rs` | `config` |
| Notification | `src/notification/` | `notification` |
| Orchestration | `src/service/` | `service::timer` |

## Threading model

```
┌──────────────────┐
│  cpal callback   │  real-time audio thread (owned by cpal)
│   thread         │  → pushes Vec<f32> chunks onto crossbeam_channel
└──────────────────┘
         │
         ▼ chunk channel
┌──────────────────┐
│  inference       │  long-lived std::thread; owns the ASR model
│   thread         │  → runs StreamingDriver on each chunk; emits transcripts
└──────────────────┘
         │
         ▼ transcript callback
┌──────────────────┐
│  retype / output │  invokes KeyboardSink; ydotool/enigo/clipboard
└──────────────────┘

┌──────────────────┐
│  evdev reader    │  one std::thread per keyboard device
│   threads        │  → push KeyEvents into Aggregator (mutex)
└──────────────────┘
         │
         ▼ on intent
┌──────────────────┐
│  tokio runtime   │  control plane: UDS server, signal handlers,
│   (single rt)    │  timer ticker. Handles IPC commands and shutdown.
└──────────────────┘
```

### Why a hybrid sync/async model?

- **cpal callback** runs in a real-time audio thread that must never allocate.
  Pushing onto a bounded channel and returning is the only safe option.
- **Inference** holds a single long-lived thread for tens of milliseconds at a
  time. Putting it on tokio's executor would block other tasks; `spawn_blocking`
  is for one-off operations, not lifetimes.
- **Hotkey reading** is `read(2)` on `/dev/input`. That's a blocking syscall
  with no async wrapper in evdev; one thread per device is the canonical
  pattern.
- **Tokio** handles only the IPC server, signal handlers, and the 100 ms
  timer tick — workloads that benefit from async multiplexing.

## The fork-vs-Tokio rule

`background_spawn` uses `std::process::Command` rather than
`daemonize::Daemonize::start()`. The reason: the parent process (the CLI)
already used a tokio runtime to talk to other daemons via the IPC client.
Forking with a live tokio reactor leaves dangling file descriptors and
poisoned thread state.

Spawning a fresh child via `Command` with `setsid()` in `pre_exec` accomplishes
the same goal (detach from terminal) without the forking hazard. The child
constructs its tokio runtime fresh.

This is mirrored in the test suite — see `tests/daemon_lifecycle.rs`.

## Streaming algorithm

Ported from Python `claude_stt/engines/nemo.py:174-253`. The
[`StreamingDriver`](../src/asr/driver.rs) walks one chunk at a time:

1. Compute energy. If below `silence_threshold_dbfs`, the chunk is "silent".
2. **Leading silence** (no speech started yet) → drop the chunk; don't even
   buffer it. Prevents Parakeet from hallucinating on silence.
3. **Mid-utterance silence** → buffer it (the user may be pausing) and increment
   the silent-chunk counter. When the counter hits `silence_reset_chunks`,
   commit the current pass as a Segment, clear the buffer, return to the
   "leading silence" state.
4. **Speech** → reset the silent counter, push to the rolling buffer, run
   inference on the *full* buffer, emit the new transcript.

The rolling buffer never exceeds `context_seconds * 16000` frames; older
frames are dropped from the front.

## Retype invariant

Ported from Python `claude_stt/daemon_service.py:73-109`. The
[`RetypeState`](../src/output/retype.rs) state machine tracks what's been
typed and computes the minimum (backspaces + insert) needed to reach the new
transcript.

The invariant is property-tested: after every diff+apply round, the simulated
window state must equal the engine's internal `typed_text` — for both ASCII
and arbitrary Unicode inputs.

The character-vs-byte rule is explicit in the code: keystroke counts use
`chars().count()`; string slicing resolves byte offsets via `char_indices()`.
