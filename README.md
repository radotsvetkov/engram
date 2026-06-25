# Engram

**A personal AI agent with a brain you can watch grow — that costs nothing to run when idle.**

Engram is a self-improving personal AI agent modeled on how a brain actually works, then translated to a machine that costs almost nothing to run. It is a single static Rust binary that sleeps to zero RAM between requests and wakes in milliseconds — versus an always-on Python+Node multi-hundred-MB runtime chain on a perpetually billing VPS. Its memory is hybrid semantic+keyword and region-partitioned, so it recalls paraphrased facts a keyword-only store returns zero hits for. Its skills are not prompts but small self-improving programs that run in a default-deny WASM sandbox and rewrite themselves from measured outcomes. Every memory write and skill mutation is signed, append-only, and one-click reversible, and a dashboard renders the brain — firing neurons, memory tiers, skills, the model of you — so growth is felt and governed, not silently committed to a Markdown folder you are told to audit later.

Engram v0.1 is complete. Every step of the architecture build order is built and tested: the reactive kernel and signed audit ledger, hybrid memory, the metered taint-aware gateway, capability-sandboxed WASM skills with a self-improving learning loop, the deterministic scheduler, the daemon that wires it all into a running agent with a live dashboard, and the benchmark harness. The whole workspace builds offline with no network dependency and passes `cargo test --workspace`.

See [`VISION.md`](./VISION.md) for the north-star vision, [`docs/ADR-0001-architecture.md`](./docs/ADR-0001-architecture.md) for the architecture decision record that turns that vision into concrete calls, and [`docs/THREAT-MODEL.md`](./docs/THREAT-MODEL.md) for the security model.

## The brain, mapped to the machine

| Brain | Engram | Role |
|---|---|---|
| Working memory (prefrontal) | **Hot** store, in-RAM | the current task and context |
| Hippocampus | **Episodic** encoder + consolidator | recent experiences, conversations |
| Neocortex | **Semantic** store (warm→cold) | consolidated long-term knowledge |
| Basal ganglia | **Procedural** store | skills / habits — the self-improving programs |
| Amygdala | **Salience** tagging | what matters, what to keep, what to forget |
| Neurons / synapses | **Reactive event bus** | fire on events; synaptic weights = learning |
| Sleep / consolidation | **Idle consolidation** | move warm→cold, strengthen used traces, prune the rest |
| Self-model | **Identity model** | the deepening picture of *you* |

## Architecture

Engram is a Rust workspace of small, single-purpose crates. Capability comes from the right primitives and the right isolation boundaries, not from lines of code. Every crate that mutates state writes to the audit ledger first, so the whole system is tamper-evident and reversible by construction.

**`crates/engram-core` — the reactive kernel.** The brainstem, now a pure library (its old demo binary was removed; the daemon lives in `engramd`). It owns the primitives everything else fires through:

- **Event bus (`event`)** — the neural substrate. A *spike* is the unit of activity; spikes flow across four priority lanes (Reflex → High → Normal → Low) on an in-process Tokio broadcast bus that exists *only while the core is awake*. There is no parked daemon. Every spike carries a monotonic provenance taint: anything derived from untrusted input is `Untrusted`, and taint only ever spreads — the basis for breaking the prompt-injection → exfiltration chain.
- **Lifecycle (`lifecycle`)** — wake/sleep. The core runs while there is activity and resolves to exit after an idle window (or on SIGINT/SIGTERM). On a socket-activated VPS this means no resident process between requests — the near-zero-idle property in one small module. Tested under Tokio's paused virtual clock, so the idle behavior is verified without real sleeping.
- **Audit ledger (`ledger`)** — an append-only, content-addressed (BLAKE3), hash-chained, Ed25519-signed record of every state change. Each entry commits to the previous entry's hash, so altering any past entry breaks every hash after it and tampering is detected on replay. The signing key lives on disk at `0600` and is never handed to a skill. A "revert" is itself an appended entry pointing at a prior good hash — history is added to, never erased.

**`crates/engram-memory` — the brain on disk.** Hybrid, region-partitioned, tiered memory in a single embedded SQLite (WAL) file that survives the core sleeping to zero.

