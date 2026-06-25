# ADR-0001 — Engram Architecture

- **Status:** Accepted
- **Author:** Radoslav Tsvetkov
- **Date:** 2026-06-25
- **Supersedes:** —
- **Relates to:** [`VISION.md`](../VISION.md) (the *why*), [`docs/THREAT-MODEL.md`](./THREAT-MODEL.md) (the full security model)

`VISION.md` states the destination. This record states how Engram gets there: the decisions that are now fixed, the alternatives that were rejected and why, the technology behind each layer, the repository that holds it, the numbers it must beat, and the questions still open. It is the authoritative *how* — when the vision and the code disagree, this document is the bridge between them.

A note on tense throughout: Engram is being built in the order described under *Build order*. Where a decision is already realised in code, it says **done**; where it is committed but not yet implemented, it says **planned**. Nothing here is aspirational hand-waving — the rejected alternatives are real designs that were considered and set aside.

---

## 1. Context and forces

Engram is a personal AI agent designed to be measurably better than [Nous Hermes](https://github.com/nousresearch/hermes-agent) on the axes that actually decide whether such a system is worth running: idle cost, footprint, cold-start latency, recall quality, and the safety of a self-modifying skill loop. Hermes is a capable agent, but its architecture carries structural costs Engram refuses to inherit:

- **An always-on, heavy runtime.** Hermes runs a Python + Node + ffmpeg + uv + ripgrep chain on a continuously billing VPS. Even idle, it holds hundreds of megabytes resident and bills by the hour.
- **Keyword-only recall.** Persistent memory is full-text (SQLite FTS5) plus a `~2200`-character `MEMORY.md` ceiling. A paraphrased query whose tokens do not overlap the stored text returns near-zero hits.
- **Unguarded self-modification.** Hermes patches its own skill code mid-use and executes it in-process with no validation — the single scariest attack surface an agent can have.

These are the forces the architecture must resolve:

1. **Near-zero idle cost is the headline.** The system must cost effectively nothing when no one is using it. This rules out any resident daemon as the *primary* always-on path.
2. **Self-modification must be safe by construction, not by audit-later.** If skills rewrite themselves, the isolation and reversibility have to be in the data model and the runtime, not in a Markdown folder a human is told to review.
3. **The brain metaphor has to earn its keep.** Tiered memory, regions, consolidation, firing neurons — each exists only where it buys capability-per-watt, never as decoration.
4. **Less is more.** v0.1 must prove the engine — memory + skills + learning loop — with the smallest design that demonstrates the differentiators. Everything beyond that is deferred on purpose, not forgotten.
5. **One isolation boundary, one audit spine.** Six sandbox backends and two storage engines are more attack surface and more behavioural drift than a v0.1 can defend. Pick one of each and make it provable.

The decisions below are the resolution of these forces.

---

## 2. Decisions

Each decision states the **Decision**, its **Rationale**, and the **Rejected alternative**.

### 2.1 Idle model: reactive core that sleeps to zero

**Decision.** The neuron/event bus is an **in-process Tokio bus that exists only while the core is awake**. The core is socket-activated by systemd on the VPS — there is no resident process at idle — and it self-exits after a 90-second idle window. The only always-on element is systemd's own listening socket plus a persisted `next_fire_at` row that a systemd-timer (or platform cron on serverless) honours. The brain metaphor is preserved: spikes cascade across the bus when work is happening. Zero-idle is preserved: nothing is parked.

*Status: the reactive kernel is done.* `engram-core` implements the priority-laned in-process bus (`event.rs`), the wake/activity/sleep lifecycle with a configurable idle window (`lifecycle.rs`), and a `standalone` entrypoint that boots the bus, runs until idle, and exits (`bin/engramd.rs`). The `socket-activation` and `lambda` entrypoints are declared as Cargo features and arrive with the deploy phase.

**Rationale.** Both timescales matter. A parked Tokio reactor still holds RSS and a billed process; socket activation gives literally zero application RAM at idle while keeping the reactive programming model fully intact whenever work is flowing. This is the single largest measurable win over Hermes's always-on path.

**Rejected alternative.** An always-on Tokio reactor running as a resident daemon. Even a "few MB RSS, parked on epoll" daemon loses the zero-idle headline to socket activation, and a separate always-on gateway process to act as an alarm clock is unnecessary when systemd-timer already holds `next_fire_at`.

### 2.2 Skill sandbox and authoring languages

**Decision.** **Wasmtime is the only skill runtime for v0.1**, using the pooling allocator with precompiled `.cwasm` modules and a capability-gated host ABI (deny-by-default manifest). Skill authoring narrows to anything that compiles to WASI — Rust and AssemblyScript/TypeScript first-class; Python-on-WASM later. No native subprocess, no Docker/SSH/Modal/Singularity/Daytona backends in v0.1.

*Status: planned* (`crates/engram-skills/`). The kernel that skills will run inside — the bus, taint type, and audit ledger — is done.

**Rationale.** Wasmtime instantiates in the microsecond range with copy-on-write and pooling, delivering fast cold start *and* deny-by-default capability security in one primitive — exactly what a self-modifying agent needs. One isolation boundary is auditable; six backends are scope creep and the bulk of the attack surface.

**Rejected alternative.** Polyglot-everything with a native subprocess fallback, plus six hardened sandbox backends. "Pick the best language per skill" is a v1+ luxury; for v0.1 the win is provable sandboxing, not language breadth. Native heavy-Python skills return later, behind the same broker.

### 2.3 Memory architecture

**Decision.** Three tiers, region-partitioned, in **one embedded store**: SQLite (WAL) with FTS5 for keyword search and vector search for semantics in a single file, fronted by a Rust **Memory Broker** exposing `remember(region, item)` and `recall(query, regions, k)` that performs hybrid retrieval and rank fusion. Hot memory is in-process structs; cold is the same SQLite file with summarised, demoted older episodes — no separate S3 cold tier in v0.1. Embeddings are produced on-device by a small model.

*Status: done, with two honest deltas from the target shape.* `engram-memory` implements the broker (`store.rs`): a single WAL SQLite file, an FTS5 virtual table for the keyword arm, a stored per-row embedding for the semantic arm, **Reciprocal Rank Fusion** over the two arms, four regions (`region.rs`: Episodic, Semantic, Identity, Procedural) with task-to-region routing, importance-scored consolidation that demotes stale low-importance rows warm→cold, and reversible `forget`/`restore`. Every write is recorded in the audit ledger before it lands. The two deltas: (1) the semantic arm is currently a brute-force cosine scan over candidate region rows in Rust rather than a `sqlite-vec` ANN index — correct and fast at v0.1 scale, with `sqlite-vec` the planned drop-in once corpus size demands it; (2) the on-device embedder shipping today is a dependency-free `TrigramHashEmbedder` (feature-hashed word tokens + character trigrams) behind an `Embedder` trait, which keeps the binary tiny and the pipeline testable offline. The real quantized transformer model plugs into the same trait through the gateway and is planned.

**Rationale.** Hybrid semantic+keyword recall is *the* differentiator over Hermes's lexical-only FTS5. One embedded file means zero idle cost, no Postgres/Pinecone daemon, and a brain that survives the core sleeping to zero. Region-first recall keeps retrieval fast and on-topic.

**Rejected alternative.** A separate `redb` hot KV store + Lance vector index + S3-compatible cold tier. Over-engineered for v0.1: one file covers vector search and tiering; S3 cold storage is a later durability optimisation (litestream snapshots suffice). Region partitioning stays as namespace scoping within the one store, not separate engines.

### 2.4 Security scope for v0.1

**Decision.** Ship exactly two controls now, both non-negotiable:

1. **The taint rule.** Any skill run that reads untrusted data (web or memory of unknown origin) is dropped to no-egress and no-secrets for that run, breaking the lethal trifecta of private data + code execution + network.
2. **Signed, append-only, content-addressed stores** for memory and skills, with one-click revert.

Capability manifests are declared and enforced by the broker (default-deny). Everything else — egress proxy, full backend hardening, supply-chain pinning, canary runs — is documented in the threat model but deferred.

*Status: the foundations are done; enforcement is planned.* The audit spine is built and live: `engram-core::ledger` is an append-only, BLAKE3 content-addressed, hash-chained, Ed25519-signed JSONL ledger whose signing key is held by the core at `0600` and never handed to a skill; it supports full-chain `verify` and revert-as-an-appended-entry. The taint *type* is implemented (`event::Taint`, monotonic `join`, carried on every spike and persisted on every memory row). The *enforcement* of the taint rule across gateway + skills + memory is planned and lands once those crates exist.

**Rationale.** Self-modifying skills create the trifecta; the taint rule alone neutralises injection→exfiltration — the highest-priority threat — at near-zero implementation cost. Signed, reversible stores give incident recovery for free, straight from the data model.

**Rejected alternative.** The full broker + vault + egress-proxy + canary pipeline in v0.1. Correct long-term, but shipping all of it first violates "less is more" and delays proving the loop. Taint + capabilities + reversibility is the minimal set that makes self-modification safe enough to demonstrate.

### 2.5 Swarm orchestrator and "grow into an agent"

**Decision.** Cut from v0.1 entirely. Skills stay stateless functions. No swarm, no agent-promotion, no blackboard.

*Status: cut (not planned for v0.1).*

**Rationale.** Neither is needed to prove the memory + skill + learning loop, and both multiply credit-assignment difficulty and attack surface. The event bus already makes them cheap to add later, once a real problem demands them.

**Rejected alternative.** A Swarm Orchestrator and skill→agent promotion. Genuinely interesting, but pure scope creep against a v0.1 whose job is to prove that one self-improving skill gets measurably better with use.

### 2.6 Learning loop and credit assignment

**Decision.** v0.1 uses only explicit, structured signals: the user's accept/tweak/reject on a diff card, plus success and retry counts. A skill mutation becomes a new **signed version**, is replayed against recorded past inputs, is A/B-gated on the skill's own metric, and is promoted only on a measured win with user consent.

*Status: planned.* The mechanisms it depends on — signed versioning and one-click revert in the ledger — are done.

**Rationale.** Implicit credit assignment is the hard, untrusted part. Conservative explicit feedback makes the learning loop demonstrable and safe, and maps directly to the diff-card / consent UX. Engram beats Hermes by making improvement continuous and free at the margin via replay-on-recorded-inputs — not by an expensive offline optimisation run.

**Rejected alternative.** Online implicit attribution and offline DSPy/GEPA-style optimisation. Deferred: they are not needed to ship a demonstrable learning loop, and implicit signals are exactly the untrusted part to keep out of v0.1.

### 2.7 Desktop client scope

**Decision.** Tauri (Rust shell + Svelte UI) talking to the core over local JSON HTTP+SSE. v0.1 ships three views only: **Live Cortex** (neurons firing on the bus, from the audit stream), **Memory Atlas** (the three tiers, browsable, one-click forget), and **Skill cards** (version, strength, accept/tweak/reject diff). Maturity Levels, swarm visualiser, dream-digest, and the editable Portrait are post-v0.1.

*Status: planned* (`desktop/`). The data it renders is already produced: the bus exposes an emitted-spike count and an observer feed for Live Cortex, and the memory broker exposes per-region/per-tier `Stats` for the Atlas.

**Rationale.** These three views are exactly what makes the loop *felt* and prove the differentiators: visible recall, visible self-improvement, visible reversibility. Tauri keeps the client a ~10 MB binary rather than Electron's ~150 MB.

**Rejected alternative.** The full product dashboard (maturity meter, dream-state digest, belief-receipts everywhere, editable Portrait) in v0.1. The right north star, but not needed to prove the engine; it layers onto the same audit stream later.

### 2.8 Reasoning and model routing

**Decision.** A single audited **LLM Gateway** choke-point with provider-agnostic configuration (Anthropic / OpenAI / OpenRouter). v0.1 routes everything to one configured frontier model; the small-local-model router is stubbed behind the same interface but not shipped.

*Status: planned* (`crates/engram-gateway/`). The `Embedder` trait in `engram-memory` is already shaped so the real embedding model arrives through this gateway without touching call sites.

**Rationale.** The gateway is the single point for metering, policy, and taint enforcement — essential now, because the taint rule lives here. Cheap-local-model routing is a cost optimisation that can wait; the interface is the thing that must not be gotten wrong.

**Rejected alternative.** Shipping the quantized local router (llama.cpp) in v0.1. Premature optimisation. One gateway interface that *can* route later is the minimal correct call.

---

## 3. Technology stack

| Layer | Choice |
|---|---|
| Core language / runtime | Rust + Tokio (current-thread runtime), statically linked via `x86_64`/`aarch64-unknown-linux-musl` |
| HTTP/SSE surface | axum + hyper, socket-activated (`LISTEN_FDS`) on VPS; same crate compiles to a standalone-bind dev mode via Cargo features |
| Event bus (neurons) | In-process Tokio broadcast/mpsc channels with a typed topic registry + priority lanes; lives only while the core is awake |
| Skill sandbox | Wasmtime with pooling allocator + precompiled `.cwasm` modules; capability-gated host ABI (deny-by-default manifest) |
| Skill authoring | Any WASI target; Rust and AssemblyScript/TypeScript first-class for v0.1 |
| Memory store | SQLite (WAL) with FTS5 + vector search in one file, via `rusqlite` (bundled); Rust Memory Broker for hybrid recall + fusion |
| Embeddings | Small quantized on-device model invoked through the gateway interface (e.g. bge-small / gte-small class); offline trigram-hash embedder as the bundled default for testing |
| Audit / versioning | Content-addressed (BLAKE3), hash-chained, append-only Ed25519-signed ledger for memory writes and skill versions; signing key held by the core, never exposed to WASM |
| LLM access | Single Rust LLM Gateway, provider-agnostic (Anthropic / OpenAI / OpenRouter), enforces the taint rule + token/cost metering |
| Scheduling | Deterministic Rust RRULE/duration grammar → persisted `next_fire_at` (UTC + IANA tz); systemd-timer (VPS) or platform cron (serverless) as the wake source; LLM only for ambiguous natural-language parse |
| Desktop client | Tauri (Rust shell) + Svelte/TypeScript UI over local JSON HTTP+SSE |
| TLS | Terminated at a reverse proxy (Caddy/nginx) on the VPS edge; rustls only if bundled for serverless |
| Primary deploy | $5-class VPS (Hetzner/Hostinger KVM1) under systemd socket activation behind Caddy; litestream snapshots to object storage for durability |
| Secondary deploy | AWS Lambda `provided.al2023` Arm64 (`lambda_http`), state on libSQL/Turso — same crate, feature-gated entrypoint |
| Release profile | thin LTO, `codegen-units=1`, `panic=abort`, strip, `opt-level=z` (target <15 MB stripped with SQLite + rustls bundled) |

The release profile is realised in the workspace `Cargo.toml` today (`opt-level = "z"`, `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`).

---

## 4. Repository layout

| Path | Purpose |
|---|---|
| `VISION.md` | North-star vision (authored); the destination this record implements. |
| `docs/ADR-0001-architecture.md` | This decision record — final calls, rejected alternatives, rationale. |
| `docs/THREAT-MODEL.md` | Full security threat model; marks which controls ship in v0.1 (taint rule, capabilities, signed reversible stores) versus deferred. |
| `Cargo.toml` | Workspace manifest tying the crates together; release-profile settings. |
| `crates/engram-core/` | The reactive kernel: event bus, wake/sleep lifecycle, taint type, audit ledger writer, and the feature-gated standalone/socket-activation/lambda entrypoints. **Done** (entrypoints beyond standalone are planned). |
| `crates/engram-memory/` | Memory Broker: SQLite + FTS5 + vector recall, regions, hybrid recall + fusion, consolidation, content-addressed writes to the ledger. **Done.** |
| `crates/engram-skills/` | Skill runtime: Wasmtime host, capability manifests + ABI, instrumentation, replay-based eval + A/B promotion gating. **Planned.** |
| `crates/engram-gateway/` | LLM Gateway: provider-agnostic model + embedding calls, taint-aware context assembly, token/cost metering. **Planned.** |
| `crates/engram-sched/` | Natural-language → RRULE parser, `next_fire_at` computation, systemd-timer / platform-cron integration, idempotency leases. **Planned.** |
| `skills/` | Built-in/seed skills (WASI source + signed manifests) to bootstrap procedural memory. **Planned.** |
| `desktop/` | Tauri shell + Svelte UI: Live Cortex, Memory Atlas, Skill diff cards. **Planned.** |
| `brain/` | Runtime state dir for the local SQLite brain and the ledger (gitignored; never commit personal memory). |
| `xtask/` or `scripts/` | Cross-compile (musl/arm64) + `.cwasm` precompile + release packaging. **Planned.** |
| `bench/` | Reproducible benchmarks for the four measurable wins + the paraphrase recall@10 test set versus a Hermes baseline. **Planned.** |

The workspace currently declares two members — `crates/engram-core` and `crates/engram-memory` — and grows by adding the planned crates above in build order. The kernel and the memory broker are the two pieces already present and tested.

---

## 5. Measurable targets versus Hermes

These are the numbers v0.1 must publish in a side-by-side `bench/` table. They are the contract this architecture exists to satisfy.

| Metric | Engram target | Hermes baseline |
|---|---|---|
| Idle RAM | 0 MB resident application memory at idle (socket-activated, no resident process); sub-1 MB for any always-on listener | Always-on Python + Node VPS process, hundreds of MB |
| Binary size | Single static musl binary <15 MB stripped (target 8–15 MB with SQLite + rustls bundled) | Multi-hundred-MB Python + Node + ffmpeg + uv + ripgrep runtime chain |
| Cold start | Core exec-to-first-byte <50 ms on a $5 VPS (dominated by SQLite WAL open); WASM skill instantiation in the microsecond-to-low-ms range via Wasmtime pooling + CoW + precompiled modules | Unbenchmarked container-wake on the Modal/Daytona path |
| Recall quality | >0.85 recall@10 on a paraphrase test set where query tokens do not overlap stored text | Keyword-only FTS5, near-0 on the same set |
| Always-on memory | No hard fact cap (importance-scored, summarisation-compacted); measured as facts retained over a simulated 90-day session without forced eviction | `~2200`-char (`~800`-token) `MEMORY.md` ceiling |
| Self-modifying-skill safety | 100% of skill executions sandboxed (WASM, default-deny), taint rule blocking exfiltration on untrusted-data runs | 0% validation on the live in-agent skill-patch path |
| Idle cost | Published $0.00/idle-hour on the socket-activation/serverless path in a 30-day side-by-side table | $5/mo always-on VPS path |
| Auditability | 100% of memory writes and skill mutations signed, hash-chained, one-click reversible | Mutable Markdown/SQLite with manual review |

The audit-spine target is already met in code: every memory write and ledger entry is BLAKE3 content-addressed, Ed25519-signed, hash-chained, and reversible via an appended revert entry, with a `verify` pass that detects any tampering in O(n).

---

## 6. v0.1 scope and build order

### 6.1 Scope

v0.1 proves the engine — memory, sandboxed self-improving skills, and the learning loop — end to end, and publishes the headline numbers. Concretely:

1. A single static Rust binary that socket-activates on a VPS, serves a turn, and self-exits to zero RAM after 90 s idle — proving the near-zero-idle claim end to end.
2. **Memory Broker:** write a provenance-tagged fact and recall it by hybrid semantic+keyword search across regions, demonstrably returning a result for a *paraphrased* query that keyword-only FTS5 misses.
3. **One self-improving WASM skill** (e.g. a message drafter) that runs sandboxed under a capability manifest, is instrumented for outcome, and on accept/tweak/reject produces a new signed version, replay-tested on recorded inputs, A/B-gated, and promoted only on a metric win.
4. **The taint rule live:** a skill run that reads untrusted web/memory content is automatically dropped to no-egress/no-secrets for that run, with the block visible in the audit log.
5. **Append-only signed audit ledger** with one-click revert of any memory record or skill version to a prior known-good hash.
6. **Deterministic NL scheduling:** "every weekday at 9am" → persisted `next_fire_at`; systemd-timer wakes the sleeping core, it runs the job and sleeps again, with no resident process between fires.
7. **Tauri desktop app** showing the three v0.1 views (Live Cortex, Memory Atlas with one-click forget, Skill card with version diff + accept/tweak/reject) over local SSE.
8. **A reproducible `bench/` harness** publishing the four headline numbers versus a Hermes baseline.

Items 1, 2, and 5 are substantially realised in `engram-core` and `engram-memory` today; the rest is the work ahead, in the order below.

### 6.2 Build order

The order is deliberate: lock the idle-RAM win first, build the audit spine before anything mutates state, then layer memory, reasoning, skills, enforcement, learning, scheduling, and finally the desktop and benchmarks on top.

1. Workspace + release profile + CI cross-compile to musl (`x86_64`/`aarch64`); prove a hello binary builds <15 MB stripped. **— Done** (workspace + release profile in place).
2. `engram-core`: in-process event bus + the wake/sleep lifecycle + the feature-gated entrypoints; get socket activation + 90 s self-exit working on a VPS (lock the idle-RAM win first). **— Done** (bus + lifecycle + standalone entrypoint; socket-activation/lambda entrypoints planned).
3. Append-only, content-addressed, signed audit ledger (every later component writes to it; build it before anything mutates state). **— Done.**
4. `engram-memory`: SQLite + FTS5 + vector recall, regions, `remember`/`recall` hybrid + fusion, on-device embeddings; wired to the ledger. **— Done** (`sqlite-vec` ANN index and the transformer embedder are the planned upgrades behind existing seams).
5. `engram-gateway`: provider-agnostic LLM + embedding calls behind one interface, with token/cost metering and the taint hook. **— Planned.**
6. `engram-skills`: Wasmtime host + capability manifest + ABI; run one signed WASM skill sandboxed end to end. **— Planned.**
7. Taint-rule enforcement across gateway + skills + memory (untrusted read → no egress/secrets), visible in the ledger. **— Planned** (the taint type and propagation primitive are done).
8. Learning loop: instrument the skill, replay-on-recorded-inputs eval, A/B promotion gate, signed new version + one-click revert. **— Planned** (signed versioning + revert are done).
9. `engram-sched`: NL → RRULE parse, `next_fire_at` persistence, systemd-timer wake of the sleeping core, idempotency lease. **— Planned.**
10. Tauri desktop: Live Cortex (from the ledger stream), Memory Atlas (+forget), Skill diff card (accept/tweak/reject) over SSE. **— Planned.**
11. `bench/`: paraphrase recall@10 test set + idle-RAM/binary-size/cold-start measurement harness versus a Hermes baseline; publish the table. **— Planned.**

---

## 7. Open questions

These are known and unresolved. Each affects either a headline number or the safety story, and each must be answered before the relevant build-order step closes.

1. **On-device embedding model choice and its size/latency budget** inside the <15 MB-binary and <50 ms-cold-start envelope — bundle the model, run it as a sidecar, or call out to the gateway? This directly trades against the binary-size and cold-start wins. (The offline trigram-hash embedder is the placeholder until this is settled.)
2. **Serverless storage divergence.** The VPS uses embedded SQLite-on-disk; serverless needs libSQL/Turso. One trait, two backends risks behavioural drift. Is the Lambda path a v0.1 commitment, or a v1 "compiles but unproven" claim?
3. **A fair Hermes baseline for paraphrase recall@10.** Do we stand up real Hermes for the comparison, or cite its documented keyword-only behaviour? This affects the credibility of the headline recall number.
4. **Capability-manifest UX.** How much does the user approve per-skill versus per-run before it becomes friction? We need a default policy that is safe but not annoying.
5. **Wake-on-event abuse.** A public socket-activated endpoint means every probe wakes the core and costs money. We need a cheap unauthenticated bounce + rate limit before the VPS is internet-facing.
6. **WASI maturity for the promised authoring languages**, Python-on-WASM in particular — confirm what is actually shippable in v0.1 versus Rust/AssemblyScript only.
7. **Missed-fire policy for scheduling** when the VPS was suspended past a fire time: catch-up versus skip, to avoid both silent misses and a reminder stampede on wake.

---

*This record is the authoritative description of how Engram is built. `VISION.md` explains why; the code in `crates/` proves it. When any two of the three disagree, this document is where the disagreement is resolved.*
