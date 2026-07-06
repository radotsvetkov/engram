# Architecture

This document explains how Engram is put together and why the pieces are shaped the
way they are. If you want the security reasoning in depth, read it alongside
[THREAT-MODEL.md](./THREAT-MODEL.md); for the terminal client, see [CLI.md](./CLI.md).

## The idea in one paragraph

Engram is a personal AI agent modelled loosely on how a brain works. It keeps what
matters, forgets the rest, and gets a little better every time you use it. It runs as a
single small Rust binary that drops to zero resident memory when nothing is happening and
wakes in milliseconds, so it costs almost nothing to keep around. Everything it does to
its own memory or skills is signed, append-only, and reversible — and you can watch it
happen live instead of reading a log after the fact.

## The brain, mapped to the machine

| Brain | Engram | Role |
|---|---|---|
| Working memory (prefrontal) | **Hot** store, in-RAM | the current task and context |
| Hippocampus | **Episodic** encoder + consolidator | recent experiences, conversations |
| Neocortex | **Semantic** store (warm → cold) | consolidated long-term knowledge |
| Basal ganglia | **Procedural** store | skills and habits, the self-improving programs |
| Amygdala | **Salience** tagging | what matters, what to keep, what to forget |
| Neurons / synapses | **Reactive event bus** | fire on events; weights encode learning |
| Sleep / consolidation | **Idle consolidation** | move warm → cold, strengthen what's used, prune the rest |
| Self-model | **Identity model** | the deepening picture of *you* |

## The workspace

Engram is a Rust workspace of small, single-purpose crates. Capability comes from the
right primitives and the right isolation boundaries, not from a large codebase. Every
crate that changes state writes to the audit ledger first, so the whole system is
tamper-evident and reversible by construction.

```
crates/
  engram-core      reactive kernel: event bus, wake/sleep lifecycle, signed ledger
  engram-memory    hybrid tiered memory on embedded SQLite (FTS5 + vectors)
  engram-gateway   the single audited choke-point for every model and embedding call
  engram-skills    signed, capability-sandboxed skills (WASM + process) and the learning loop
  engram-sched     deterministic natural-language scheduling and systemd unit generation
  engram-agent     the tool-use loop and the built-in tools
  engramd          the daemon that ties it together and serves the API + dashboard
  engram-cli       the terminal client (scriptable CLI + full-screen TUI)
  engram-bench     the paraphrase-recall and footprint benchmark
  engram-eval      deterministic, replay-based regression testing for the agent loop
```

### `engram-core` — the reactive kernel

The brainstem. It owns the primitives everything else fires through.

- **Event bus.** The neural substrate. A *spike* is the unit of activity; spikes flow
  across four priority lanes (Reflex → High → Normal → Low) on an in-process broadcast
  bus that exists *only while the core is awake*. There is no parked daemon. Every spike
  carries a monotonic provenance taint: anything derived from untrusted input is marked
  `Untrusted`, and taint only ever spreads — the basis for breaking the
  prompt-injection → exfiltration chain.
- **Lifecycle.** Wake and sleep. The core runs while there is activity and resolves to
  exit after an idle window (or on `SIGINT`/`SIGTERM`). On a socket-activated host this
  means no resident process between requests. The idle behaviour is tested under a paused
  virtual clock, so it is verified without real sleeping.
- **Audit ledger.** An append-only, content-addressed (BLAKE3), hash-chained,
  Ed25519-signed record of every state change. Each entry commits to the previous entry's
  hash, so altering any past entry breaks every hash after it and tampering is detected on
  replay. The signing key lives on disk at `0600` and is never handed to a skill. A
  "revert" is itself an appended entry pointing at a prior good hash — history is added
  to, never erased. `engramd verify [HOME]` replays the chain against the published public
  key **offline, without starting or trusting the daemon** (exit 0 = intact, 1 = tampered
  at a named sequence number).

### `engram-memory` — the brain on disk

Hybrid, region-partitioned, tiered memory in a single embedded SQLite (WAL) file that
survives the core sleeping to zero.

