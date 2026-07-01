<p align="center"><img src="assets/brand/icon.svg" width="84" alt="Engram" /></p>

<h1 align="center">Engram</h1>

**A personal AI agent with a brain you can watch grow, that costs almost nothing to run while it sits idle.**

Engram is a personal AI agent built loosely on how a brain works: it keeps what matters, forgets the rest, and gets better the more you use it. It runs as a single small Rust binary that drops to zero RAM when nothing is happening and wakes in a few milliseconds, so you are not paying for an always-on Python and Node stack idling on a VPS all month.

Its memory searches by meaning and by keyword at the same time, so a paraphrased question still finds the right fact even when it shares no words with how you first wrote it. Skills are not prompts. They are small programs that run in a locked-down WASM sandbox and rewrite themselves based on how well they actually performed. Anything the agent does to its own memory or skills is signed, append-only, and reversible in one click, and the dashboard shows it as it happens rather than quietly writing to a folder you are told to review later.

v0.1 is finished and tested: the reactive core and the signed ledger, hybrid memory, the metered gateway, the sandboxed skills and their learning loop, the scheduler, the daemon that ties it all together, and the benchmarks. It all builds offline with no network, and passes `cargo test --workspace`.

See [`VISION.md`](./VISION.md) for the north-star vision, [`docs/ADR-0001-architecture.md`](./docs/ADR-0001-architecture.md) for the architecture decision record that turns that vision into concrete calls, and [`docs/THREAT-MODEL.md`](./docs/THREAT-MODEL.md) for the security model.

## The brain, mapped to the machine

| Brain | Engram | Role |
|---|---|---|
| Working memory (prefrontal) | **Hot** store, in-RAM | the current task and context |
| Hippocampus | **Episodic** encoder + consolidator | recent experiences, conversations |
| Neocortex | **Semantic** store (warm→cold) | consolidated long-term knowledge |
| Basal ganglia | **Procedural** store | skills / habits, the self-improving programs |
| Amygdala | **Salience** tagging | what matters, what to keep, what to forget |
| Neurons / synapses | **Reactive event bus** | fire on events; synaptic weights = learning |
| Sleep / consolidation | **Idle consolidation** | move warm→cold, strengthen used traces, prune the rest |
| Self-model | **Identity model** | the deepening picture of *you* |

## Architecture

Engram is a Rust workspace of small, single-purpose crates. Capability comes from the right primitives and the right isolation boundaries, not from lines of code. Every crate that mutates state writes to the audit ledger first, so the whole system is tamper-evident and reversible by construction.

**`crates/engram-core`, the reactive kernel.** The brainstem, now a pure library (its old demo binary was removed; the daemon lives in `engramd`). It owns the primitives everything else fires through:

- **Event bus (`event`)**: the neural substrate. A *spike* is the unit of activity; spikes flow across four priority lanes (Reflex → High → Normal → Low) on an in-process Tokio broadcast bus that exists *only while the core is awake*. There is no parked daemon. Every spike carries a monotonic provenance taint: anything derived from untrusted input is `Untrusted`, and taint only ever spreads, the basis for breaking the prompt-injection → exfiltration chain.
- **Lifecycle (`lifecycle`)**: wake/sleep. The core runs while there is activity and resolves to exit after an idle window (or on SIGINT/SIGTERM). On a socket-activated VPS this means no resident process between requests, the near-zero-idle property in one small module. Tested under Tokio's paused virtual clock, so the idle behavior is verified without real sleeping.
- **Audit ledger (`ledger`)**: an append-only, content-addressed (BLAKE3), hash-chained, Ed25519-signed record of every state change. Each entry commits to the previous entry's hash, so altering any past entry breaks every hash after it and tampering is detected on replay. The signing key lives on disk at `0600` and is never handed to a skill. A "revert" is itself an appended entry pointing at a prior good hash, history is added to, never erased. The public key is published to `<ENGRAM_HOME>/ledger.pub`, and **`engramd verify [HOME]`** replays the chain against it **offline, without starting or trusting the daemon** (exit 0 = intact, 1 = tampered at a named seq). A run can be exported as a self-contained, independently-verifiable receipt via `GET /v1/tasks/{id}/receipt`. *Honest boundary:* this is tamper-evident against post-hoc edits and any party without the signing key, **not** against a fully-compromised host that holds the key; hardware-backed keys / external co-signing (deferred) are what would make it tamper-*proof* against host compromise. See [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) (T8).

