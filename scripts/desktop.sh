#!/usr/bin/env bash
# Build and run the Engram desktop app.
#
# It builds the engramd daemon (with real model providers), stages it as the Tauri sidecar
# binary the bundle expects, then starts the Tauri window, which spawns the daemon and opens
# its dashboard.
#
#   scripts/desktop.sh         # run in dev (cargo tauri dev)
#   scripts/desktop.sh build   # produce a native bundle (.app/.dmg/.deb/.AppImage/.msi)
#
# Prereqs: Rust, and the Tauri CLI (cargo install tauri-cli --version '^2').
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> building engramd (release, real providers + OS keychain + browser)"
# http = real LLM providers; keyring = persist the API key in the OS Keychain across restarts;
# browser-cdp = interactive browser automation + screenshots; docs = read PDF/DOCX/XLSX uploads.
cargo build --release -p engramd --features http,keyring,browser-cdp,docs

# Stage the daemon as the Tauri sidecar. Tauri's externalBin looks for a binary suffixed with
# the target triple (e.g. engramd-aarch64-apple-darwin), placed next to the app executable in
# the bundle. Without this step `cargo tauri build` ships an app that can't find its daemon.
TRIPLE="$(rustc -vV | sed -n 's/host: //p')"
SIDECAR_DIR="desktop/src-tauri/binaries"
mkdir -p "$SIDECAR_DIR"
EXT=""
case "$TRIPLE" in *windows*) EXT=".exe" ;; esac
cp -f "target/release/engramd${EXT}" "${SIDECAR_DIR}/engramd-${TRIPLE}${EXT}"
echo "==> staged sidecar: ${SIDECAR_DIR}/engramd-${TRIPLE}${EXT}"

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
