# Auto-updater (Tauri v2)

The desktop app has the [`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/)
wiring in place, but it ships **off by default**. It does nothing — and adds no
dependency to a normal `cargo build` — until you (a) generate a signing keypair,
(b) fill in the config below, and (c) build with the `updater` cargo feature.

This is deliberate: a Tauri build with a *placeholder* signing pubkey fails at bundle
time, so the whole path is gated behind the `updater` feature and stays out of dev/default
builds.

## What's already wired

- **Dependency** (`src-tauri/Cargo.toml`): `tauri-plugin-updater = { version = "2", optional = true }`,
  behind the `updater` feature (`updater = ["dep:tauri-plugin-updater"]`), off by default.
- **Plugin registration** (`src-tauri/src/main.rs`): `.plugin(tauri_plugin_updater::Builder::new().build())`,
  compiled only under `#[cfg(feature = "updater")]`.
- **Config template** (`src-tauri/updater.capability.json.example`): the `updater:default` capability,
  granted to the main window's **local** context only (never the remote daemon origin). It lives OUTSIDE
  `capabilities/` on purpose — a capability referencing `updater:default` can't be resolved unless the
  updater plugin is actually linked, so leaving it in `capabilities/` would break the default (feature-off)
  build. You copy it in when enabling the updater (step 2b).
- **Config keys** (`src-tauri/tauri.conf.json`): `plugins.updater` (endpoints + pubkey) and
  `bundle.createUpdaterArtifacts: true` are NOT in the default config for the same reason — you add them
  when enabling (step 2a). Templates are in this file (step 2).
- **Entry points** (`src-tauri/src/main.rs`, all `#[cfg(feature = "updater")]`):
  - `check_for_updates_on_launch(...)` — a best-effort background check fired in `setup`.
    If an update is available it raises a **native notification** (via the existing
    `tauri-plugin-notification` plumbing) instead of force-installing. Every failure mode
    (placeholder endpoint, offline, bad manifest) is logged and swallowed — it never blocks
    launch or crashes.
  - `check_for_updates` — a Tauri command the frontend can `invoke()` on demand; returns the
    new version string, `None` if up to date, or an `Err(String)` (never a panic) if it can't check.
  - `install_update` — a Tauri command that downloads + installs the pending update and relaunches.
    Signature verification against the configured `pubkey` is enforced by the plugin, so an
    unsigned/tampered artifact is rejected before it is applied.

## Remaining user steps (you must do these)

These can't be scaffolded — they need *your* private key and *your* server.

### 1. Generate a signing keypair

```sh
# from desktop/src-tauri (or anywhere with the Tauri CLI installed)
cargo tauri signer generate -w ~/.tauri/engram.key
```

This prints a **public key** and writes the **private key** to `~/.tauri/engram.key`
(optionally password-protected). **Never commit the private key.**

### 2. Activate the updater config + capability

**2a.** Add a `plugins.updater` block to `src-tauri/tauri.conf.json` and set
`bundle.createUpdaterArtifacts: true`:

```jsonc
"plugins": {
  "deep-link": { "desktop": { "schemes": ["engram"] } },
  "updater": {
    "endpoints": ["https://your-update-server/engram/{{target}}/{{arch}}/{{current_version}}"],
    "pubkey": "<the public key printed in step 1 — the whole dW50cnVzdGVk… blob>"
  }
}
```

**2b.** Copy the capability template into the discovered dir:

```sh
cp src-tauri/updater.capability.json.example src-tauri/capabilities/updater.json
```

- `pubkey` → the public key printed in step 1 (the whole `dW50cnVzdGVk…` base64 blob).
- `endpoints` → your real update-manifest URL. Tauri expands `{{target}}`, `{{arch}}`, and
  `{{current_version}}`. The server must return either **204 No Content** (up to date) or a JSON
  manifest, e.g.:

  ```json
  {
    "version": "0.3.0",
    "notes": "What's new…",
    "pub_date": "2026-07-02T00:00:00Z",
    "platforms": {
      "darwin-aarch64": {
        "signature": "<contents of the .sig file>",
        "url": "https://your-server/engram/Engram_0.3.0_aarch64.app.tar.gz"
      }
    }
  }
  ```

  Host the release artifacts (the `*.app.tar.gz` / `*.AppImage.tar.gz` / `*.msi.zip` and their
  `.sig` files produced by the build) anywhere static — S3, GitHub Releases, a plain web server.

### 3. Set the private key as a CI secret

The build must sign the update artifacts, so give it the private key via environment variables
(never on the command line):

```sh
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/engram.key)"   # or the key contents directly
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"                    # only if you set a password
```

In CI, store `TAURI_SIGNING_PRIVATE_KEY` (and the password, if any) as encrypted secrets and
export them into the build step.

### 4. Build with the feature on

```sh
cd desktop/src-tauri
cargo tauri build --features updater
```

This emits the normal bundle **plus** the signed updater artifacts (because
`createUpdaterArtifacts: true`). Upload those to your endpoint and update the manifest.

> A build **without** the signing key set, or **with** the still-placeholder `pubkey`, will fail —
> that's why the feature is off by default. Leave it off for dev / unsigned local builds.
