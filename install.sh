#!/bin/sh
# Engram installer for macOS and Linux.
#
# By default this fetches a prebuilt static binary for your platform — fast, no Rust
# toolchain needed — and installs `engramd` + `engram` into one place on your machine.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/radotsvetkov/engram/main/install.sh | sh
#
# Build from source instead (for an architecture with no prebuilt binary, to track a
# branch, or to add cargo features not baked into the release build):
#   curl -fsSL https://raw.githubusercontent.com/radotsvetkov/engram/main/install.sh | sh -s -- --source
#
# Environment:
#   ENGRAM_INSTALL_DIR   where to put the binaries (default: $CARGO_HOME/bin or ~/.cargo/bin —
#                        one canonical location regardless of install method, so a later
#                        `--source` run or `cargo install` upgrades the same binaries in place)
#   ENGRAM_VERSION       release tag to install, e.g. v0.2.1 (default: the latest release)
#   ENGRAM_REF           git ref to build from in --source mode (default: main)
#   ENGRAM_FEATURES      extra cargo features for --source mode, e.g. http,docs (the prebuilt
#                        release binaries already ship with http,docs — see below)
set -eu

REPO="radotsvetkov/engram"
INSTALL_DIR="${ENGRAM_INSTALL_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
MODE="prebuilt"
for arg in "$@"; do
  case "$arg" in
    --source) MODE="source" ;;
    -h | --help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

say()  { printf '\033[36m›\033[0m %s\n' "$1"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; }
warn() { printf '\033[33m!\033[0m %s\n' "$1"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }

case "$(uname -s)" in
  Darwin | Linux) : ;;
  *) die "Unsupported OS: $(uname -s). Engram builds on macOS and Linux." ;;
esac

# ---- build from source (opt-in, or the fallback for an unsupported target) ------------

install_from_source() {
  ref="${ENGRAM_REF:-main}"
  features="${ENGRAM_FEATURES:-}"
  command -v git >/dev/null 2>&1 || die "git is required but was not found."
  if ! command -v cargo >/dev/null 2>&1; then
    die "The Rust toolchain (cargo) was not found. Install it from https://rustup.rs and re-run, or drop --source to use a prebuilt binary."
  fi
  say "Building Engram from '$ref' (this compiles an optimized release, so grab a coffee)."
  workdir="$(mktemp -d)"
  trap 'rm -rf "$workdir"' EXIT
  git clone --depth 1 --branch "$ref" "https://github.com/$REPO.git" "$workdir/engram" >/dev/null 2>&1 \
    || die "Could not clone https://github.com/$REPO at ref '$ref'."
  cd "$workdir/engram"
  if [ -n "$features" ]; then
    say "Installing engramd with features: $features"
    cargo install --path crates/engramd --features "$features" --locked --root "${CARGO_HOME:-$HOME/.cargo}"
  else
    say "Installing engramd (offline/mock provider only — set ENGRAM_FEATURES=http,docs for a real model)"
    cargo install --path crates/engramd --locked --root "${CARGO_HOME:-$HOME/.cargo}"
  fi
  cargo install --path crates/engram-cli --locked --root "${CARGO_HOME:-$HOME/.cargo}"
  ok "Installed engramd and engram to ${CARGO_HOME:-$HOME/.cargo}/bin"
}

# ---- prebuilt binary (default) ---------------------------------------------------------

resolve_latest_tag() {
  api="https://api.github.com/repos/$REPO/releases/latest"
  if command -v jq >/dev/null 2>&1; then
    curl -fsSL "$api" | jq -r .tag_name
  else
    curl -fsSL "$api" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
  fi
}

install_prebuilt() {
  command -v curl >/dev/null 2>&1 || die "curl is required but was not found."

  case "$(uname -s)" in
    Darwin) plat="apple-darwin" ;;
    Linux) plat="unknown-linux-musl" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) triple="x86_64-$plat" ;;
    arm64 | aarch64) triple="aarch64-$plat" ;;
    *)
      warn "No prebuilt binary for $(uname -m); building from source instead."
      install_from_source
      return
      ;;
  esac

  version="${ENGRAM_VERSION:-}"
  if [ -z "$version" ]; then
    say "Resolving the latest release..."
    version="$(resolve_latest_tag)"
    [ -n "$version" ] || die "Could not resolve the latest release (check your connection, or set ENGRAM_VERSION explicitly, or pass --source)."
  fi

  asset="engram-${version}-${triple}"
  url="https://github.com/$REPO/releases/download/${version}/${asset}.tar.gz"
  say "Downloading Engram $version for $triple..."
  workdir="$(mktemp -d)"
  trap 'rm -rf "$workdir"' EXIT
  if ! curl -fsSL "$url" -o "$workdir/$asset.tar.gz"; then
    warn "No prebuilt binary at $url"
    warn "Falling back to a source build (pass --source to skip straight to this next time)."
    install_from_source
    return
  fi
  if curl -fsSL "$url.sha256" -o "$workdir/$asset.tar.gz.sha256" 2>/dev/null; then
    if (cd "$workdir" && sha256sum -c "$asset.tar.gz.sha256") >/dev/null 2>&1 \
      || (cd "$workdir" && shasum -a 256 -c "$asset.tar.gz.sha256") >/dev/null 2>&1; then
      ok "Checksum verified"
    else
      die "Checksum verification failed for $asset.tar.gz — the download may be corrupted or tampered with."
    fi
  else
    warn "No checksum file published for this release; skipping verification."
  fi

  tar -xzf "$workdir/$asset.tar.gz" -C "$workdir"
  mkdir -p "$INSTALL_DIR"
  cp "$workdir/$asset/engramd" "$workdir/$asset/engram" "$INSTALL_DIR/"
  chmod +x "$INSTALL_DIR/engramd" "$INSTALL_DIR/engram"
  ok "Installed engramd and engram $version to $INSTALL_DIR"
  say "(built with the http and docs cargo features — a real model provider works out of the box)"
}

if [ "$MODE" = "source" ]; then
  install_from_source
else
  install_prebuilt
fi

BIN="$INSTALL_DIR"
case ":$PATH:" in
  *":$BIN:"*) : ;;
  *) printf '\033[33m!\033[0m Add %s to your PATH to use the commands:\n    export PATH="%s:$PATH"\n' "$BIN" "$BIN" ;;
esac

cat <<'EOF'

Next steps:
  engramd            start the daemon, then open http://127.0.0.1:8088
  engram             the terminal UI (starts the daemon for you)
  engram --help      the full command reference

No API key is needed to look around — Engram runs in an honest offline demo mode
until you connect a model in Settings.
EOF
