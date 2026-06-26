# Engram desktop

A native desktop shell (Tauri 2) that wraps the agent's dashboard in its own window
and starts the local `engramd` daemon for you. It is a thin shell on purpose: the UI
*is* the daemon's dashboard, served from `engramd`, so there is one source of truth and
no duplicated frontend.

## Prerequisites

- Rust (already required for the workspace).
- The Tauri CLI: `cargo install tauri-cli --version '^2'`.
- Platform webview deps: macOS has WKWebView built in; on Linux install
  `webkit2gtk` (`libwebkit2gtk-4.1-dev`) and `libappindicator`.
- The `engramd` binary reachable on `PATH` or in the workspace `target/` (the shell
  looks in `target/release` and `target/debug`). Build it first:
  `cargo build --release -p engramd`.

## Run (dev)

```sh
cd desktop/src-tauri
cargo tauri dev
```

This opens the Engram window, starts `engramd` on `127.0.0.1:8088`, and loads the
dashboard. Because the window loads the daemon's own origin, the dashboard's API calls
work with no CORS configuration.

## Build a distributable app

```sh
cd desktop/src-tauri
cargo tauri build
```

Produces a native bundle (`.app`/`.dmg` on macOS, `.deb`/`.AppImage` on Linux,
`.msi` on Windows) under `target/release/bundle/`.

## Notes

- **Icons** are minimal placeholders. Replace them with a real source image via
  `cargo tauri icon path/to/logo.png`, which regenerates every required size.
- For a fully self-contained bundle, ship `engramd` as a Tauri **sidecar**
  (`bundle.externalBin`) so users don't need it on `PATH`. The shell already
  best-effort-spawns it; wiring it as a sidecar is the packaging follow-up.
- Closing the window leaves `engramd` to sleep to zero on idle - consistent with the
  zero-idle design rather than force-killing it.
