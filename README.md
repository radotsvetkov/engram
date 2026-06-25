# Engram

**A personal AI agent with a brain you can watch grow — that costs nothing to run when idle.**

Engram is a self-improving personal AI agent modeled on how a brain actually works, then translated to a machine that costs almost nothing to run. It is a single static Rust binary (target <15MB) that sleeps to zero RAM between requests and wakes in milliseconds — versus an always-on Python+Node multi-hundred-MB runtime chain on a perpetually billing VPS. Its memory is hybrid semantic+keyword and region-partitioned, so it recalls paraphrased facts a keyword-only store returns zero hits for. Its skills are not prompts but small self-improving programs that run in a default-deny WASM sandbox and rewrite themselves from measured outcomes. Every memory write and skill mutation is signed, append-only, and one-click reversible, and a desktop app renders the brain — firing neurons, memory tiers, skills leveling up, the model of you — so growth is felt and governed, not silently committed to a Markdown folder you are told to audit later.

This is a working project under active construction. The reactive kernel, the signed hash-chained audit ledger, and the hybrid semantic+keyword memory broker are built and tested today; the gateway, WASM skills, taint enforcement, learning loop, scheduler, desktop, and benchmark harness are designed and on the build order. The sections below are explicit about what is **done** versus **planned**.

See [`VISION.md`](./VISION.md) for the north-star vision and [`docs/ADR-0001-architecture.md`](./docs/ADR-0001-architecture.md) for the architecture decision record that turns that vision into concrete calls.

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

### Built today

**`crates/engram-core` — the reactive kernel.** The brainstem. It owns the primitives everything else fires through:

- **Event bus (`event`)** — the neural substrate. A *spike* is the unit of activity; spikes flow across four priority lanes (Reflex → High → Normal → Low) on an in-process Tokio broadcast bus that exists *only while the core is awake*. There is no parked daemon. Every spike carries a monotonic provenance taint: anything derived from untrusted input is `Untrusted`, and taint only ever spreads — the basis for breaking the prompt-injection → exfiltration chain.
- **Lifecycle (`lifecycle`)** — wake/sleep. The core runs while there is activity and resolves to exit after an idle window (or on SIGINT/SIGTERM). On a socket-activated VPS this means no resident process between requests — the near-zero-idle property in one small module. Tested under Tokio's paused virtual clock, so the 90s idle behavior is verified without real sleeping.
- **Audit ledger (`ledger`)** — an append-only, content-addressed (BLAKE3), hash-chained, Ed25519-signed record of every state change. Each entry commits to the previous entry's hash, so altering any past entry breaks every hash after it and tampering is detected on replay. The signing key lives on disk at `0600` and is never handed to a skill. A "revert" is itself an appended entry pointing at a prior good hash — history is added to, never erased.
- **`engramd`** — the core daemon entrypoint. Boots the bus, attaches a cortex observer that keeps the brain awake while spikes flow, records boot/sleep to the ledger, and verifies the chain on the way out. Entrypoints are feature-gated: `standalone` (TCP bind, the default) ships now; `socket-activation` (zero-idle prod via systemd `LISTEN_FDS`) and `lambda` (serverless) are stubbed feature flags that arrive with the deploy phase.

**`crates/engram-memory` — the brain on disk.** Hybrid, region-partitioned, tiered memory in a single embedded SQLite (WAL) file that survives the core sleeping to zero.

- **Memory broker (`store`)** — `remember` and `recall`. Recall fuses a **keyword** arm (FTS5 / BM25) and a **semantic** arm (vector cosine over candidate rows) with Reciprocal Rank Fusion, so a paraphrased query with no shared words still surfaces the right memory — exactly where a keyword-only agent returns nothing. Each hit reports which arm carried it, so the UI can show *why* a memory surfaced. Writes are ledgered before they land; `forget`/`restore` and idle `consolidate` (warm→cold demotion of stale, low-importance facts) are all recorded and reversible.
- **Regions (`region`)** — memory is partitioned the way a brain is (Episodic, Semantic, Identity, Procedural), and recall consults only the regions that fit the task type, so a question about *who you are* does not scan every conversation you ever had.
- **Embeddings (`embed`)** — an `Embedder` trait with a dependency-free default (signed feature hashing over word tokens and character trigrams, L2-normalized) that keeps the binary tiny and the pipeline testable offline. A real transformer embedding model plugs into the same trait through the gateway when present.

