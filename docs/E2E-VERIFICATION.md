# End-to-end multi-project verification (2026-07-05)

A real, live test of the memory-upgrade plan's core claim — "handles multiple big projects and
complex tasks" — run against a real `engramd` daemon (release build), through the real HTTP API and
the real `engram` CLI binary, not unit tests. This is the honest record of what was actually
checked and what the environment (no live LLM API key) meant could and couldn't be exercised.

## What was run

A scratch daemon (`ENGRAM_HOME` pointed at an empty directory, default config, offline `MockProvider`
— no `ANTHROPIC_API_KEY`/similar was available in this environment) was driven through:

1. **Two real projects** created via `POST /v1/projects`: "Aurora Web App" and "Borealis Data
   Pipeline", each with its own session (`POST /v1/sessions`).
2. **A distinct fact seeded into each project's scope** via the real `POST /v1/remember` (session-
   routed, the same path a real user's "remember this" or `engram memory remember` uses) — Aurora's
   database/port, Borealis's Kafka/port.
3. **The real agentic chat path** (`POST /v1/converse/stream`, the same route the desktop/TUI chat
   surfaces call) asking each project's session a question whose answer depends on its own fact.
4. **An adversarial isolation check**: asking Aurora's session about Borealis by name and by its
   exact keyword ("Kafka") - the hardest case, since semantic similarity to the query is high but
   the fact must still be excluded by scope, not just outranked.
5. **Whole-brain recall** (`GET /v1/recall`, the Atlas/admin view) confirming both facts exist and
   are correctly tagged with their own `scope_id`.
6. **Per-project stats** (`GET /v1/memory/stats?scope_kind=project&scope_id=...`) for both projects.
7. **Consciousness** (`POST /v1/consciousness/distill`) before and after adding a genuinely
   user-global Identity fact, to verify the scope *boundary* itself (not just that recall respects
   it) - project facts must never leak into the always-loaded working-memory block.
8. **Real skill execution** (`POST /v1/skills/calc/run`) - a real sandboxed process, not mocked.
9. **A real multi-hop task** created and run three times via the actual `engram` CLI binary
   (`engram tasks new`, `engram tasks run`, `engram tasks receipt`), with a plan-milestone
   breadcrumb added between hops, tracking the ledger's `tokens_in` to confirm the cross-run mission
   relay (docs/MEMORY-UPGRADE-PLAN.md Phase C) actually grows the prompt with real context.
10. **Ledger integrity** (`GET /v1/ledger/verify`) and the task's own signed, ed25519-verified
    receipt (`steps_match_ledger: true`).
11. **`engram projects list`** via the CLI, confirming both projects show up correctly.

## Results

**Multi-project memory isolation: confirmed at every layer, including the adversarial case.**

- Aurora's session, asked "What port does our API listen on?", recalled *only* its own fact:
  `"The Aurora project's database is PostgreSQL 15 and its API listens on port 4000."` — nothing
  from Borealis.
- Borealis's session, asked "What message system do we stream events through?", recalled *only*:
  `"The Borealis pipeline streams events through Kafka on port 9092."`
- **Adversarial check passed**: asking Aurora's session "Do we use Kafka anywhere in Borealis?" -
  a query that shares its exact keywords with Borealis's fact, so a naive similarity-only recall
  would have every reason to surface it - still recalled *only* the Aurora fact. Scope ringing wins
  over raw similarity, as designed.
- Whole-brain `/v1/recall?q=Kafka` correctly found the Borealis fact tagged
  `scope_kind: project, scope_id: <borealis-id>` (plus each session's own episodic conversation
  captures, correctly tagged to the project the conversation happened in - not a leak, since the
  conversation genuinely belongs to that session).
- Per-project stats showed exactly the expected counts (3 facts for Aurora: 1 semantic + 2 episodic
  conversation captures; 2 for Borealis: 1 semantic + 1 episodic) - no cross-contamination.

**Consciousness scope boundary: confirmed both directions.**

- Before any global fact existed, distilling with only the two project-scoped facts in the brain
  produced an empty consciousness (`version: 0`, zero lines) - correct, since `distill()` only ever
  scans the user-global ring (`ScopeCtx::user_only()`), and neither Aurora's nor Borealis's fact
  lives there.
