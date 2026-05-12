# scribed

Local streaming dictation daemon. Press a hotkey, speak, the text appears in your focused window. Powered by NVIDIA Parakeet-TDT via sherpa-onnx; runs offline.

Linux (X11 + Wayland) and macOS (Apple Silicon). Native single binary.

See [DOMAIN.md](DOMAIN.md) for the vocabulary used throughout this codebase, and [docs/](docs/) for setup and architecture notes.

## Status

In development. The Rust port of [claude-stt](https://github.com/) (Python). Phases land one at a time — check `git log` for what works today.

## Quick start

```bash
# Build with the ASR engine (downloads sherpa-onnx native libs into target/)
cargo build --release --features asr

# Fetch the Parakeet model bundle (~460 MB, cached at ~/.cache/scribed/)
./target/release/scribed fetch-model

# End-to-end smoke test: transcribe a WAV without starting the daemon
./target/release/transcribe_wav ~/.cache/scribed/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/test_wavs/0.wav
# → Transcript:  Well, I don't wish to see it any more, observed Phebe,...
# → Inference: ~115 ms (64x realtime on CPU)

# Daemon control
./target/release/scribed status
./target/release/scribed start --background     # ctrl+shift+space to record
./target/release/scribed toggle
./target/release/scribed stop
```

The shared `libsherpa-onnx-c-api.so` is downloaded into `target/release/` by
`sherpa-rs-sys`; an `$ORIGIN` rpath ([`build.rs`](build.rs)) lets the binary
find it without `LD_LIBRARY_PATH`. For distribution, ship the binary alongside
the two `.so` files.

## Platform requirements

### Linux

- **Wayland users:** the daemon types text via `ydotool`; install it and run `ydotoold`.
- **Global hotkey:** the daemon reads `/dev/input/event*` directly. Your user must be in the `input` group:
  ```bash
  sudo usermod -aG input "$USER"
  # log out and back in
  ```
- ALSA dev headers for build: `sudo apt install libasound2-dev` (Debian/Ubuntu).

### macOS (Apple Silicon)

- On first run, grant Accessibility permission to `scribed` (System Settings → Privacy & Security → Accessibility).

## Configuration

TOML at `~/.config/scribed/config.toml`. Run `scribed status` once to generate a default file you can edit.

## License

MIT. See [LICENSE](LICENSE).
