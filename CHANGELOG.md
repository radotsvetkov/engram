# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **The README's prebuilt-binary link no longer goes stale.** It hardcoded a specific
  release tag in the download filename, so it always fetched v0.2.0 no matter how many
  versions had shipped since.

### Changed
- **`install.sh` fetches a prebuilt binary by default** (resolves the latest GitHub
  release, verifies its checksum, no Rust toolchain needed) instead of always building
  from source — the same one-line experience as most modern CLI tools. Building from
  source is now an opt-in `--source` flag, and the automatic fallback when there's no
  prebuilt binary for your architecture.
- **README quickstart is one command** instead of three competing options, with a
  collapsed "other ways to install" section for the tarball-by-hand / build-from-source /
  VPS paths.
- New **"Run on a server"** README section, and `deploy/README.md` now leads with the
  prebuilt Linux binary (installed straight into `/usr/local/bin`) instead of requiring
  Docker or a manual musl cross-compile toolchain just to get `engramd` onto a VPS.

## [0.2.1] - 2026-07-03

The terminal client grows up, and a config-save secret-loss bug dies.

### Added
- **TUI boot splash** — the Engram neuron logomark as half-block pixel art (firing synapse
  in the brand teal), shown while the client connects; any key skips it.
- **Proposed skills in the terminal** — a distilled-but-inactive skill now shows as
  `◆ proposed` in the TUI Skills view (`a` adopts it) and in `engram skills list`;
  new `engram skills show <id>` (manifest, versions, learning history) and
  `engram skills adopt <id>`.
- **Agent tools are switchable from both surfaces** — a new "Agent tools" section in the
  TUI Settings view toggles each tool on/off, and `engram tools enable|disable <name>`
  does the same from scripts (both write `security.disabled_tools`).
- **`engram mcp list|add|remove`** — manage MCP servers without hand-editing JSON.
- **`engram sessions list|show`** — list chat sessions and print a transcript.
- **`engram stop` / `engram restart`** — daemon lifecycle from the CLI (`stop` never
  auto-spawns a daemon just to stop it).
- The TUI's theme and mouse-capture preferences persist across runs (`~/.engram/cli.json`).

### Fixed
- **The skills list no longer vanishes when a proposal exists.** The daemon sends
  `"active": null` for a proposed skill; the client's wire type demanded a number, so one
  proposal made the whole `/v1/skills` payload fail to decode and the TUI/CLI showed no
  skills at all.
- **A config round-trip no longer wipes a remote MCP server's bearer token.** The redacted
  config reports only `bearer_set`, and the daemon never restored the stored token when the
  array came back — any settings save (desktop, TUI, or the new `engram mcp`) silently broke
  auth to every remote server. The bearer now follows the same "blank keeps it" rule as
  every other secret (`clear_bearer` removes it), and url-only remote servers survive
  round-trips instead of being dropped for having no spawn command.
- `engram stop`/`restart` judge success by the daemon's actual state (a 401 from a
  token-protected daemon is no longer reported as "✓ stopped"), `restart` honours
  `--no-spawn`, and tool toggles preserve `disabled_tools` entries that name MCP or
  daemon-registered tools instead of silently re-enabling them.

## [0.2.0]

The agent grows up: a full tool-use loop, a redesigned control center, and a terminal client.

### Added
- **Agentic tool loop** (`engram-agent`) with built-in tools for memory, workdir-confined
  files, a multi-backend shell (local / Docker / SSH), keyless web search and fetch, a
  headless and an interactive Chrome browser over CDP, depth-bounded subagents, and
  vision / image / speech through the metered gateway.
- **MCP client** — connect any Model Context Protocol server and use its tools as native,
  audited Engram tools.
- **Desktop control center** — a single-page window with a Kanban board fed by one chat
  composer, glass-box signed task receipts, and an ambient trust/cost spine, over SSE.
- **Terminal client** (`engram-cli`) — a scriptable CLI (`--json` everywhere) and a
  full-screen TUI that share the daemon's HTTP API.
- **Conversation memory** — chat turns persist to episodic memory, past turns are recalled,
  and identity facts are learned across sessions.
- **Static (model2vec) embedder** for real synonym-level recall, in pure Rust with no ONNX
  runtime (`ENGRAM_EMBED=static`).
- **Scheduling** with a live natural-language "when" preview and generated systemd
  socket-activation / wake-timer units.
- **Document ingest** (`--features docs`) for PDF / DOCX / XLSX / CSV chat uploads.

### Changed
- The gateway now speaks Anthropic's native Messages transport (prompt caching + streaming)
  alongside the OpenAI-compatible path shared by every other provider.
- Memory is region-partitioned and scope-aware, so facts learned inside a project stay there.

### Security
- The **taint boundary** cuts the shell and egress once a run has read untrusted content and
  is also holding private data — breaking the prompt-injection → exfiltration chain.
- The SSRF guard pins each connection to the validated IP and re-checks every redirect hop.

## [0.1.0]

The foundation: a reactive core, a signed ledger, and hybrid memory.

### Added
- **Reactive kernel** (`engram-core`): a priority event bus, a wake/sleep lifecycle that
  exits to zero at idle, and an append-only, BLAKE3-hash-chained, Ed25519-signed audit ledger
  with offline `engramd verify`.
- **Hybrid memory** (`engram-memory`): embedded SQLite with FTS5 + vector recall fused by
  Reciprocal Rank Fusion, memory regions, tiers, and idle consolidation.
- **Provider-agnostic gateway** (`engram-gateway`) with per-call metering and taint redaction.
- **Signed, capability-sandboxed WASM skills** (`engram-skills`) and the replay-based
  learning loop with an A/B promotion gate.
- **Benchmark harness** (`engram-bench`) for paraphrase recall and footprint.

[Unreleased]: https://github.com/radotsvetkov/engram/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/radotsvetkov/engram/releases/tag/v0.2.0
[0.1.0]: https://github.com/radotsvetkov/engram/releases/tag/v0.1.0
