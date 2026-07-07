# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.8] - 2026-07-08

### Added
- Bare URLs in chat text now auto-linkify (not just markdown `[text](url)` links) - so a reply
  like "open http://localhost:8000/" after the agent starts a local dev server is directly
  clickable, opening in your real browser like any other link.

### Fixed
- The desktop app now remembers which project was active and reopens on it after a full quit and
  relaunch, instead of always landing back on Personal.

## [0.3.7] - 2026-07-07

### Fixed
- A tainted run's network-isolated shell sandbox (Seatbelt on macOS, bubblewrap on Linux) denied
  *all* networking, including loopback - so the agent could never start and verify a local dev
  server (e.g. `python3 -m http.server`) on a run that had read any web content. Loopback traffic
  can't reach anywhere off the machine, so it's carved back out of the deny on both platforms;
  everything off-machine is still refused.

## [0.3.6] - 2026-07-07

### Fixed
- Picking a folder for a *new* project (e.g. "Desktop") no longer binds that exact folder to the
  project. It now creates a new subfolder named after the project inside the picked location -
  the same guarantee a project already got when no folder was picked at all - so a project named
  "Rado1" reliably gets a "Rado1" folder, and the agent is never accidentally scoped to an
  existing folder full of unrelated files.

## [0.3.5] - 2026-07-07

### Fixed
- A new project's folder is now provisioned at creation time instead of lazily on first
  terminal/files-drawer use, and the Settings › Workspace "+ New project" flow now switches to
  the project it just created. Previously, creating a project there and immediately opening the
  Folder drawer could land on nothing, or on whatever project was active before.

## [0.3.4] - 2026-07-06

### Fixed
- The files-drawer preview pane clipped instead of scrolling on files taller than the visible
  area (a grid-sizing bug: the preview grid's row had no height cap, so it grew to fit the
  content instead of the drawer).

### Added
- The file preview can now edit a file in place: an **Edit** button swaps the rendered view for
  a plain-text editor, **Save** (or ⌘S) writes it back through a new `POST /v1/fs/write` -
  gated by the same shell consent as the rest of the drawer and signed into the ledger as
  `file.write`. Switching files, navigating, or closing the drawer with unsaved edits prompts to
  discard first, and a failed save keeps the edit in the textarea instead of losing it.

## [0.3.3] - 2026-07-06

