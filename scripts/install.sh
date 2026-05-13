#!/usr/bin/env bash
# Install scribed into ~/.cargo/bin/, with the sherpa-onnx native libs colocated
# so the binary's $ORIGIN rpath finds them at runtime.
#
# User-level only — never invokes sudo. System-level setup (apt packages, group
# membership, ydotoold systemd unit, /dev/uinput udev rule) is documented in
# docs/SETUP.md.
#
# Idempotent: re-running rebuilds with cargo's incremental cache and overwrites
# the installed binary + libs in place.
#
# Usage:
#   scripts/install.sh                  # install scribed
#   scripts/install.sh --with-helpers   # also install transcribe_wav, audio_probe
#   scripts/install.sh --help

set -euo pipefail

with_helpers=0
for arg in "$@"; do
  case "$arg" in
    --with-helpers) with_helpers=1 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "unknown flag: $arg" >&2
      exit 2 ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"
dest="${CARGO_HOME:-$HOME/.cargo}/bin"

case "$(uname -s)" in
  Linux)  lib_ext=so ;;
  Darwin) lib_ext=dylib ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

bins=(scribed)
if [[ $with_helpers -eq 1 ]]; then
  bins+=(transcribe_wav audio_probe)
fi

cd "$repo_dir"

build_args=(--release --locked)
for b in "${bins[@]}"; do build_args+=(--bin "$b"); done

echo "==> building (release): ${bins[*]}"
cargo build "${build_args[@]}"

mkdir -p "$dest"

echo "==> installing binaries to $dest"
for b in "${bins[@]}"; do
  install -m 0755 "target/release/$b" "$dest/$b"
  echo "    $b"
done

echo "==> installing sherpa-onnx libs to $dest"
# $ORIGIN rpath (set by build.rs) makes the binary find sidecar libs in its
# own directory, so libs must travel with the binary. cp -P preserves any
# version-symlink chains.
find target/release -maxdepth 1 \( -name "*.$lib_ext" -o -name "*.$lib_ext.*" \) -print0 \
  | while IFS= read -r -d '' lib; do
      cp -P "$lib" "$dest/"
      echo "    $(basename "$lib")"
    done

cat <<EOF

==> done.

Next steps:
  - Fetch the ~460 MB Parakeet model bundle (first run only):
      scribed fetch-model
  - Generate a config you can edit:
      scribed print-config > ~/.config/scribed/config.toml
  - Start the daemon:
      scribed start --background

On Linux you also need: membership in the 'input' group, and on Wayland a
running ydotoold. See docs/SETUP.md for the system-level setup.
EOF
