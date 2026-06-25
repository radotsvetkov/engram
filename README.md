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

**`crates/engramd` — the daemon.** This is where the parts become an agent. It opens the ledger, the hybrid memory, the skill registry, the gateway, and the scheduler, and exposes them over a small local HTTP API plus a dashboard. The dashboard ships five live views: **Live Cortex** (the audit stream as neurons firing on the bus), **Memory Atlas** (regions and tiers, browsable, with one-click forget), **Skills**, **Schedule**, and the **gateway meter**. Every request keeps the brain awake and fires a spike; after an idle window with no requests the process exits to zero, so on a socket-activated VPS there is nothing resident between uses.

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

## Measurable wins vs Hermes

The thesis is that Engram beats [Nous Hermes](https://github.com/nousresearch/hermes-agent) on the numbers that matter and on the one thing it cannot retrofit — a self-modifying agent that is provably sandboxed and auditable by construction.

| Dimension | Engram | Hermes |
|---|---|---|
| **Idle RAM** | 0 MB resident application memory at idle (socket-activated, no resident process) | always-on Python+Node VPS process, hundreds of MB |
| **Binary size** | single static binary, 3.2 MB native (musl on the VPS) | multi-hundred-MB Python+Node+ffmpeg+uv+ripgrep runtime chain |
| **Cold start** | core exec-to-first-byte in milliseconds (dominated by SQLite WAL open); WASM skill instantiation in the microsecond-to-low-ms range | unbenchmarked container-wake on the Modal/Daytona path |
| **Recall quality** | 100% recall@10 on a paraphrase test set, including a zero-lexical-overlap subset where query tokens do not overlap stored text | keyword-only FTS5 returns 0 on that subset by construction |
| **Always-on memory** | no hard fact cap (importance-scored, consolidation-compacted), surviving the core sleeping to zero | ~2200-char (~800-token) `MEMORY.md` ceiling |
| **Self-modifying-skill safety** | 100% of skill executions sandboxed (WASM, deny-by-default, fuel-bounded), with egress revoked on untrusted-data runs | 0% validation on the live in-agent skill-patch path |
| **Idle cost** | $0.00/idle-hour on the socket-activation path | $5/mo always-on VPS path |
| **Auditability** | 100% of memory writes and skill mutations signed, hash-chained, and one-click reversible | mutable Markdown/SQLite with manual review |

## Build & run

Requires a stable Rust toolchain (see `rust-toolchain.toml`). The release profile is tuned for a small, fast-to-load binary (`opt-level = "z"`, thin LTO, `codegen-units = 1`, `panic = "abort"`, stripped). Everything builds and tests offline — the network LLM provider is opt-in behind `--features http`.

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
| `RUST_LOG` | `info` | Tracing filter, e.g. `debug` or `engram_core=trace`. |

```sh
# Sleep after 30s idle, keep brain state in /tmp/engram, log everything.
ENGRAM_IDLE_SECS=30 ENGRAM_HOME=/tmp/engram RUST_LOG=debug \
  ./target/release/engramd
```

The dashboard includes **Talk** (a conversation that writes to episodic memory, recalls
past turns, and learns identity facts about you), **Memory Atlas**, **Skills** (run the
seeded `shout` and `ask` skills — `ask` calls the model through the gateway from inside
the sandbox), **Schedule**, **Live Cortex** (the audit stream), and the gateway meter.

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
| 10 | `engramd`: the daemon — HTTP API + dashboard (Live Cortex, Memory Atlas, Skills, Schedule, meter) | **Done** |
| 11 | `engram-bench`: paraphrase recall harness | **Done** |

## What's next

The engine is proven end to end offline, and the integration points for going online are
in place. Delivered since the initial v0.1:

- **Conversation memory** — `/v1/converse` and the Talk panel: each turn is logged to
  episodic memory, past turns are recalled, and identity facts about you are extracted
  and persisted across sessions.
- **Async LLM/Net host capabilities for skills** — a granted, untainted skill calls the
  model through the metered, audited gateway from inside the sandbox (seed `ask` skill).
- **Real model + embedder wiring** — `ENGRAM_EMBED=gateway` plus `--features http` and a
  provider URL/key route completions and embeddings through a real OpenAI-compatible model.
- **Tauri desktop shell** — `desktop/` wraps the dashboard in a native window.

Remaining:

- **Provider key** — with a real embedding model configured, synonym-level paraphrase
  recall (>0.85) on top of today's morphological recall; the benchmark harness measures it.
- **VPS deploy** — the generated systemd socket-activation and wake-timer units on a $5 VPS
  behind a reverse proxy, with the published $0.00/idle-hour table.
- **Skill sidecar packaging** for the desktop app, and **swarms** (multiple skills
  composing on the event bus), deferred from v0.1 by design.

## Design principles

- **Less is more.** The smallest design that delivers the vision wins. Capability comes from architecture and the right primitives, not from lines of code.
- **Transparent and auditable.** Every memory write, skill mutation, and autonomous action is logged, attributable, and reversible. The audit ledger is signed and hash-chained, so you can replay it and prove nothing was rewritten. You can watch the brain think.
- **Near-zero idle.** A Rust core means a single small binary, tiny resident memory, and instant wake. It sleeps to nothing on a $5 VPS or a serverless trigger and wakes on an event. You should be able to forget it is running.
- **Skills are programs, not prompts.** A skill is executable, versioned, sandboxed, and self-improving: it measures its own success and rewrites itself toward it — under an explicit capability model, with every mutation signed and reversible.

## License

MIT. Copyright (c) 2026 Radoslav Tsvetkov. See [`LICENSE`](./LICENSE).

Authored by Radoslav Tsvetkov.