**`crates/engram-gateway` — the LLM gateway.** The single audited choke-point every model and embedding call passes through, so nothing reaches a model off the record. Provider-agnostic behind a `Provider` trait (Anthropic / OpenAI / OpenRouter via an OpenAI-compatible `HttpProvider`, plus an offline `MockProvider` for tests). It meters tokens and cost per call, and enforces the first half of the taint rule: an untrusted call has its secret-bearing context stripped before it reaches the model, and the redaction is metered and ledgered. The network provider is an opt-in (`--features http`) so default builds stay small and offline.

### Planned (on the build order)

- **`engram-skills` — the skill runtime.** Wasmtime host with a pooling allocator, precompiled (`.cwasm`) modules, and a capability-gated, deny-by-default host ABI. Skills are versioned, instrumented programs, replay-tested on recorded inputs and A/B-gated before promotion. WASI authoring targets (Rust and AssemblyScript/TS first).
- **Taint enforcement.** The taint rule made live across gateway + skills + memory: any run that reads untrusted web/memory content is dropped to no-egress / no-secrets for that run, with the block visible in the ledger.
- **Learning loop.** Explicit accept/tweak/reject feedback turns a skill mutation into a new signed version, replayed against recorded inputs, A/B-gated on the skill's own metric, and promoted only on a win with consent — with one-click revert.
- **`engram-sched` — scheduling.** Deterministic NL→RRULE parse to a persisted `next_fire_at`; systemd-timer (VPS) or platform cron (serverless) wakes the sleeping core, it runs the job, and sleeps again — no resident process between fires.
- **`desktop/` — the Tauri app.** A Rust shell + Svelte UI over local HTTP+SSE, shipping three views: Live Cortex (neurons firing on the bus, from the audit stream), Memory Atlas (the tiers, browsable, one-click forget), and Skill cards (version, strength, accept/tweak/reject diff).
- **`bench/` — the benchmark harness.** Reproducible measurement of the headline numbers and a paraphrase recall@10 test set, side by side against a Hermes baseline.

## Measurable wins vs Hermes

