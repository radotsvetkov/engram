#!/bin/sh
# Engram installer for macOS and Linux.
#
# Builds the daemon (engramd) and the terminal client (engram) from source with your
# Rust toolchain and installs them into Cargo's bin directory (usually ~/.cargo/bin).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/radotsvetkov/engram/main/install.sh | sh
#
# Environment:
#   ENGRAM_REF       git ref to build (default: main)
#   ENGRAM_FEATURES  extra cargo features for engramd, comma-separated (e.g. http,docs)
set -eu

REPO="https://github.com/radotsvetkov/engram.git"
REF="${ENGRAM_REF:-main}"
FEATURES="${ENGRAM_FEATURES:-}"

say()  { printf '\033[36m›\033[0m %s\n' "$1"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }

case "$(uname -s)" in
  Darwin|Linux) : ;;
  *) die "Unsupported OS: $(uname -s). Engram builds on macOS and Linux." ;;
esac

command -v git >/dev/null 2>&1 || die "git is required but was not found."
if ! command -v cargo >/dev/null 2>&1; then
  die "The Rust toolchain (cargo) was not found. Install it from https://rustup.rs and re-run."
fi

say "Building Engram from $REF (this compiles an optimized release, so grab a coffee)."

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
git clone --depth 1 --branch "$REF" "$REPO" "$WORKDIR/engram" >/dev/null 2>&1 \
  || die "Could not clone $REPO at ref '$REF'."
cd "$WORKDIR/engram"

if [ -n "$FEATURES" ]; then
  say "Installing engramd with features: $FEATURES"
  cargo install --path crates/engramd --features "$FEATURES" --locked
else
  cargo install --path crates/engramd --locked
fi
cargo install --path crates/engram-cli --locked

BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
ok "Installed engramd and engram to $BIN"
case ":$PATH:" in
  *":$BIN:"*) : ;;
  *) printf '\033[33m!\033[0m Add %s to your PATH to use the commands.\n' "$BIN" ;;
esac

cat <<'EOF'

Next steps:
  engramd            start the daemon, then open http://127.0.0.1:8088
  engram             the terminal UI (starts the daemon for you)
  engram --help      the full command reference

No API key is needed to look around — Engram runs in an honest offline demo mode
until you connect a model in Settings.
EOF
