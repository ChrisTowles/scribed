# Setup

## Linux

### System packages

Debian / Ubuntu:

```bash
sudo apt install libasound2-dev libdbus-1-dev ydotool xdotool
```

Fedora:

```bash
sudo dnf install alsa-lib-devel dbus-devel ydotool xdotool
```

Arch:

```bash
sudo pacman -S alsa-lib dbus ydotool xdotool
```

### Hotkey permission

`scribed` reads `/dev/input/event*` directly so the global hotkey works on
both X11 and Wayland. Your user must be in the `input` group:

```bash
sudo usermod -aG input "$USER"
# log out and back in (or run `newgrp input` for the current shell)
```

Verify: `id -nG | grep -q input && echo ok`.

### Wayland: ydotool

On Wayland, scribed types text via `ydotool`. The `ydotoold` service must be
running. systemd user unit:

```bash
sudo install -m 644 docs/ydotoold.service /etc/systemd/user/ydotoold.service
systemctl --user enable --now ydotoold
```

If `ydotool` complains about `/dev/uinput`, add a udev rule:

```bash
sudo install -m 644 docs/99-uinput.rules /etc/udev/rules.d/99-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

## macOS (Apple Silicon)

On first run, macOS will prompt for **Accessibility** permission. Grant it under
*System Settings → Privacy & Security → Accessibility*.

No system packages are needed; rodio uses CoreAudio and active-win uses the
accessibility APIs already present.

## Configuration

The first invocation of any command will create `~/.config/scribed/`
(`~/Library/Application Support/scribed/` on macOS) on demand. The default
hotkey is `Ctrl+Shift+Space`.

Generate a config file you can edit:

```bash
scribed print-config > ~/.config/scribed/config.toml
```

## Building from source

```bash
git clone <repo>
cd scribed
cargo build --release --features asr     # asr feature pulls in sherpa-onnx
```

The `asr` feature requires the sherpa-onnx native library; sherpa-rs downloads
prebuilt binaries automatically on first build. For NVIDIA CUDA acceleration:

```bash
cargo build --release --features cuda
```

CUDA 12.x toolkit must be installed.

## Models

`scribed` uses Parakeet-TDT-0.6B-v2 (~600 MB ONNX bundle). It is downloaded on
first run to `~/.cache/scribed/` (`~/Library/Caches/scribed/` on macOS).

To pre-download:

```bash
scribed fetch-model    # not yet implemented; downloads on first daemon start
```
