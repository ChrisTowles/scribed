# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Scribed is a local streaming dictation daemon for Linux (X11/Wayland) and macOS (Apple Silicon), written in Rust. It uses a streaming Zipformer RNN-T transducer via `sherpa-onnx` (Nemotron streaming model) to type live partial transcriptions into the focused window when a global hotkey is held.

For non-trivial changes, read these first — they define the bounded contexts, threading model, and naming conventions used across the codebase:

- @DOMAIN.md — ubiquitous language, invariants, naming.
- @docs/ARCHITECTURE.md — threading model, channels, bounded contexts.
- @docs/SETUP.md — platform setup (uinput group, ydotoold, macOS Accessibility).

## Verify before declaring done

Run this chain before reporting any change as complete:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

Clippy is strict: warnings are denied. `cargo nextest` is the project's test runner — do not substitute `cargo test`. `tests/daemon_lifecycle.rs` downloads the ~442 MB Nemotron bundle to `~/.cache/scribed/` on first run; subsequent runs reuse it.

## Workflow conventions

- **Hard cutover.** Do not write backward-compatibility shims, deprecation paths, or feature flags to preserve old behavior. When changing a default, schema, or interface, change every caller and remove the old path in the same change.
- **Commit style.** Conventional commits with scope and PR number: `type(scope): subject (#N)` — e.g., `feat(asr): swap to sherpa-onnx 1.13 + Nemotron streaming model (#4)`. Common scopes: `asr`, `audio`, `config`, `lifecycle`, `input`, `output`, `ci`.
- **rustfmt** uses `max_width = 100` (see `rustfmt.toml`). Edits are auto-formatted by the PostToolUse hook in `.claude/settings.json`.

## Platform gotchas

- **Linux hotkey** reads `/dev/input/event*` via evdev. The user must be in the `input` group; udev rules live at `docs/99-uinput.rules`.
- **Wayland output** requires `ydotoold` running; X11 uses `enigo` directly. The output backend is chosen at runtime.
- **Audio build deps (Linux):** `libasound2-dev`, `libdbus-1-dev`, `libxcb1-dev` are required for cpal + notifications + active-window.
- **sherpa-onnx native libs** are downloaded by `build.rs` into `target/<profile>/sherpa-onnx-prebuilt/`. CI caches these; locally the first release build is slow.
- **Inference-thread isolation.** The cpal audio callback must never call `transcribe()` — audio chunks cross to the inference thread via a bounded `crossbeam-channel`. The sherpa model is owned by the inference thread.