**`crates/engram-memory`, the brain on disk.** Hybrid, region-partitioned, tiered memory in a single embedded SQLite (WAL) file that survives the core sleeping to zero.

- **Memory broker (`store`)**: `remember` and `recall`. Recall fuses a **keyword** arm (FTS5 / BM25) and a **semantic** arm (brute-force vector cosine over candidate rows) with Reciprocal Rank Fusion, so a paraphrased query with no shared words still surfaces the right memory, exactly where a keyword-only agent returns nothing. Each hit reports which arm carried it, so the UI can show *why* a memory surfaced. Writes are ledgered before they land; `forget`/`restore` and idle `consolidate` (warm→cold demotion of stale, low-importance facts) are all recorded and reversible.
- **Regions (`region`)**: memory is partitioned the way a brain is (Episodic, Semantic, Identity, Procedural), and recall consults only the regions that fit the task type, so a question about *who you are* does not scan every conversation you ever had.
- **Embeddings (`embed` / `static_embed`)**: an `Embedder` trait with a dependency-free default (signed feature hashing over word tokens and character trigrams, L2-normalized) that keeps the binary tiny and the pipeline testable offline, plus a **pure-Rust static (model2vec) embedder** for real synonym/paraphrase recall (`ENGRAM_EMBED=static`), a distilled embedding table read straight from `model.safetensors`, no ONNX runtime, no ML crate. A provider embedding model also plugs into the same trait through the gateway. Changing embedders re-embeds existing memories into the new space.

**`crates/engram-gateway`, the LLM gateway.** The single audited choke-point every model and embedding call passes through, so nothing reaches a model off the record. Provider-agnostic behind a `Provider` trait: an offline `MockProvider` runs everywhere with no credentials, and an `HttpProvider` (behind `--features http`) is opt-in so default builds stay small and offline. Anthropic uses its native Messages transport; every other backend is OpenAI-compatible and shares one code path - OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Together, xAI, Perplexity, Google's Gemini endpoint, and local Ollama / LM Studio / vLLM / llama.cpp - each with a built-in default endpoint so picking one Just Works once you add a key. It meters tokens and cost per call, and enforces the first half of the taint rule, an untrusted call has its secret-bearing context stripped before it reaches the model, with the redaction metered and ledgered.

**`crates/engram-skills`, the skill runtime.** Skills are not prompts; they are small signed WASM programs that run in a capability-sandboxed, fuel-bounded host (via `wasmi`, a pure-Rust interpreter chosen for a tiny binary and a deterministic deny-by-default sandbox). A skill receives a host function only if its signed manifest was granted the matching capability; importing anything ungranted fails to link, so an over-reaching skill never starts, and a runaway skill traps on fuel exhaustion instead of hanging the core. A registry versions skills and their recorded runs. On top of it sits the **self-improving learning loop**: a candidate version is replayed against the inputs the skill has actually seen, scored on the skill's own metric, A/B-gated head-to-head against the incumbent, promoted only on a measured win with consent, and one `set_active` away from being reverted. **Egress capabilities (LLM, Net) are revoked automatically for any run that read untrusted input**: the no-egress half of the taint rule, proven at the sandbox boundary.

**`crates/engram-sched`, the scheduler.** Deterministic natural-language → recurrence parsing ("every weekday at 9am") with no model call, persisted jobs that reschedule forward across sleep with skip-on-missed so a suspended VPS does not stampede on wake, and generators for the systemd socket-activation and wake-timer units that make zero-idle and scheduled wake real on a $5 VPS. Every change is recorded in the audit ledger.

**`crates/engram-agent`, the tool-use loop.** This is where the model stops talking and starts *doing*. The agent advertises its tools to the model, runs the calls the model makes, feeds each observation back, and repeats until the model answers with no further tool call (or a step budget is hit). It is exposed at **`POST /v1/agent`** and driven from the dashboard's **Agent** panel; the same loop runs behind every messaging channel. Every step is ledgered, so a run is a replayable trace, not a black box. The loop is frontier-grade: a turn's independent tool calls run **in parallel**, model calls **retry with backoff**, the transcript is **compacted** (older turns summarized) so long runs don't overflow the context window, the agent maintains an explicit **plan** (`update_plan`, shown as a live checklist) and runs a **verify-before-finish reflection** pass, all while the no-egress taint gate holds. With a native Anthropic key it adds **prompt caching** of the tools+system prefix and **token streaming**.