### Changed
- **The desktop terminal drawer is now two separate panels.** Browsing a project's files and
  running shell commands used to share one drawer (file tree + git panel + shell). They're now
  independent topbar buttons: **Folder** (⌘E) opens a real file browser with an inline preview
  pane — markdown renders, code is highlighted, images display, binaries and oversized files are
  handled gracefully — plus per-file Copy-path/Download and a "show hidden files" toggle.
  **Terminal** (⌘\`) is now a plain multi-tab shell. Each keeps its own state per project.

### Added
- `GET /v1/fs/read`: reads one file for the new preview pane (capped text preview) or, with
  `raw=1`, streams its bytes for images and downloads. `/v1/fs` gained a `hidden` toggle and
  real file sizes, and directory listings now follow symlinks so a linked folder browses like
  a normal one.

### Security
- Raw file serving mirrors `/v1/artifact`'s inline policy: only raster images render inline in
  the dashboard origin; HTML, SVG, and everything else are served as a download instead, so a
  file dropped into the workdir (by a repo clone or an agent) can never execute same-origin.

## [0.3.2] - 2026-07-06

### Added
- **A durable agent's own charter and self-model.** `AgentDef` gains a `charter` field (its
  role/system prompt, renamed from `role` — a non-colliding name for the per-agent identity
  register, distinct from persona/brief which shape voice rather than mandate) and an optional
  `home_project` (a default project scope for a non-coding agent — a content writer or
  researcher — with no working directory required; kanban-board task runs, which previously had
  no project scope at all, now resolve to the assigned agent's home project). Memory writes made
  through `memory_remember` during a named agent's run now attribute to that agent
  (`agent:<name>`) instead of the generic literal `"agent"`, which makes a genuinely new feature
  possible: each durable agent gets its own distilled, always-loaded self-model — what *that
  agent* has learned — kept separate from the shared global one and from every other agent's.
  New surface: `GET/POST /v1/agents/{id}/consciousness[/distill]`, `engram agents consciousness
  <id> [--distill]`, a TUI `c` keybinding on the Agents view, and a "Self-model" link in the
  desktop agent's track-record panel.
- **Scheduled jobs bind to a durable agent as a first-class field.** `Job.agent_id` replaces the
  old `payload.agent` convention (still read as a fallback for a job scheduled before this
  change), so "this job runs as this agent" is a documented contract instead of an emergent
  payload key — surfaced in `engram schedule add/edit --agent`, the TUI schedule form, and the
  desktop scheduler.

### Changed
- **Naming cleanup, closing two real collisions found in a grounded architecture review:**
  `Project.persona` is renamed to `Project.brief` (the daemon-wide SOUL.md persona and a
  project's own standing instructions used to share one field name, disambiguated only by which
  accessor happened to be called), and the CLI's `engram memory identity[-edit|-add|-remove
  |-revert]` verbs are renamed to `consciousness*` to match the API/route/struct layer, which had
  called this feature `Consciousness` all along. Both old names still work (`persona`/`identity`
  accepted as aliases) so no existing script or saved `workspace.json`/`agents.json` breaks.

## [0.3.1] - 2026-07-06

### Added
- **`engram model fetch`** — one command downloads a real, offline-capable-once-fetched
  (model2vec) embedding model and switches memory recall to it, taking hybrid recall from 88% to
  100% on the labeled benchmark (see `crates/engram-bench/BENCHMARKS.md`). User-initiated only (no
  automatic/silent network access — offline-by-default is preserved); a corrupt or partial download
  can never become "the active model" (verified before committing); requires a daemon restart, same
  as any other embedder-affecting setting.
- **Per-project terminal, always-on git status, and worktree management** in the desktop app.
  The terminal is no longer one shared shell falling back to a daemon-wide folder — it's a
  multi-tab drawer, opened from the topbar, scoped to whichever project is active (each project
  keeps its own tabs and file tree; one with no folder set is prompted to pick one instead of
  silently sharing another project's workspace). A topbar chip shows the active project's branch,
  ahead/behind, and dirty-file count on every view, with a panel to create, switch to, and remove
  per-project git worktrees.

## [0.3.0] - 2026-07-05

A full memory-system upgrade: truth over time (bi-temporal recall, confirmed contradiction
detection), long-task continuity (paged compaction, mission relay), grounded reflection, and a
real benchmark suite proving the recall-quality wins with numbers instead of prose.

### Added
- **Bi-temporal memory recall.** `engram memory recall --as-of <date>` (CLI, TUI keybinding,
  `GET /v1/recall?as_of=`) answers "what did I believe on this date" instead of only ever
  showing current-state facts. Additive to the existing `supersede()` path — one code path,
  not two.
- **Contradiction detection with mandatory confirmation.** Extends supersession beyond the
  old 3-rule, Identity-region-only literal-prefix whitelist to any region and any wording: a
  new fact that resembles an existing one gets a citation-verified model judgment on whether
  it contradicts it. Never applies automatically — always produces a `pending_supersessions`
  row for a human to accept or reject. New surface everywhere: `GET /v1/supersessions`,
  `POST /v1/supersessions/{id}/resolve`, `engram memory supersessions [--accept|--reject]`,
  a TUI panel (`Tab` to cycle to it, `a`/`x` to accept/reject), and a desktop "⇄ Pending"
  panel — closing what had been a CLI-only review workflow.
- **Grounded reflection (opt-in, default off — `security.auto_reflect`).** An hourly pass
  synthesizes a higher-level fact from a small, bounded, co-scoped group of related Trusted
  memories, citing exactly which facts it drew on (`GET /v1/memory/reflections`,
  `engram memory reflections`, a dedicated TUI/desktop panel). A reflection is never mixed
  with, or visually indistinguishable from, a directly-witnessed memory on any surface.
- **Agent attribution and per-scope stats.** Every memory records who wrote it (`user`,
  `core`, a skill, or a durable agent's own name); `GET /v1/memory/stats` and
  `engram memory stats` now break down by actor and can be scoped to one project.
- **The procedural-memory bridge.** Skill promotion and revert now write real
  `Region::Procedural` facts, so the brain's procedural-memory view finally shows real
  points instead of staying empty.
- **Long-task continuity.** Context compaction now pages the elided transcript out to memory
  instead of discarding it (a new `memory_recall_page` tool reads it back); completed plan
  steps leave durable episodic breadcrumbs; and a multi-run mission's prompt now also
  recalls earlier-hop breadcrumbs, not just the single immediately-prior answer.
- **A real, honest benchmark suite** (`crates/engram-bench/BENCHMARKS.md`): a three-arm
  keyword-only / semantic-only / hybrid recall-quality comparison, plus the multi-project
  scale benchmark, with real numbers and an honestly-reported non-obvious result rather than
  a smoothed-over win claim.
- **Skill self-improvement and consciousness editing, from every surface.**
  `engram skills improve/revert/activate/teach` and `engram memory identity-edit/add/remove/
  revert` (CLI); a matching TUI Skills-view score/provenance display and consciousness
  editor.
- **The cold tier now actually affects recall ranking** (a small penalty instead of being
  purely informational), and opt-in, conservative, reversible auto-pruning of already-
  superseded memories completes the sleep-cycle triad (consolidate → tier-penalize → prune).
- **Fixed: per-project persona was computed but never actually injected** into the live
  agentic chat path (`run_agent_task_cb`, the code behind the desktop/TUI chat, as opposed
  to the simpler `agent_handler`) — a project's persona now reliably shapes its conversations.

### Fixed
- **The README's prebuilt-binary link no longer goes stale.** It hardcoded a specific
  release tag in the download filename, so it always fetched v0.2.0 no matter how many
  versions had shipped since.
- **The default embedder now matches what's actually configured.** `Config::from_env` used
  to unconditionally overwrite the embedder kind even when `ENGRAM_EMBED` wasn't set,
  silently reverting a configured static/gateway embedder back to the old default on every
  boot. The default is now the real static (model2vec) embedder, with a clear
  `embedder_degraded` signal (surfaced in `/v1/memory/stats`) when no model is installed and
  it's quietly running on the trigram fallback instead.
- **Two persist-time taint holes closed**, so untrusted content can no longer slip into
  trusted recall via the legacy conversational path.

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

[Unreleased]: https://github.com/radotsvetkov/engram/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/radotsvetkov/engram/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/radotsvetkov/engram/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/radotsvetkov/engram/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/radotsvetkov/engram/releases/tag/v0.2.0
[0.1.0]: https://github.com/radotsvetkov/engram/releases/tag/v0.1.0
