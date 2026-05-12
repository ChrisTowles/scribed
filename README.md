# scribed

Local streaming dictation daemon. Press a hotkey, speak, the text appears in your focused window. Powered by NVIDIA Parakeet-TDT via sherpa-onnx; runs offline.

Linux (X11 + Wayland) and macOS (Apple Silicon). Native single binary.

See [DOMAIN.md](DOMAIN.md) for the vocabulary used throughout this codebase, and [docs/](docs/) for setup and architecture notes.

## Status

In development. The Rust port of [claude-stt](https://github.com/) (Python). Phases land one at a time — check `git log` for what works today.

## Quick start

```bash
# Build
cargo build --release --features asr

# Show config + status
./target/release/scribed status

# Start in background
./target/release/scribed start --background

# Toggle recording (or press the hotkey: default Ctrl+Shift+Space)
./target/release/scribed toggle

# Stop daemon
./target/release/scribed stop
```

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