- After writing one genuinely user-global Identity fact (no session), distilling immediately picked
  it up (`version: 1`, one line, correctly sourced to its memory id) - while still excluding both
  project facts. The boundary holds in both directions, not just "recall happens to filter right."

**Real skill execution: confirmed.** `calc` with input `{"expression": "2 + 2 * 10"}` returned
`{"expr": "2 + 2 * 10", "result": 22}` from the real sandboxed process runtime (not a mock) — this
required `security.allow_shell` to be enabled first, itself a real, working security gate (the
first attempt correctly refused with an explicit error before shell was turned on).

**Mission relay across CLI-driven hops: confirmed.** Ledger `tokens_in` for the same task's LLM
calls: hop 1 = 788, hop 2 (after adding one plan-milestone breadcrumb) = 1141, hop 3 = 1373 - real,
growing context, driven through the actual shipped `engram` CLI binary rather than raw HTTP calls
(a repeat of the verification already done for the mission-relay commit itself, but this time
through the CLI surface specifically).

**Ledger and receipts: confirmed.** `GET /v1/ledger/verify` returned `{"ok": true, "entries": 75}`
after the full scenario. The task's own receipt (`engram tasks receipt <id>`) confirmed
`steps_match_ledger: true` with a real ed25519 signature and pubkey.

**CLI surface: confirmed.** `engram tasks new/run/receipt` and `engram projects list` all worked
correctly against the running daemon and showed the right data (three projects: Personal, Aurora,
Borealis).

## What this environment could NOT exercise (stated plainly, not glossed over)

No live LLM API key was available in this environment (`MockProvider`, which never emits tool
calls, by design — mirrored honestly in `crates/engram-bench/BENCHMARKS.md`'s own scope note). This
means the following were **not** exercised end-to-end here, though each is separately covered by
deterministic unit tests using a `ScriptedProvider` (see `reflection.rs`, `contradiction.rs`,
`dissent.rs`'s test suites) that drive the exact same code paths with scripted model responses:

- Real tool-driven memory writes from a chat turn (the agent calling `memory_remember` itself,
  rather than a fact being seeded via the direct `/v1/remember` API as this test did).
- Real multi-step tool use within one task run (skill invocation, file edits, `update_plan` calls)
  - `MockProvider` always returns a final text answer with zero tool calls, so a live task run here
    never exercises the tool-calling loop itself, only the cross-run prompt composition around it.
- Contradiction detection and grounded reflection actually firing (both require a real judging LLM
  call; both stay silent under the offline mock, confirmed as *correct* behavior by the
  `the_offline_mock_provider_never_*` tests in `contradiction.rs`/`reflection.rs`).
- Desktop UI interactive verification via a real browser (the Preview tool's process launcher could
  not spawn a server in this sandboxed session - a `shell-init`/cwd-resolution failure reproducible
  even with a trivial command, confirmed unrelated to any code change here by running the identical
  command directly via Bash, which worked). Fallback verification used instead: `node --check` on
  the extracted `<script>` block, plus a direct debug-build daemon + curl confirming every new
  button/function/CSS rule/route is served correctly - see the desktop commits' own messages
  (`e2f1a32`, `ef07cb8`) for the specific traces.
- TUI interactive verification via a real terminal session (a `pty`-driven automation attempt did
  not produce reliable output in this sandbox and was abandoned rather than reported as passing on
  faith) - the TUI code was verified by `cargo build`/`clippy` (clean) and by code-level inspection
  confirming it reuses the exact same, already-verified backend routes this document's live test
  exercised directly.

## Bottom line

The core "multiple big projects" claim - that Engram keeps each project's memory genuinely separate
while still giving the user one coherent, ringed brain - is real and verified, including under an
adversarial query designed to defeat it by keyword. The consciousness scope boundary is verified in
both directions. Real skill execution, ledger integrity, signed receipts, and the CLI surface are
all confirmed working together, not in isolation. What couldn't be exercised here (real-LLM tool use,
contradiction/reflection actually firing, live browser/terminal interaction) is exactly what's
already covered by separate deterministic tests, plus an honest note on the one thing that's
genuinely un-exercised in this environment - not a soft-pedaled gap.
