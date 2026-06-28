# Engram desktop

A native desktop shell (Tauri 2) that wraps the agent's dashboard in its own window
and starts the local `engramd` daemon for you. The UI *is* the daemon's dashboard,
served from `engramd`, so there is one source of truth and no duplicated frontend - but
the shell is a *real* desktop app around it, not a webview over a URL.

## Native integration

The shell wires the OS-level surface a real agent app needs and that the web page can't
reach on its own:

- **System tray** with Show / Hide / Restart agent / Open at login / Quit, so Engram is
  reachable from the menu bar even with its window closed.
- **Close-to-tray**: closing the window hides it (the agent stays warm); Cmd/Ctrl+Q or
  the tray's Quit fully exits and lets the daemon sleep itself to zero on idle.
- **Native menu bar** (Edit: undo/cut/copy/paste/select-all, Window, App). Without it,
  clipboard shortcuts don't work inside a webview on macOS - a correctness fix, not chrome.
- **Global hotkey** (Cmd/Ctrl+Shift+Space) to summon Engram from anywhere.
- **Single-instance lock**: a second launch focuses the existing window instead of
  spawning a duplicate daemon + window; an `engram://` deep link is routed to it.
- **Run at login** (a real, persisted autostart entry) toggled from the tray.
- **Window-state persistence** (size/position remembered across launches).
- **Deep links**: the `engram://` URL scheme routes into the running window
  (e.g. `engram://settings`, `engram://task/<id>`).
- **Desktop notifications** when a background task finishes (shown by the dashboard via
  the platform Notification API; enable them in Settings › Messaging).

The native pieces are configured in Rust ([`src-tauri/src/main.rs`](src-tauri/src/main.rs))
and need no Tauri IPC, so they work even though the page is loaded from the daemon's origin.

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
  page on launch. While the app is open (or sitting in the tray) the shell keeps
  `engramd` warm; fully quitting leaves the daemon to sleep to zero on idle, consistent
  with the zero-idle design.
- `engramd` ships as a Tauri **sidecar** (`bundle.externalBin`): `scripts/desktop.sh`
  stages the freshly built daemon as `binaries/engramd-<target-triple>` so the bundled
  app is self-contained and users don't need it on `PATH`. (In a dev run the shell also
  best-effort-spawns it from the workspace `target/`.)
- A restart requested from the tray or Settings re-execs the daemon **in place** and
  carries the in-memory API key forward through the successor's environment, so reloading
  boot-time settings never silently drops a connected provider back to the offline mock -
  and the key still never touches disk.
