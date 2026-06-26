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

One command from the repo root builds the daemon and opens the app:

```sh
scripts/desktop.sh          # builds engramd, then runs `cargo tauri dev`
```

Or by hand:

```sh
cargo build --release -p engramd --features http   # the runtime the shell launches
cd desktop/src-tauri && cargo tauri dev
```

This opens the Engram window, waits for `engramd` to come up on `127.0.0.1:8088`, and
loads the dashboard. Because the window loads the daemon's own origin, the dashboard's
API calls work with no CORS configuration.

## Build a distributable app

```sh
scripts/desktop.sh build    # or: cd desktop/src-tauri && cargo tauri build
```

Produces a native bundle under `target/release/bundle/` (`.app`/`.dmg` on macOS,
`.deb`/`.AppImage` on Linux, `.msi` on Windows). The macOS `.app` build is verified: it
embeds the Engram icon and comes out around 8 MB.

## Notes

- **Icons** are the real Engram neuron mark, generated for every platform from
  [`assets/brand/icon.svg`](../assets/brand/icon.svg). To regenerate after a design
  change, render a 1024px PNG and run `cargo tauri icon ../../assets/brand/icon-1024.png`
  from `desktop/src-tauri`.
- The window opens only once the daemon answers, so you never see a connection-refused
  page on launch. While the app is open the shell keeps `engramd` alive; closing the
  window quits the app and leaves the daemon to sleep to zero on idle, consistent with
  the zero-idle design.
- For a fully self-contained bundle, ship `engramd` as a Tauri **sidecar**
  (`bundle.externalBin`) so users don't need it on `PATH`. The shell already
  best-effort-spawns it from the workspace `target/`; wiring it as a sidecar is the
  packaging follow-up.