- **Memory broker (`store`)** — `remember` and `recall`. Recall fuses a **keyword** arm (FTS5 / BM25) and a **semantic** arm (brute-force vector cosine over candidate rows) with Reciprocal Rank Fusion, so a paraphrased query with no shared words still surfaces the right memory — exactly where a keyword-only agent returns nothing. Each hit reports which arm carried it, so the UI can show *why* a memory surfaced. Writes are ledgered before they land; `forget`/`restore` and idle `consolidate` (warm→cold demotion of stale, low-importance facts) are all recorded and reversible.
- **Regions (`region`)** — memory is partitioned the way a brain is (Episodic, Semantic, Identity, Procedural), and recall consults only the regions that fit the task type, so a question about *who you are* does not scan every conversation you ever had.
- **Embeddings (`embed`)** — an `Embedder` trait with a dependency-free default (signed feature hashing over word tokens and character trigrams, L2-normalized) that keeps the binary tiny and the pipeline testable offline. A real transformer embedding model plugs into the same trait through the gateway when present.

**`crates/engram-gateway` — the LLM gateway.** The single audited choke-point every model and embedding call passes through, so nothing reaches a model off the record. Provider-agnostic behind a `Provider` trait: an offline `MockProvider` runs everywhere with no credentials, and an `HttpProvider` (behind `--features http`, OpenAI-compatible, for Anthropic / OpenAI / OpenRouter) is opt-in so default builds stay small and offline. It meters tokens and cost per call, and enforces the first half of the taint rule — an untrusted call has its secret-bearing context stripped before it reaches the model, with the redaction metered and ledgered.

**`crates/engram-skills` — the skill runtime.** Skills are not prompts; they are small signed WASM programs that run in a capability-sandboxed, fuel-bounded host (via `wasmi`, a pure-Rust interpreter chosen for a tiny binary and a deterministic deny-by-default sandbox). A skill receives a host function only if its signed manifest was granted the matching capability; importing anything ungranted fails to link, so an over-reaching skill never starts, and a runaway skill traps on fuel exhaustion instead of hanging the core. A registry versions skills and their recorded runs. On top of it sits the **self-improving learning loop**: a candidate version is replayed against the inputs the skill has actually seen, scored on the skill's own metric, A/B-gated head-to-head against the incumbent, promoted only on a measured win with consent — and one `set_active` away from being reverted. **Egress capabilities (LLM, Net) are revoked automatically for any run that read untrusted input** — the no-egress half of the taint rule, proven at the sandbox boundary.

**`crates/engram-sched` — the scheduler.** Deterministic natural-language → recurrence parsing ("every weekday at 9am") with no model call, persisted jobs that reschedule forward across sleep with skip-on-missed so a suspended VPS does not stampede on wake, and generators for the systemd socket-activation and wake-timer units that make zero-idle and scheduled wake real on a $5 VPS. Every change is recorded in the audit ledger.

**`crates/engram-agent` — the tool-use loop.** This is where the model stops talking and starts *doing*. The agent advertises its tools to the model, runs the calls the model makes, feeds each observation back, and repeats until the model answers with no further tool call (or a step budget is hit). It is exposed at **`POST /v1/agent`** and driven from the dashboard's **Agent** panel; the same loop runs behind every messaging channel. Every step is ledgered, so a run is a replayable trace, not a black box.

The built-in tools are deliberately small and auditable:

- **`memory_recall` / `memory_remember`** — search and write the agent's own hybrid long-term memory (the same broker the rest of the system uses), so facts learned in one run survive into the next. Writes inherit the run's taint, so injected content cannot launder itself into a trusted fact.
- **`read_file` / `write_file` / `list_dir`** — filesystem access **confined to the workdir** by path normalization that rejects `..` escapes; writing is policy-gated.
- **`shell`** — run a command with three backends: **local** (`sh -c`), **Docker** (`docker run --network none` against a configured image — sandboxed code execution with the network cut), and **SSH** (run on a remote host). Off by default, and refused outright once the run is tainted.
- **`web_search` / `web_fetch`** — real web access with no API key, via DuckDuckGo's HTML endpoint and a plain fetch, returning readable text (default `web` feature).
- **`browser_read`** (headless `--dump-dom`, runs the page's JavaScript) and the interactive **`browser_open` / `browser_click` / `browser_type` / `browser_extract`** plus **`browser_screenshot`** — a persistent Chrome session driven over the Chrome DevTools Protocol (`--features browser-cdp`), for JS-heavy pages and multi-step flows a plain fetch can't reach.
- **`delegate_task`** — spawn an isolated subagent on a focused subtask and return its result; subagents inherit the parent's taint and are depth-bounded so recursion can't run away.
- **`vision_analyze`, `image_generate`, `text_to_speech`** — multimodal actions routed through the metered, audited gateway (look at a screenshot, generate a PNG, synthesize audio).
- **`send_message`** — post to a Slack/Discord/Mattermost-style incoming webhook; paired with a **Telegram** inbound channel (`ENGRAM_TELEGRAM_TOKEN`) that long-polls messages, runs the agent on each, and replies — one transport, the same agent behind it.
- **The MCP client** — connect to any Model Context Protocol server listed in `<ENGRAM_HOME>/mcp.json` (JSON-RPC 2.0 over a subprocess's stdio); each remote tool is wrapped as a native, ledgered Engram tool. Rather than hand-coding dozens of integrations, the agent borrows the whole MCP ecosystem, audited through the same ledger as everything else.

The security edges here are what a bolted-on tool loop cannot retrofit:

- **Every tool call is ledgered** — signed, hash-chained, replayable. The run is auditable by construction, not by a log you are asked to trust.
- **The filesystem is workdir-confined** and the **shell is off by default**; the dangerous capabilities are closed unless you open them.
- **The run is *tainted* the instant a web or browser tool reads untrusted content**, and taint only ever spreads. After that point the `shell` is refused and the model's secret-bearing context is stripped before the next call — the prompt-injection → exfiltration chain is broken at the boundary, not by a hoped-for prompt. The same taint flows into subagents and into any memory the run writes.

**`crates/engramd` — the daemon.** This is where the parts become an agent. It opens the ledger, the hybrid memory, the skill registry, the gateway, the scheduler, and the agent's toolset (built-ins plus any MCP servers), and exposes them over a small local HTTP API plus a dashboard. The dashboard ships live views: **Agent** (run a task and watch each tool step), **Talk**, **Live Cortex** (the audit stream as neurons firing on the bus), **Memory Atlas** (regions and tiers, browsable, with one-click forget), **Skills**, **Schedule**, and the **gateway meter**. Every request keeps the brain awake and fires a spike; after an idle window with no requests the process exits to zero, so on a socket-activated VPS there is nothing resident between uses.

**`crates/engram-bench` — the benchmark harness.** A reproducible paraphrase recall harness that writes a labelled fact/query set plus distractors into the real memory broker and reports recall@10, MRR, and the zero-lexical-overlap subset where a keyword index scores zero by construction.

## Benchmark results

Measured by `cargo run -p engram-bench` with the bundled offline trigram-hash embedder, over a 20-fact corpus and 12 paraphrase queries:

| Metric | Engram (hybrid) | Keyword-only baseline |
|---|---|---|
| Recall@10 (all queries) | **100%** (12/12) | — |
| MRR | **0.917** | — |
| Recall@10 on zero-lexical-overlap paraphrases | **100%** (5/5) | 0% (by construction) |
| Binary size (full agent) | **3.2 MB** (native; musl on the VPS) | hundreds of MB |
| Idle RAM | **0 MB** (socket-activated) | always-on process |

Five of the twelve queries share no content word with their target; a keyword index returns nothing for those, and hybrid recall recovers all five. Synonym-level paraphrase ("car" → "automobile") needs the transformer embedder, which plugs into the same `Embedder` trait via the gateway; this same harness is what measures it once that model is wired.

## vs Hermes

The thesis is that Engram now *matches* [Nous Hermes](https://github.com/nousresearch/hermes-agent) on the agentic surface — the tool-use loop, sandboxed code execution, files, web, an interactive browser, vision/image/speech, memory, MCP, subagents, messaging, and personality — while *exceeding* it on the things Hermes cannot retrofit: footprint, zero-idle cost, hybrid recall, security-by-construction, a signed audit ledger, and a measured learning loop. This section is deliberately honest about where Engram is still behind.

### Where Engram now matches Hermes

| Capability | Engram | Hermes |
|---|---|---|
| **Agentic tool-use loop** | model emits tool calls → execute → observe → repeat, at `POST /v1/agent` and the dashboard Agent panel | the same central loop and tool registry |
| **Sandboxed code execution** | `shell` over local / network-isolated Docker / SSH backends | code execution over its own backends |
| **File operations** | `read_file` / `write_file` / `list_dir`, workdir-confined | file read/write tools |
| **Web access** | `web_search` + `web_fetch`, real, no API key (DuckDuckGo) | web search and fetch |
| **Interactive browser** | `browser_open` / `click` / `type` / `extract` / `screenshot` over CDP, plus headless `browser_read` | browser automation |
| **Vision / image / speech** | `vision_analyze`, `image_generate`, `text_to_speech` via the gateway | multimodal tools |
| **Long-term memory** | `memory_recall` / `memory_remember` over hybrid, tiered store | a `MEMORY.md` knowledge file |
| **MCP** | connect any MCP server via `mcp.json`; tools join the registry | MCP client |
| **Subagents** | `delegate_task` spawns isolated, taint-inheriting, depth-bounded subagents | subagent delegation |
| **Messaging** | Telegram inbound + outbound webhook (`send_message`) | broad messaging integration |
| **Personality** | `SOUL.md` persona prepended to every run | a `SOUL.md`-style persona |

### Where Engram exceeds Hermes

| Dimension | Engram | Hermes |
|---|---|---|
| **Idle RAM** | 0 MB resident application memory at idle (socket-activated, no resident process) | always-on Python+Node VPS process, hundreds of MB |
| **Binary size** | single static binary, 3.2 MB native (musl on the VPS) | multi-hundred-MB Python+Node+ffmpeg+uv+ripgrep runtime chain |
| **Idle cost** | $0.00/idle-hour on the socket-activation path | $5/mo always-on VPS path |
| **Cold start** | core exec-to-first-byte in milliseconds (SQLite WAL open); WASM skill instantiation in the microsecond-to-low-ms range | unbenchmarked container-wake on the Modal/Daytona path |
| **Recall quality** | 100% recall@10 on a paraphrase test set, including a zero-lexical-overlap subset where query tokens do not overlap stored text | keyword-only FTS5 returns 0 on that subset by construction |
| **Always-on memory** | no hard fact cap (importance-scored, consolidation-compacted), surviving the core sleeping to zero | ~2200-char (~800-token) `MEMORY.md` ceiling |
| **Security by construction** | filesystem workdir-confined; shell off by default; the run is tainted the instant a web/browser tool reads untrusted content, after which the shell and secret context are revoked (injection guard) | tool calls run without a taint/egress boundary |
| **Signed audit ledger** | 100% of memory writes, skill mutations, and tool calls signed, hash-chained, and one-click reversible | mutable Markdown/SQLite with manual review |
| **Self-modifying-skill safety** | 100% of skill executions sandboxed (WASM, deny-by-default, fuel-bounded), egress revoked on untrusted-data runs | 0% validation on the live in-agent skill-patch path |
| **Learning loop** | candidate skill versions replayed, A/B-gated head-to-head, promoted only on a measured win, reversible | no measured self-improvement gate |

### Where Engram does *not* yet match Hermes

These are honest gaps, not spin:

- **Voice mode** — not built. Engram does text-to-speech as a tool, but there is no live voice-conversation loop.
- **Breadth of integrations** — Hermes ships 20+ messaging platforms and ~6 execution backends. Engram has **Telegram + outbound webhook** for messaging and **local / Docker / SSH** shell backends plus the **WASM** skill sandbox. The MCP client narrows this gap (any MCP server becomes available) but the out-of-the-box surface is smaller.
- **Live media and real recall need a provider key.** `vision_analyze`, `image_generate`, `text_to_speech`, and synonym-level (>0.85) semantic recall route through a real model: they require building with `--features http` and configuring a provider, and fall back to the offline mock otherwise. The default offline build still does the agentic loop, files, shell, web, the interactive browser, and morphological recall.

## Build & run

Requires a stable Rust toolchain (see `rust-toolchain.toml`). The release profile is tuned for a small, fast-to-load binary (`opt-level = "z"`, thin LTO, `codegen-units = 1`, `panic = "abort"`, stripped). Everything builds and tests offline; the network LLM provider (`--features http`) and the interactive CDP browser (`--features browser-cdp`) are opt-in, while web search/fetch are on by default.

```sh
# Build the whole workspace, optimized.
cargo build --release

# Run the full test suite: kernel (bus, lifecycle, ledger), hybrid recall and
# consolidation, gateway metering and taint, WASM sandbox and learning loop,
# recurrence parsing and scheduling, systemd unit generation.
cargo test --workspace
```

### Running the agent

`engramd` boots the bus, opens the signed audit ledger, hybrid memory, skill registry, gateway, and scheduler, and serves a small HTTP API plus the dashboard. Every request keeps the brain awake; after the idle window it exits to zero and verifies the ledger chain on the way out.

```sh
# Run the built daemon, then open the dashboard.
./target/release/engramd
# → http://127.0.0.1:8088
```

It is configured by environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `ENGRAM_HOME` | `./brain` | Brain state directory: SQLite memory, ledger, and signing keys. |
| `ENGRAM_ADDR` | `127.0.0.1:8088` | Address the HTTP API and dashboard bind to. |
| `ENGRAM_IDLE_SECS` | `900` | Idle window, in seconds, before the core sleeps to zero. |
| `ENGRAM_EMBED` | _(unset)_ | Set to `gateway` to embed memory through the gateway model instead of the offline trigram embedder (switching needs a fresh `ENGRAM_HOME`). |
| `ENGRAM_LLM_BASE_URL` | _(unset)_ | OpenAI-compatible base URL for a real provider (requires building with `--features http`). |
| `ENGRAM_LLM_API_KEY` | _(unset)_ | API key for that provider. With both set, the gateway uses the real model for completions and embeddings; otherwise an offline mock. |
| `ENGRAM_MODEL` | `claude-haiku` | Model id the agent uses for the tool-use loop and delegated subagents. |
| `ENGRAM_VISION_MODEL` | _(`ENGRAM_MODEL`)_ | Override model for the `vision_analyze` tool, if the vision model differs. |
| `ENGRAM_TOOLS_SHELL` | _(unset)_ | Set to `1` to enable the `shell` tool. Off by default; always refused once a run is tainted. |
| `ENGRAM_SHELL_BACKEND` | _(local)_ | `docker` runs shell commands in a network-isolated container; `ssh` runs them on a remote host. Unset runs locally. |
| `ENGRAM_DOCKER_IMAGE` | `alpine` | Image used by the `docker` shell backend (`docker run --network none`). |
| `ENGRAM_SSH_HOST` | _(unset)_ | `user@host` for the `ssh` shell backend. |
| `ENGRAM_WORKDIR` | `<ENGRAM_HOME>/work` | Directory the agent's filesystem tools are confined to. |
| `ENGRAM_WEBHOOK_URL` | _(unset)_ | Default destination for the `send_message` tool (Slack / Discord / Mattermost-style incoming webhook). |
| `ENGRAM_TELEGRAM_TOKEN` | _(unset)_ | Bot token; when set, Engram runs a Telegram inbound channel that runs the agent on each message and replies. |
| `ENGRAM_CHROME` | _(autodetected)_ | Path to a Chrome/Chromium binary for the browser tools, if not on a standard path. |
| `ENGRAM_CDP_PORT` | `9222` | Debugging port for the interactive CDP browser (`--features browser-cdp`). |
| `RUST_LOG` | `info` | Tracing filter, e.g. `debug` or `engram_core=trace`. |

The agent toolset is shaped by Cargo feature flags. The `web` feature (web search/fetch, outbound webhook) is **on by default**; `--features http` enables the real LLM/embedding provider behind vision, image generation, speech, and synonym-level recall; and `--features browser-cdp` compiles the interactive Chrome-DevTools-Protocol browser. The default offline build still runs the full tool-use loop, files, shell, web, headless browser, and morphological recall.

```sh
# Sleep after 30s idle, keep brain state in /tmp/engram, log everything.
ENGRAM_IDLE_SECS=30 ENGRAM_HOME=/tmp/engram RUST_LOG=debug \
  ./target/release/engramd
```

The dashboard includes **Agent** (give it a task and watch each tool step it takes),
**Talk** (a conversation that writes to episodic memory, recalls past turns, and learns
identity facts about you), **Memory Atlas**, **Skills** (run the seeded `shout` and `ask`
skills — `ask` calls the model through the gateway from inside the sandbox), **Schedule**,
**Live Cortex** (the audit stream), and the gateway meter.

To exercise the agent with real tools, enable the optional features and (for live media)
a provider:

```sh
# Build with the interactive browser and a real network LLM provider.
cargo build --release --features browser-cdp,http

# Enable the shell, route it through a network-isolated container, and run.
ENGRAM_TOOLS_SHELL=1 ENGRAM_SHELL_BACKEND=docker \
ENGRAM_LLM_BASE_URL=https://api.example.com/v1 ENGRAM_LLM_API_KEY=… \
  ./target/release/engramd
```

### Desktop app

A native Tauri shell that wraps the dashboard and starts the daemon for you:

```sh
cd desktop/src-tauri && cargo tauri dev    # needs: cargo install tauri-cli --version '^2'
```

See [`desktop/README.md`](./desktop/README.md).

### Running the benchmark

```sh
# Paraphrase recall@10, MRR, the zero-overlap subset, and the binary footprint.
cargo run -p engram-bench
```

The `brain/` state directory holds your personal memory and the signing keys; it is gitignored and must never be committed.

## Status

Every step of the v0.1 architecture build order is built and tested.

| Phase | Component | Status |
|---|---|---|
| 1 | Workspace + release profile (small static binary) | **Done** |
| 2 | `engram-core`: event bus, wake/sleep lifecycle | **Done** |
| 3 | Append-only, content-addressed, signed audit ledger | **Done** |
| 4 | `engram-memory`: SQLite + FTS5 + hybrid recall + regions + consolidation | **Done** |
| 5 | `engram-gateway`: provider-agnostic LLM + embeddings, metering, taint redaction | **Done** |
| 6 | `engram-skills`: capability-sandboxed signed WASM host, registry | **Done** |
| 7 | Taint rule: egress revoked on untrusted-data runs at the sandbox boundary | **Done** |
| 8 | Learning loop: replay eval, A/B promotion gate, signed versions, revert | **Done** |
| 9 | `engram-sched`: NL→recurrence, persisted jobs, systemd socket + timer units | **Done** |
| 10 | `engramd`: the daemon — HTTP API + dashboard (Agent, Talk, Live Cortex, Memory Atlas, Skills, Schedule, meter) | **Done** |
| 11 | `engram-bench`: paraphrase recall harness | **Done** |
| 12 | `engram-agent`: tool-use loop, built-in tools (memory, files, shell, web, browser, media, delegate, messaging), MCP client, taint guard | **Done** |

## What's next

The engine is proven end to end offline, and the integration points for going online are
in place. Delivered since the initial v0.1:

- **The agentic layer** — `engram-agent`: the tool-use loop at `/v1/agent` and the
  dashboard Agent panel, with built-in tools for memory, files, the multi-backend shell,
  web, the interactive browser, vision/image/speech, subagent delegation, and messaging,
  plus an MCP client — all under the workdir confinement and taint guard described above.
- **Conversation memory** — `/v1/converse` and the Talk panel: each turn is logged to
  episodic memory, past turns are recalled, and identity facts about you are extracted
  and persisted across sessions.
- **Async LLM/Net host capabilities for skills** — a granted, untainted skill calls the
  model through the metered, audited gateway from inside the sandbox (seed `ask` skill).
- **Real model + embedder wiring** — `ENGRAM_EMBED=gateway` plus `--features http` and a
  provider URL/key route completions and embeddings through a real OpenAI-compatible model.
- **Swarms** — `/v1/swarm` composes multiple skills into a pipeline over shared input.
- **Tauri desktop shell** — `desktop/` wraps the dashboard in a native window.

Remaining:

- **Provider key** — with a real embedding model configured, synonym-level paraphrase
  recall (>0.85) on top of today's morphological recall; the benchmark harness measures it.
  The same key lights up live vision, image generation, and speech in the agent.
- **VPS deploy** — the generated systemd socket-activation and wake-timer units on a $5 VPS
  behind a reverse proxy, with the published $0.00/idle-hour table.
- **Voice mode** and **broader messaging/execution breadth** — the honest gaps against
  Hermes noted above; the MCP client narrows the integration gap in the meantime.

## Design principles

- **Less is more.** The smallest design that delivers the vision wins. Capability comes from architecture and the right primitives, not from lines of code.
- **Transparent and auditable.** Every memory write, skill mutation, tool call, and autonomous action is logged, attributable, and reversible. The audit ledger is signed and hash-chained, so you can replay it and prove nothing was rewritten. You can watch the brain think.
- **Near-zero idle.** A Rust core means a single small binary, tiny resident memory, and instant wake. It sleeps to nothing on a $5 VPS or a serverless trigger and wakes on an event. You should be able to forget it is running.
- **Skills are programs, not prompts.** A skill is executable, versioned, sandboxed, and self-improving: it measures its own success and rewrites itself toward it — under an explicit capability model, with every mutation signed and reversible.

## License

MIT. Copyright (c) 2026 Radoslav Tsvetkov. See [`LICENSE`](./LICENSE).

Authored by Radoslav Tsvetkov.