The thesis is that Engram beats [Nous Hermes](https://github.com/nousresearch/hermes-agent) on the numbers that matter and on the one thing it cannot retrofit — a self-modifying agent that is provably sandboxed and auditable by construction.

| Dimension | Engram | Hermes |
|---|---|---|
| **Idle RAM** | 0 MB resident application memory at idle (socket-activated, no resident process); target sub-1MB for any always-on listener | always-on Python+Node VPS process, hundreds of MB |
| **Binary size** | single static musl binary <15MB stripped (target 8–15MB with SQLite+rustls bundled) | multi-hundred-MB Python+Node+ffmpeg+uv+ripgrep runtime chain |
| **Cold start** | core exec-to-first-byte <50ms on a $5 VPS; WASM skill instantiation in the microsecond-to-low-ms range via Wasmtime pooling + CoW + precompiled modules | unbenchmarked container-wake on the Modal/Daytona path |
| **Recall quality** | >0.85 recall@10 on a paraphrase test set where query tokens do not overlap stored text | keyword-only FTS5 returns near-0 on the same set |
| **Always-on memory** | no hard fact cap (importance-scored, summarization-compacted); measured as facts retained over a simulated 90-day session | ~2200-char (~800-token) `MEMORY.md` ceiling |
| **Self-modifying-skill safety** | 100% of skill executions sandboxed (WASM, default-deny), with the taint rule blocking exfil on untrusted-data runs | 0% validation on the live in-agent skill-patch path |
| **Idle cost** | published $0.00/idle-hour on the socket-activation / serverless path in a side-by-side 30-day table | $5/mo always-on VPS path |
| **Auditability** | 100% of memory writes and skill mutations signed, hash-chained, and one-click reversible | mutable Markdown/SQLite with manual review |

Several of these targets depend on planned components (skills, scheduler, bench harness). The idle-RAM, binary-size, recall, and auditability mechanics already exist in code; the bench harness will publish the verified table.

## Build & run

Requires a stable Rust toolchain (see `rust-toolchain.toml`). The release profile is tuned for a small, fast-to-load binary (`opt-level = "z"`, thin LTO, `codegen-units = 1`, `panic = "abort"`, stripped).

```sh
# Build the whole workspace, optimized.
cargo build --release

# Run the full test suite (kernel + memory: bus, lifecycle, ledger,
# hybrid recall, consolidation).
cargo test --workspace
```

### Running the core

`engramd` boots the neural bus, opens the signed audit ledger, fires a boot spike, and runs until it has been idle long enough to sleep to zero — demonstrating the full wake → activity → sleep lifecycle and verifying the ledger chain on exit.

```sh
# Default: 90s idle window, brain state under ./brain, info-level logs.
cargo run --release --bin engramd

# Or run the built binary directly.
./target/release/engramd
```

It is configured by environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `ENGRAM_IDLE_SECS` | `90` | Idle window, in seconds, before the core sleeps to zero. |
| `ENGRAM_HOME` | `./brain` | Brain state directory for the audit ledger (and signing key). |
| `RUST_LOG` | `info` | Tracing filter, e.g. `debug` or `engram_core=trace`. |

```sh
# Sleep after 5s idle, keep brain state in /tmp/engram, log everything.
ENGRAM_IDLE_SECS=5 ENGRAM_HOME=/tmp/engram RUST_LOG=debug \
  cargo run --release --bin engramd
```

The `brain/` state directory holds your personal memory and the ledger signing key; it is gitignored and must never be committed.

## Status

Per the architecture build order:

| Phase | Component | Status |
|---|---|---|
| 1 | Workspace + release profile (musl cross-compile target) | **Done** (workspace + profile in place) |
| 2 | `engram-core`: event bus, wake/sleep lifecycle, `engramd` | **Done** (standalone entrypoint; socket-activation/lambda feature-gated) |
| 3 | Append-only, content-addressed, signed audit ledger | **Done** |
| 4 | `engram-memory`: SQLite + FTS5 + hybrid recall + regions + consolidation | **Done** (transformer embeddings pending the gateway) |
| 5 | `engram-gateway`: provider-agnostic LLM + embeddings, metering, taint hook | Planned |
| 6 | `engram-skills`: Wasmtime host, capability manifests, ABI | Planned |
| 7 | Taint rule enforcement across gateway + skills + memory | Planned |
| 8 | Learning loop: instrument, replay eval, A/B promotion, revert | Planned |
| 9 | `engram-sched`: NL→RRULE, `next_fire_at`, systemd-timer wake | Planned |
| 10 | Tauri desktop: Live Cortex, Memory Atlas, Skill diff cards | Planned |
| 11 | `bench/`: paraphrase recall@10 + idle/size/cold-start harness vs Hermes | Planned |

## Design principles

- **Less is more.** The smallest design that delivers the vision wins. Capability comes from architecture and the right primitives, not from lines of code.
- **Transparent and auditable.** Every memory write, skill mutation, and autonomous action is logged, attributable, and reversible. The audit ledger is signed and hash-chained, so you can replay it and prove nothing was rewritten. You can watch the brain think.
- **Near-zero idle.** A Rust core means a single small binary, tiny resident memory, and instant wake. It sleeps to nothing on a $5 VPS or a serverless trigger and wakes on an event. You should be able to forget it is running.
- **Skills are programs, not prompts.** A skill is executable, versioned, sandboxed, and self-improving: it measures its own success and rewrites itself toward it — under an explicit capability model, with every mutation signed and reversible.

## License

MIT. Copyright (c) 2026 Radoslav Tsvetkov. See [`LICENSE`](./LICENSE).

Authored by Radoslav Tsvetkov.