- **Recall** fuses a **keyword** arm (FTS5 / BM25) and a **semantic** arm (vector cosine
  over candidate rows) with Reciprocal Rank Fusion, so a paraphrased query that shares no
  words with the stored fact still surfaces it — exactly where a keyword-only store returns
  nothing. Each hit reports which arm carried it, so the interface can show *why* a memory
  surfaced.
- **Regions.** Memory is partitioned the way a brain is (Episodic, Semantic, Identity,
  Procedural), and recall consults only the regions that fit the task, so a question about
  who you are does not scan every conversation you have ever had.
- **Embeddings.** An `Embedder` trait with a dependency-free default (signed feature
  hashing over word tokens and character trigrams, L2-normalised) that keeps the binary
  tiny and the pipeline testable offline, plus a **pure-Rust static (model2vec) embedder**
  for real synonym recall (`ENGRAM_EMBED=static`) read straight from `model.safetensors` —
  no ONNX runtime, no heavy ML dependency. Changing embedders re-embeds existing memories
  into the new space.

### `engram-gateway` — the model gateway

The single audited choke-point every model and embedding call passes through, so nothing
reaches a model off the record. Provider-agnostic behind a `Provider` trait: an offline
`MockProvider` runs everywhere with no credentials, and an `HttpProvider` (behind
`--features http`) is opt-in so default builds stay small and offline. Anthropic uses its
native Messages transport; every other backend shares one OpenAI-compatible code path, each
with a built-in default endpoint. It meters tokens and cost per call, and enforces the
first half of the taint rule: an untrusted call has its secret-bearing context stripped
before it reaches the model, with the redaction metered and recorded.

### `engram-skills` — skills are programs, not prompts

Skills are small, signed programs on **two substrates**, each behind the same capability
model. The manifest names the runtime and is signed with the bytes, so a skill can't be
silently re-pointed at a different interpreter.

1. **WASM.** Pure-compute transforms run in a capability-sandboxed, fuel-bounded host (via
   `wasmi`, a pure-Rust interpreter chosen for a tiny binary and a deterministic
   deny-by-default sandbox). A skill receives a host function only if its signed manifest
   was granted the matching capability; importing anything ungranted fails to link, so an
   over-reaching skill never starts, and a runaway skill traps on fuel exhaustion instead
   of hanging the core.
2. **Process.** The richer script skills (the seed library is Python) run through the
   agent's shell backend, so they carry the shell's guarantees: **off by default** (the
   shell gate must be enabled), **refused the instant a run has read untrusted content**,
   and network-isolated only under the **Docker** backend (`docker run --network none`).

On top of both sits the **learning loop**: a candidate version is replayed against the
inputs the skill has actually seen, scored on the skill's own metric, A/B-gated
head-to-head against the incumbent, promoted only on a measured win with consent, and one
`set_active` away from being reverted.

### `engram-sched` — the scheduler

Deterministic natural-language → recurrence parsing ("every weekday at 9am") with no model
call, persisted jobs that reschedule forward across sleep with skip-on-missed so a
suspended host does not stampede on wake, and generators for the systemd socket-activation
and wake-timer units that make zero-idle and scheduled wake real. Every change is recorded
in the ledger.

### `engram-agent` — the tool-use loop

This is where the model stops talking and starts doing. The agent advertises its tools to
the model, runs the calls the model makes, feeds each observation back, and repeats until
the model answers with no further tool call (or a step budget is hit). Every step is
recorded, so a run is a replayable trace, not a black box. The loop runs a turn's
independent tool calls **in parallel**, retries model calls with backoff, **compacts** the
transcript so long runs don't overflow the context window, maintains an explicit **plan**
shown as a live checklist, and runs a **verify-before-finish** pass — all while the
no-egress taint gate holds. With a native Anthropic key it adds prompt caching of the
tools+system prefix and token streaming.

