# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- First public release preparation: documentation, install script, and release automation.

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