The built-in tools are deliberately small and auditable:

- **`memory_recall` / `memory_remember`**: search and write the agent's own hybrid long-term memory (the same broker the rest of the system uses), so facts learned in one run survive into the next. Writes inherit the run's taint, so injected content cannot launder itself into a trusted fact.
- **`read_file` / `write_file` / `list_dir`**: filesystem access **confined to the workdir** by path normalization that rejects `..` escapes; writing is policy-gated.
- **`shell`**: run a command with three backends: **local** (`sh -c`), **Docker** (`docker run --network none` against a configured image, sandboxed code execution with the network cut), and **SSH** (run on a remote host). Off by default, and refused outright once the run is tainted.
- **`web_search` / `web_fetch`**: real web access with no API key, via DuckDuckGo's HTML endpoint and a plain fetch, returning readable text (default `web` feature).
- **`browser_read`** (headless `--dump-dom`, runs the page's JavaScript) and the interactive **`browser_open` / `browser_click` / `browser_type` / `browser_extract`** plus **`browser_screenshot`**: a persistent Chrome session driven over the Chrome DevTools Protocol (`--features browser-cdp`), for JS-heavy pages and multi-step flows a plain fetch can't reach.
- **`delegate_task`**: spawn an isolated subagent on a focused subtask and return its result; subagents inherit the parent's taint and are depth-bounded so recursion can't run away.
- **`vision_analyze`, `image_generate`, `text_to_speech`**: multimodal actions routed through the metered, audited gateway (look at a screenshot, generate a PNG, synthesize audio).
- **`send_message`**: post to a Slack/Discord/Mattermost-style incoming webhook; paired with a **Telegram** inbound channel (`ENGRAM_TELEGRAM_TOKEN`) that long-polls messages, runs the agent on each, and replies, one transport, the same agent behind it.
- **The MCP client**: connect to any Model Context Protocol server listed in `<ENGRAM_HOME>/mcp.json` (JSON-RPC 2.0 over a subprocess's stdio); each remote tool is wrapped as a native, ledgered Engram tool. Rather than hand-coding dozens of integrations, the agent borrows the whole MCP ecosystem, audited through the same ledger as everything else.

The security edges here are what a bolted-on tool loop cannot retrofit:

- **Every tool call is ledgered**: signed, hash-chained, replayable. The run is auditable by construction, not by a log you are asked to trust.
- **The filesystem is workdir-confined** and the **shell is off by default**; the dangerous capabilities are closed unless you open them.
- **The run is *tainted* the instant a web or browser tool reads untrusted content**, and taint only ever spreads. After that point the `shell` is refused and the model's secret-bearing context is stripped before the next call. **Egress** (web_fetch/web_search, send_message, browser navigation, untrusted MCP) is refused once the run is *also* sensitive - it has read the user's private data (a `memory_recall`, a `read_file`, an authenticated MCP) - the full lethal trifecta. Gating egress on the conjunction is deliberate: pure web research keeps working, while a run holding something worth stealing can't carry it out. The prompt-injection → exfiltration chain is broken at the boundary, not by a hoped-for prompt; the SSRF guard additionally pins each connection to the IP it validated and re-checks every redirect hop, so a public URL can't rebind or 302 to a metadata address. The same taint flows into subagents and into any memory the run writes.

**`crates/engramd`, the daemon.** This is where the parts become an agent. It opens the ledger, the hybrid memory, the skill registry, the gateway, the scheduler, and the agent's toolset (built-ins plus any MCP servers), and exposes them over a small local HTTP API plus the redesigned desktop control center (see [Desktop](#desktop)). That single page gives you a Kanban board fed by a chat composer, glass-box signed task cards, an ambient trust/cost spine, and views for **Chat / Tasks / Schedule / Memory / Skills**: all over Server-Sent Events. Every request keeps the brain awake and fires a spike; after an idle window with no requests the process exits to zero, so on a socket-activated VPS there is nothing resident between uses.

**`crates/engram-bench`, the benchmark harness.** A reproducible paraphrase recall harness that writes a labelled fact/query set plus distractors into the real memory broker and reports recall@10, MRR, and the zero-lexical-overlap subset where a keyword index scores zero by construction.

**`crates/engram-eval`, deterministic harness regression testing.** Answers "tested by vibes": an eval *case* records a task plus the exact model completions a run received, and replaying it drives the *real* agent and *real* tools through the scripted provider, no model, no network, fully deterministic, asserting the tool sequence, answer, and stop reason against a baseline. Change a prompt, a tool, or the loop, re-run `engram-eval`, and a regression is a failing case, not a hunch. `engram-eval` runs the built-in suite (tool-use, planning, the token-budget stop, the loop guard); `engram-eval <dir>` runs every `*.json` case in a directory.

## Desktop

The dashboard is now a redesigned single-page **control center**: one self-contained `index.html` (HTML + CSS + vanilla JS, no build step, no framework) served at `/` by `engramd` (`crates/engramd/assets/index.html`). It is a calm, dark, Claude-like window: a left rail (**Chat / Tasks / Schedule / Memory / Skills**) and one work surface, with an ambient **trust spine** in the top bar that answers the two questions other agent UIs leave open, *is this safe?* and *what's it costing?*. A live **"ledger verified · N"** chip (flipping to a red tamper banner the moment the audit chain fails to verify) sits next to a **"today's cost"** chip, so the agent's integrity and spend are always in view, not buried in a separate admin tool.

- **A Kanban board at the heart.** Three columns, **To do / Running / Done**: with a **chat composer as the only input**, and intent routing on a single keystroke: **Enter** answers in Chat, **⌘+Enter** creates a task, **⇧+⌘+Enter** creates *and runs* it. Dragging a card between columns runs, cancels, or re-queues it. A running card shows live **"step N · tool"** progress as the agent works.
- **Glass-box task cards.** Clicking a card opens a detail panel with the agent's answer *and* the signed ledger audit slice for that run, each tool step paired with its ledger sequence number and BLAKE3 hash (click to copy), plus the pinned ledger head. It is a tamper-evident receipt, not just a log: the card proves what the agent did.
- **Graduated, calm autonomy.** Read-only steps run silently. When a side-effecting tool is blocked, the `shell`, off by default, the panel surfaces a plain-language **"Allow & re-run"** approval card instead of a stack trace. Granting it flips a runtime policy that is itself written to the ledger, so even a consent change is on the record.
- **Visible scheduling.** A natural-language **"when"** field with a **live next-fire preview** as you type (no model call), plus an in-process scheduler tick that fires due jobs as ordinary board cards. (For true zero-idle wake while the daemon is asleep, the generated systemd timers do the waking.)
- **Honest offline mode.** With no model key configured, the UI shows an explicit *"add a model key to think for real"* banner rather than returning fake answers.
- **Live and persistent.** Updates stream over **Server-Sent Events** (the connection is deliberately time-bounded so a held-open stream can never block the daemon's zero-idle exit; the browser reconnects seamlessly). Chat **persists across reloads** from episodic memory, every view is **deep-linkable via `#hash`**, and a **Memory** view renders the brain's regions (Identity / Semantic / Episodic / Procedural) with their warm/cold tiers.

The desktop sits on a small task model (`crates/engramd/src/tasks.rs`): a `Task` moves `todo → doing → done | failed | scheduled`, and a completed run carries a `TaskRun` receipt, the answer, every step verbatim, token and cost deltas, and the signed ledger head pinned at finish. The native [Tauri shell](#desktop-app) wraps exactly this page in a window.

The control center is driven by these endpoints, alongside the existing `/v1/agent`, `/v1/converse` (and `/v1/converse/stream` for token-by-token replies), `/v1/swarm`, `/v1/skills`, `/v1/remember` · `/v1/recall` · `/v1/forget`, `/v1/memory/stats`, `/v1/meter`, `/v1/ledger/tail` · `/v1/ledger/verify`, and `/v1/schedule`:

| Endpoint | Method | Purpose |
|---|---|---|
| `/v1/tasks` | GET / POST | list the board / create a task |
| `/v1/tasks/{id}` | PATCH / DELETE | move a card between columns / delete it |
| `/v1/tasks/{id}/run` | POST | run the task with the agent and attach a `TaskRun` receipt |
| `/v1/tasks/{id}/audit` | GET | the signed ledger slice for that run, the glass-box receipt |
| `/v1/policy` | GET / POST | read or set the runtime shell consent (the "Allow & re-run" toggle), itself ledgered |
| `/v1/schedule/preview` | GET | parse a natural-language "when" and return the next fire, without creating a job |
| `/v1/memory/recent` | GET | recent records by region, backs persistent chat and the Memory view |
| `/v1/events` | GET (SSE) | the live event stream the board updates from (bounded to protect zero-idle) |
| `/v1/voice` | POST | one voice turn: audio in → transcribe → run → synthesize → audio out |
| `/v1/voice/stream` | GET (WebSocket) | a multi-turn live voice session |
| `/v1/channel/{platform}` | POST | an inbound messaging-channel webhook |

## Terminal (CLI & TUI)

The same control surface, keyboard-first, in your terminal. `crates/engram-cli` is a
single small binary (`engram`) that talks to the daemon over the very same HTTP API
the desktop uses — it shares no process and stores nothing, so the daemon stays the
one audited choke-point. Run `engram` with no arguments for a full-screen TUI, or use
a subcommand for scripting; if the daemon isn't up, the client starts it and waits.

- **A full-screen TUI** built on `ratatui`: a streaming chat pane (tool steps and the
  model's narration stream live, answers render as real Markdown with a region-tinted
  **recall ribbon**), a three-column **kanban** with glass-box receipt cards, and views
  for **Memory / Skills / Schedule / Autonomy / Ledger / Agents** — all behind a
  `Ctrl-P` command palette. The same **trust spine** rides the header: model, today's
  cost, and a live `✓ ledger N` chip that flips to `✗ TAMPER` if the chain ever fails to
  verify. Staged egress waits for your approval in the **Autonomy** view (`a`/`d`), the
  graduated-autonomy gate in the terminal.
- **A scriptable CLI** with `--json` on every command: `engram ask` / `run` (streaming),
  `tasks`, `memory recall|remember|forget`, `skills`, `schedule`, `autonomy`,
  `ledger verify`, `config`, plus `engram status` / `doctor` and shell `completions`.
  `engram ledger verify` exits non-zero on tamper, so it drops straight into CI or a
  health check.

The renderer is shared between both surfaces, so `engram ask "…"` in a pipe looks
identical to the TUI's chat pane. See [`docs/CLI.md`](./docs/CLI.md) for the full
command and key reference.

## Benchmark results

Measured by `cargo run -p engram-bench` over a 25-fact corpus and 17 paraphrase queries (10 of them sharing no content word with their target). The harness compares the zero-dependency offline default against the pure-Rust static (model2vec) embedder:

| Embedder | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |
|---|---|---|---|
| trigram-hash (offline default) | 94% | 0.779 | 90% |
| **static model2vec (pure-Rust)** | **100%** | **0.887** | **100%** |
| keyword-only baseline |, |, | 0% (by construction) |

The static embedder closes the synonym gap the trigram default can't, *"purchasing a car"* recalls *"she bought a new automobile last week"* (no shared word or character-trigram). It needs no ONNX runtime or ML crate: inference is tokenize → look up each token's row in the distilled `[vocab, dim]` matrix → mean → normalize, all in pure Rust. The full-agent binary stays **5.6 MB** and idle RAM stays **0 MB**; the ~30 MB model is a data directory fetched at deploy time (`scripts/build_embedder.py`), never bundled. Enable with `ENGRAM_EMBED=static`; existing memories are re-embedded into the new space automatically.

## vs Hermes

The thesis is that Engram now *matches* [Nous Hermes](https://github.com/nousresearch/hermes-agent) on the agentic surface, the tool-use loop, sandboxed code execution, files, web, an interactive browser, vision/image/speech, memory, MCP, subagents, messaging, and personality, while *exceeding* it on the things Hermes cannot retrofit: footprint, zero-idle cost, hybrid recall, security-by-construction, a signed audit ledger, and a measured learning loop. This section is deliberately honest about where Engram is still behind.

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
| **Document ingest** | chat uploads of PDF / DOCX / XLSX / ODS / CSV are text-extracted server-side (capped) so the agent reads the actual content, not a placeholder | attachments are not first-class |
| **Per-agent model & provider** | each durable agent can carry its own model AND provider/base-url/key, so a team mixes a cheap triage model with a frontier reasoning model; the key is masked in the API and stored 0600 like the signing key | single global model |
| **Parallel-safe worktrees** | with `ENGRAM_WORKTREES=1` on a git workspace, each task runs in its own detached `git worktree`, cleaned up on exit, so several agents work one project without clobbering | shared working dir |
| **Integrations** | a one-click MCP gallery (Filesystem, Fetch, Git, GitHub, GitLab, Slack, Postgres, Notion, Brave, ...) with a per-server secret / working-dir / trust editor; secrets masked in the API and 0600 on disk | a fixed integration list, no MCP client |
| **Control surface** | one coherent desktop control center: a Kanban board fed by a single chat composer, glass-box task cards carrying the signed ledger slice for their run, and an ambient trust/cost spine ("ledger verified · N", today's cost) in the top bar | a split CLI + chat-app + admin-dashboard, with opaque cost |

### Where Engram does *not* yet match Hermes

These are honest gaps, not spin:

- **Voice mode**: not built. Engram does text-to-speech as a tool, but there is no live voice-conversation loop.
- **Breadth of messaging platforms**: Hermes ships 20+ messaging platforms. Engram has **Telegram + a generic inbound/outbound webhook** (which already covers Make / Zapier / Typeform / Tally and any platform that can POST). For tools, the **MCP gallery** turns any MCP server into a first-class integration (GitHub, GitLab, Slack, Postgres, Notion, Brave, filesystem, git, fetch, ...) with a per-server secret editor, so the *capability* surface is broad; the out-of-the-box *chat-platform* count is still smaller.
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

## Settings

The desktop app has a **Settings** panel (the gear in the sidebar) for the things you change most: the model and provider - Anthropic (native), or any OpenAI-compatible backend with a one-click preset and built-in endpoint (OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Together, xAI, Perplexity, Google Gemini) and the local ones (Ollama, LM Studio, vLLM, llama.cpp) - the embeddings mode, the security gates (API token, channel secret, shell access), the per-task token budget, the MCP servers (with a small bounded set of one-click templates: filesystem, fetch, git, memory, …), and the persona (your `SOUL.md`). There is a **Test connection** button so you can check a key and model before saving, and a first-run wizard that walks a new user through connecting a model. A brand-new install runs the offline mock and says so honestly until you connect one.

Settings are stored in `<ENGRAM_HOME>/config.json` (written `0600`, since it holds keys). Almost everything applies immediately, with no restart: the model and provider are hot-swapped under the running daemon, the security gates and cost cap are read live, the persona shapes the very next run, and editing the MCP list reconnects the servers on the spot. The one exception is the embeddings mode, which is wired once at boot, so the panel offers a **Restart daemon** button for it (the desktop shell brings the daemon straight back). When there is no `config.json` yet, the daemon reads its settings from the environment below, so an existing env-configured deployment keeps working and shows its current state in the panel until you first save.

Everything is also configurable by environment variable, which is convenient for headless or scripted deployments:

| Variable | Default | Meaning |
|---|---|---|
| `ENGRAM_HOME` | `./brain` | Brain state directory: SQLite memory, ledger, and signing keys. |
| `ENGRAM_ADDR` | `127.0.0.1:8088` | Address the HTTP API and dashboard bind to. |
| `ENGRAM_IDLE_SECS` | `900` | Idle window, in seconds, before the core sleeps to zero. |
| `ENGRAM_EMBED` | _(unset)_ | `static` uses the pure-Rust model2vec embedder (real synonym recall; fetch a model with `scripts/build_embedder.py`). `gateway` embeds through the provider model. Unset uses the offline trigram default. Switching embedders re-embeds existing memories into the new space automatically. |
| `ENGRAM_STATIC_MODEL` | `<ENGRAM_HOME>/embedder` | Directory of the model2vec model (`tokenizer.json` + `model.safetensors`) used when `ENGRAM_EMBED=static`. |
| `ENGRAM_ANTHROPIC_API_KEY` | _(unset)_ | When set (with `--features http`), uses the native Anthropic provider, the Messages API with **prompt caching** of the tools+system prefix and token streaming. Takes priority over the OpenAI-compatible path; `ENGRAM_LLM_BASE_URL` optionally overrides the host. |
| `ENGRAM_LLM_BASE_URL` | _(unset)_ | OpenAI-compatible base URL for a real provider (requires building with `--features http`). |
| `ENGRAM_LLM_API_KEY` | _(unset)_ | API key for that provider. With both set, the gateway uses the real model for completions and embeddings; otherwise an offline mock. |
| `ENGRAM_MODEL` | `claude-haiku` | Model id the agent uses for the tool-use loop and delegated subagents. |
| `ENGRAM_VISION_MODEL` | _(`ENGRAM_MODEL`)_ | Override model for the `vision_analyze` tool, if the vision model differs. |
| `ENGRAM_TOOLS_SHELL` | _(unset)_ | Set to `1` to enable the `shell` tool. Off by default; always refused once a run is tainted. |
| `ENGRAM_SHELL_BACKEND` | _(local)_ | `docker` runs shell commands in a network-isolated container; `ssh` runs them on a remote host. Unset runs locally. |
| `ENGRAM_DOCKER_IMAGE` | `alpine` | Image used by the `docker` shell backend (`docker run --network none`). |
| `ENGRAM_SSH_HOST` | _(unset)_ | `user@host` for the `ssh` shell backend. |
| `ENGRAM_WORKDIR` | `<ENGRAM_HOME>/work` | Directory the agent's filesystem tools are confined to (symlink-resolving). |
| `ENGRAM_API_TOKEN` | _(unset)_ | When set, every `/v1` call must present `Authorization: Bearer <token>` (or `?token=` for SSE). The dashboard, `/health`, and webhooks stay open and the token is injected into the first-party UI. Unset = open (intended for the local `127.0.0.1` bind); set it whenever the daemon is exposed. |
| `ENGRAM_CHANNEL_SECRET` | _(unset)_ | When set, inbound webhooks (`/v1/channel/{platform}`) must present it via the `X-Engram-Secret` header or `?secret=` query, else `401`. (Channel runs are also started untrusted, so they can't shell or exfiltrate regardless.) |
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

The control center opens on the **Tasks** board: type into the chat composer and press
**⌘+Enter** to create a task or **⇧+⌘+Enter** to create and run it, then watch each card's
**"step N · tool"** progress and open it for the answer plus its signed audit slice.
**Chat** is a conversation that writes to episodic memory, recalls past turns, learns
identity facts about you, and persists across reloads; **Schedule** previews a
natural-language "when" as you type; **Memory** renders the brain's regions and tiers; and
**Skills** runs the seeded `shout` and `ask` skills (`ask` calls the model through the
gateway from inside the sandbox). The top bar's **"ledger verified · N"** and **today's
cost** chips stay live throughout.

To exercise the agent with real tools, enable the optional features and (for live media)
a provider:

```sh
# Build with the interactive browser, a real network LLM provider, and document ingest
# (--features docs adds PDF / DOCX / XLSX / CSV text extraction for chat uploads).
cargo build --release --features browser-cdp,http,docs

# Enable the shell, route it through a network-isolated container, and run.
ENGRAM_TOOLS_SHELL=1 ENGRAM_SHELL_BACKEND=docker \
ENGRAM_LLM_BASE_URL=https://api.example.com/v1 ENGRAM_LLM_API_KEY=… \
  ./target/release/engramd

# Isolate every task in its own git worktree (parallel agents on one repo, no clobbering).
# Point the workspace at a git repo, then:
ENGRAM_WORKTREES=1 ENGRAM_WORKDIR=/path/to/repo ./target/release/engramd
```

### Desktop app

A native Tauri shell that wraps the dashboard and starts the daemon for you - a *real*
desktop app, not a webview over a URL. On top of supervising `engramd` it wires the OS-level
surface a real agent app needs: a **system tray** (Show / Hide / Restart agent / Open at
login / Quit), **close-to-tray** so the agent stays reachable with the window shut, a
**native menu bar** (so clipboard shortcuts work in the webview on macOS), a **global hotkey**
(Cmd/Ctrl+Shift+Space) to summon it, a **single-instance lock** that focuses the existing
window, **run-at-login**, **window-state persistence**, the **`engram://` deep-link scheme**,
and **desktop notifications** on task completion. One command builds the daemon, stages it as
the bundled sidecar, and launches:

```sh
scripts/desktop.sh                         # needs: cargo install tauri-cli --version '^2'
scripts/desktop.sh build                   # native bundle (.app/.dmg/.deb/.AppImage/.msi)
```

The macOS `.app` build is verified end to end (it carries the Engram icon and weighs about
8 MB). See [`desktop/README.md`](./desktop/README.md).

The daemon also self-diagnoses: **`engramd doctor [HOME]`** prints a plain-language health
check (state dir, provider + key, embedder, ledger integrity, MCP servers, channels, security
gates, port) and exits non-zero on a hard problem - the local equivalent of the verify command
for "is this install set up right?".

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
| 10 | `engramd`: the daemon, HTTP API + desktop control center (Kanban board, glass-box signed task cards, trust/cost spine, Chat, Schedule, Memory, Skills, SSE) | **Done** |
| 11 | `engram-bench`: paraphrase recall harness | **Done** |
| 12 | `engram-agent`: tool-use loop, built-in tools (memory, files, shell, web, browser, media, delegate, messaging), MCP client, taint guard | **Done** |

## What's next

The engine is proven end to end offline, and the integration points for going online are
in place. Delivered since the initial v0.1:

- **The agentic layer**: `engram-agent`: the tool-use loop at `/v1/agent` and the
  dashboard Agent panel, with built-in tools for memory, files, the multi-backend shell,
  web, the interactive browser, vision/image/speech, subagent delegation, and messaging,
  plus an MCP client - all under the workdir confinement and taint guard described above.
- **Conversation memory**: `/v1/converse` and the Talk panel: each turn is logged to
  episodic memory, past turns are recalled, and identity facts about you are extracted
  and persisted across sessions.
- **Async LLM/Net host capabilities for skills**: a granted, untainted skill calls the
  model through the metered, audited gateway from inside the sandbox (seed `ask` skill).
- **Real model + embedder wiring**: `ENGRAM_EMBED=gateway` plus `--features http` and a
  provider URL/key route completions and embeddings through a real OpenAI-compatible model.
- **Swarms**: `/v1/swarm` composes multiple skills into a pipeline over shared input.
- **Desktop control center**: the dashboard, redesigned into one calm single-page window:
  a Kanban board fed by a chat composer with intent routing, glass-box signed task cards,
  an ambient trust/cost spine, graduated shell-approval autonomy, live scheduling, honest
  offline mode, and SSE-driven updates (see [Desktop](#desktop)).
- **Tauri desktop shell**: `desktop/` wraps that control center in a native window.

Remaining:

- **Provider key (optional)**: synonym-level paraphrase recall already ships **locally** via
  the pure-Rust static embedder (`ENGRAM_EMBED=static`; measured 100% recall@10 vs trigram's
  94%). A provider key with `--features http` is only needed to drive completions through a
  real model (Anthropic-native or OpenAI-compatible) and to light up live vision, image
  generation, and speech.
- **VPS deploy**: the generated systemd socket-activation and wake-timer units on a $5 VPS
  behind a reverse proxy, with the published $0.00/idle-hour table.
- **Hardware-backed audit keys**: TPM/Secure-Enclave/YubiKey or external co-signing to make
  the ledger tamper-*proof against host compromise* (today it is tamper-*evident*; the
  boundary is stated honestly in the threat model).
- **Voice mode** and **broader messaging/execution breadth**: the honest gaps against
  Hermes noted above; the MCP client narrows the integration gap in the meantime.

## Design principles

- **Less is more.** The smallest design that delivers the vision wins. Capability comes from architecture and the right primitives, not from lines of code.
- **Transparent and auditable.** Every memory write, skill mutation, tool call, and autonomous action is logged, attributable, and reversible. The audit ledger is signed and hash-chained, so you can replay it and prove nothing was rewritten. You can watch the brain think.
- **Near-zero idle.** A Rust core means a single small binary, tiny resident memory, and instant wake. It sleeps to nothing on a $5 VPS or a serverless trigger and wakes on an event. You should be able to forget it is running.
- **Skills are programs, not prompts.** A skill is executable, versioned, sandboxed, and self-improving: it measures its own success and rewrites itself toward it - under an explicit capability model, with every mutation signed and reversible.

## License

MIT. Copyright (c) 2026 Radoslav Tsvetkov. See [`LICENSE`](./LICENSE).

Authored by Radoslav Tsvetkov.
