#!/usr/bin/env bash
# Build and run the Engram desktop app.
#
# It builds the engramd daemon (with real model providers) so the shell has something to
# launch, then starts the Tauri window, which spawns the daemon and opens its dashboard.
#
#   scripts/desktop.sh         # run in dev (cargo tauri dev)
#   scripts/desktop.sh build   # produce a native bundle (.app/.dmg/...) instead
#
# Prereqs: Rust, and the Tauri CLI (cargo install tauri-cli --version '^2').
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> building engramd (release, real providers)"
cargo build --release -p engramd --features http

if ! command -v cargo-tauri >/dev/null 2>&1; then
  echo "error: the Tauri CLI is not installed. Run: cargo install tauri-cli --version '^2'" >&2
  exit 1
fi

cd desktop/src-tauri
if [ "${1:-dev}" = "build" ]; then
  echo "==> bundling the desktop app"
  cargo tauri build
else
  echo "==> launching the desktop app (dev)"
  cargo tauri dev
fi