The built-in tools are deliberately small and auditable: hybrid `memory_recall` /
`memory_remember`; workdir-confined `read_file` / `write_file` / `list_dir`; a `shell` with
local / Docker / SSH backends (off by default); `web_search` / `web_fetch` that work with
no API key; a headless and an interactive Chrome browser over the DevTools Protocol;
`delegate_task` for depth-bounded subagents; `vision_analyze` / `image_generate` /
`text_to_speech` through the metered gateway; `send_message` plus a Telegram channel; and an
MCP client that turns any Model Context Protocol server into a native, audited tool.

### `engramd` — the daemon

Where the parts become an agent. It opens the ledger, the hybrid memory, the skill
registry, the gateway, the scheduler, and the agent's toolset, and exposes them over a small
local HTTP API plus the desktop control center. Every request keeps the brain awake and
fires a spike; after an idle window with no requests the process exits to zero.

### Identity: SOUL.md, briefs, charters, and self-models

Four distinct, non-overlapping layers shape what an agent knows about itself and the user,
assembled into every run's prompt in this order: **SOUL.md** (one global, live-editable file —
your standing voice/rules, the same for every project and every agent); a project's own
**brief** (per-project standing instructions, independent of SOUL.md — what gives one project
a different voice than another); a durable agent's **charter** (its role/system prompt — the
specialization that narrows focus, distinct from persona/brief, which shape voice rather than
mandate); and **consciousness** (a small, always-loaded, distilled self-model — never hand-
written, only ever the verbatim text of real trusted memories, so every line traces to
evidence). Consciousness has two independent slices: the global one (facts about the user,
shared by every agent) and, for each durable named agent, its own — distilled only from facts
*that agent itself wrote* (tagged `agent:<name>` at write time), so a content-writer agent and
a research agent accumulate separate expertise even while sharing the same underlying memory
store. Uploaded documents are deliberately excluded from all of this: they become ordinary,
recallable memory chunks, never standing instructions — the guard against turning a merely-
uploaded PDF into a prompt-injection channel.

### `engram-cli` — the terminal client

The same control surface, keyboard-first, in your terminal. A single small binary (`engram`)
that talks to the daemon over the very same HTTP API the desktop uses, so the daemon stays
the one audited choke-point. Run `engram` with no arguments for a full-screen TUI, or use a
subcommand for scripting. See [CLI.md](./CLI.md).

## The security spine

Three ideas do most of the work, and they compose:

1. **Everything is on the record.** Every memory write, skill mutation, and tool call is a
   signed, hash-chained ledger entry. The run is auditable by construction, not by a log you
   are asked to trust.
2. **Dangerous capabilities are closed by default.** The filesystem is confined to the
   workdir; the shell is off unless you turn it on.
3. **Taint breaks the injection → exfiltration chain at the boundary.** The run is *tainted*
   the instant a web or browser tool reads untrusted content, and taint only ever spreads.
   After that the shell is refused and the model's secret-bearing context is stripped.
   **Egress** is refused once the run is *also* holding something worth stealing — it has
   read your private data through a `memory_recall`, a `read_file`, or an authenticated MCP
   call. Gating egress on that conjunction is deliberate: pure web research keeps working,
   while a run that could leak your data cannot carry it out.

This is *tamper-evident* against post-hoc edits and any party without the signing key. It is
not tamper-*proof* against a fully compromised host that holds the key; hardware-backed keys
and external co-signing are the deferred work that would close that gap. The boundary is
stated honestly in [THREAT-MODEL.md](./THREAT-MODEL.md).

## Measuring it honestly

Two harnesses keep the claims grounded:

- **`engram-bench`** writes a labelled fact/query set plus distractors into the real memory
  broker and reports recall@10, MRR, and the zero-lexical-overlap subset where a keyword
  index scores zero by construction.
- **`engram-eval`** records a task plus the exact model completions a run received, then
  replays it through the *real* agent and *real* tools with a scripted provider — no model,
  no network, fully deterministic — asserting the tool sequence, answer, and stop reason
  against a baseline. Change a prompt, a tool, or the loop, re-run it, and a regression is a
  failing case, not a hunch.
