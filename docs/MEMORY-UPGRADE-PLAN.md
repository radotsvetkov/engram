# Engram Memory System — SOTA Upgrade Plan

*The output of a 20-agent adversarial pressure-test: research against mem0, LangChain/LangGraph,
MemGPT/Letta, Zep/Graphiti, Generative Agents, A-Mem, RAPTOR, Claude's memory tools, and Hermes'
Curator, run in parallel with six adversarial audits of the actual Engram code, then three
independently-designed candidate architectures, each scored twice by adversarial judges, then a
completeness critic that cross-examined all of it. This document is the reconciliation — one plan,
not three. Every claim below is either confirmed against the running code (file:line) or explicitly
flagged as disputed/deferred. Scope gate, unchanged from `docs/ROADMAP.md`: **does this produce a
signature a competitor can't?** If a line item doesn't deepen verifiable memory, verifiable
expertise, or verifiable dissent, it doesn't belong here.*

## 0. Verdict in one paragraph

Engram's memory system is architecturally sound and mostly ahead of where a first look suggests —
scope isolation is a real SQL-level filter with tests, ledger-first writes are near-universal, and
taint discipline is strong in the main agentic loop. But it is running on a **placeholder embedder**
that a fully-built, unused local alternative already fixes; it does **zero active reasoning** over
memory beyond exact-match dedup and a 3-rule prefix whitelist; long-task continuity is **destructive
compaction with no write-back**; and the CLI/TUI — the terminal-first persona this product explicitly
targets — **cannot drive or even see** the skill-improvement loop that is Engram's single named
differentiator. None of this requires new infrastructure, an external database, or abandoning the
single-SQLite-file pitch. It requires finishing what's already half-built.

---

## 1. What's already real (don't re-litigate this)

Confirmed by direct code audit, not assumed from `docs/STRATEGY.md`'s prose:

- **Scope isolation is a genuine SQL filter**, not a convention: `scope_clause()` builds a real
  `WHERE` predicate unioned across user/project/session rings, exercised inside the recall query
  itself, with explicit isolation tests (`store.rs:1307-1360`). The prior cross-project bleed bug
  looks fixed at the root.
- **Ledger-first writes are near-universal**: `remember`, `forget`, `supersede`, `promote_to_user`,
  `restore`, `consolidate` all append to the signed ledger before mutating. Two confirmed exceptions
  are derived/index state, not content (`backfill_binary()`, and the per-recall access-count bump —
  both fixed below).
- **Taint discipline is strong where it's been applied**: the agentic loop's memory tools hard-refuse
  on untrusted taint, and the flywheel auto-capture is a model exemplar (gate + still stamp taint).
  It just isn't applied *everywhere* yet (§4).
- **The hybrid FTS5 + vector-cosine + RRF recall path is a legitimate, competitive fusion retriever**
  for its class — the gap is what feeds it (the embedder), not the fusion logic itself.
- **A real local, pure-Rust, dependency-free semantic embedder already exists and is fully wired**
  (`crates/engram-memory/src/static_embed.rs` — a hand-rolled model2vec implementation with its own
  tokenizer and safetensors parser) — it's just not the default. This is a much shorter path to "real
  local embeddings" than it looks from the outside.
- **`dissent.rs` is a proven pattern**: recall candidates, number them, force citations, strip
  hallucinated ones, sign the result. This is the anti-decorative-theater template every new
  "intelligence" feature below reuses rather than inventing a second, weaker verification scheme.

---

## 2. A claim we did not carry forward unverified — RESOLVED 2026-07-05

One of the three candidate designs justified its highest-priority backend item — restructuring
`scope_clause()`'s union-of-rings query — with a "confirmed 40x scan amplification at 40 projects ×
10k rows" benchmark. A second candidate design adopted the same claim without independently
re-checking it. Neither number came with a checked-in reproduction.

**Task 0 is done.** `crates/engram-bench/src/bin/scale_bench.rs` builds the brain via the real
`Memory::open` (byte-identical schema/indexes to production), bulk-inserts 40 projects × 10,000 rows,
and runs `EXPLAIN QUERY PLAN` plus real wall-clock timing on the exact SQL text (parens and all —
the first draft of this benchmark itself had an operator-precedence bug from dropping
`scope_clause()`'s outer wrapping parens, which silently changes `AND`/`OR` grouping; fixed before
trusting the result). Full method and output: `crates/engram-bench/SCALE-BENCHMARK.md`.

**The claim does not reproduce, for either query.** SQLite already applies `MULTI-INDEX OR` against
`idx_facts_scope` for `scope_clause()`'s union-of-rings predicate — a single project's recall touches
only that project's ~10,001 in-scope rows (3.7ms) out of 400,001 total (257.8ms unscoped), not the
whole region across every project. `current_with_prefix_scoped()`'s `LIKE`-based query — independently
flagged as unable to use the composite index "regardless of the outer benchmark's outcome" — **also**
uses the same `MULTI-INDEX OR` plan: SQLite satisfies the scope predicate via the index first, then
applies `region = ?` and `text LIKE ?` as residual filters over the already-small candidate set.

**Conclusion: no scope-index restructuring work of any kind is needed.** Both disputed claims are
dropped from this plan, not implemented — verified, not assumed. This section stays as a record of
the process, since it's the one place two independent design efforts converged on the same
unverified number and it would have been easy to just trust the corroboration.

---

## 3. Named gap disposition

Five strategic gaps were already on record before this audit. Two got zero design coverage across
all three candidate architectures — that's a real hole, not a prioritization call, so it's decided
explicitly here rather than silently dropped:

| # | Gap | Disposition | Why |
|---|---|---|---|
| 1 | Trigram-hash placeholder embedder | **In this plan — Phase A** | A real fix already exists in-tree; this is a default-flip + packaging problem, not an unsolved one. |
| 2 | No validity windows / supersession-as-overwrite | **In this plan — Phase B** | `superseded_by` already exists; additive columns get us to Zep-parity cheaply. |
| 3 | Taint enforced at egress, not persist | **In this plan — Phase A** | Two concrete, confirmed live holes (`corpus.rs`, `converse.rs`); reuses the existing `Taint::join` primitive. |
| 4 | No implicit preference-learning loop (opt-in + notification) | **Deferred, explicitly** | Needs its own consent-UX design (the brief's own guardrail: silent inference about the user needs an opt-in and a per-update notice, not just a ledger entry) — folding it into this pass would dilute review of the memory-correctness work above. Revisit after Phase B ships and the confirmation-UI pattern (§5) exists to reuse. |
| 5 | Key custody (verify without trusting the host) | **Deferred, explicitly** | This is a cryptographic/infra decision (HSM, split-key, remote co-signing, or an honestly-stated threat-model boundary) orthogonal to memory-quality work — it's a ledger-signing question, not a memory-schema question. Track separately; do not let it block this plan. |

---

## 4. Two SOTA techniques we're deliberately not building yet

The research surfaced five techniques with no flat-similarity-search equivalent (Zep's bi-temporal
graph, MemGPT's paging, Generative Agents' reflection, A-Mem's link+evolution, RAPTOR's summary
tree). Three are in this plan (bi-temporal versioning, paging, reflection — scoped down from their
original form). Two are explicitly cut for now, with a cheap groundwork hook left for later:

- **RAPTOR (recursive tree-of-summaries)** needs GMM/UMAP clustering with no native-Rust story — the
  honest options are a Python-skill subprocess or a much cruder approximation, and neither is worth
  it before reflection (below) even ships once. **Groundwork only:** when the reflection pass writes
  a synthesized fact, give it a `parent_id`/`tree_level` pair pointing at its sources now, so a real
  tree can be layered on later without a second schema migration.
- **A-Mem (Zettelkasten link + retroactive evolution)** is a genuinely different capability
  (memories that rewrite each other's framing) from anything else here and deserves its own design
  pass, not a bullet point bolted onto reflection. Not in this plan.

---

## 5. Locked decisions (where the two real design candidates disagreed)

Two independently-generated designs converged heavily — good corroborating signal — but diverged on
specifics that must be one answer, not two parallel code paths:

- **Bi-temporal migration path:** extend `supersede()` **in place** to also stamp
  `valid_from_ms`/`valid_until_ms` (not a new parallel `supersede_with_validity()` function). One
  supersession code path, not two.
- **Supersession confirmation policy:** once contradiction-detection replaces the 3-rule prefix
  whitelist, there is **no silent-auto-confirm mode**. A detected contradiction always produces a
  `pending_supersessions` row surfaced for accept/reject. An opt-in "silent mode" would re-introduce
  the exact unverifiable-silent-overwrite failure this feature exists to fix — that defeats the
  point, so it isn't offered.
- **Confirmation-UI copy must not overclaim:** contradiction-detection's citations prove "the model
  looked at these specific rows," not "the model is correct that they conflict" — unlike `dissent.rs`,
  which grounds in a hard, checkable replay-win score. The UI must read as *"possible conflict, your
  call"*, never as an assertion with dissent's evidentiary weight.
- **Reflection's "clustering" is not RAPTOR-style clustering.** No clustering primitive exists in
  the codebase, and inventing one (GMM/UMAP, or even a from-scratch greedy grouping) to serve a single
  hourly-tick feature is disproportionate. Reflection instead runs over the **small, already-bounded
  candidate set the consolidation tick already fetches** (warm→cold demotion candidates in a single
  region+scope), doing a simple pairwise-cosine greedy grouping over that bounded set (tens of rows,
  not the whole brain) — no new infrastructure, no new dependency.
- **One CLI verb for time-travel queries:** `engram memory recall --as-of <date>`. (Not a separate
  `History <id>` shape in the TUI and a different one in the CLI — same concept, same name, same
  flag across surfaces; TUI gets a keybinding that maps to the same query.)
- **The Tauri desktop app needs no separate work.** It wraps the same `crates/engramd/assets/index.html`
  — every "desktop" item in this plan applies to it automatically. Stated explicitly so it doesn't
  read as an unaddressed surface.
- **A `pending_supersessions`-style entailment gap applies to reflection too:** citation-presence
  alone is necessary but not sufficient — an LLM can cite three real, unrelated facts and still draw
  an unsupported leap. Reflection's synthesis prompt must require the model to state, per cited fact,
  *what specifically it contributes* to the conclusion, not just list source IDs; reviewers (human,
  in the Reflections UI) see the per-fact justification, not just a citation list.

---

## 6. The plan

### Phase A — Fix the foundation (no new surface, no new roadmap phase; hardens Phase 1.5's already-shipped promises)

**Backend**
1. **Flip the default embedder from trigram to the existing static model2vec embedder**
   (`crates/engram-memory/src/static_embed.rs`, already fully wired into config/UI/migration — this
   is a default flip + model packaging problem, not new engineering). Fall back to trigram, visibly,
   only when the model file is genuinely unavailable offline.
2. **Commit a real recall@10/MRR benchmark** (`crates/engram-bench` already has the harness; it's
   just never run in CI) proving the flip is a real improvement, not an assertion.
3. **Batch `migrate_embedding_space`** (`store.rs:224-282`) so the mandatory re-embed on the default
   flip doesn't stall every existing installed brain synchronously with zero progress feedback —
   chunked transactions + a resumable cursor + a `/v1/status` progress field. **This ships in the
   same release as item 1, not after it.**
4. **DONE (2026-07-05, `26559d3`):** `Embedder` grew a default `embed_checked()` method that
   reports when a call silently degraded (default: never); `GatewayEmbedder` overrides it to flag
   exactly the fallback its own `NEEDS-INTEGRATION` comment named. `remember()` stores this as a new
   `needs_reembed` column, and `Memory::reembed_flagged()` repairs flagged rows once the embedder is
   healthy again — wired into the existing hourly consolidation tick. Verified with two deterministic
   `engramd` unit tests (a dimension-mismatching `MockProvider`, no network flakiness) and one
   `engram-memory` test proving the flag sets, survives a still-degraded repair attempt, and clears
   once healthy.
5. **DONE (2026-07-05, `26559d3`):** `GET /v1/memory/stats` now returns `embedder_configured` /
   `embedder_active` / `embedder_degraded`, turning the silent `tracing::warn!` into a fact every
   surface can read. (No dedicated `/v1/status` route exists — `memory_stats` is the natural home
   since this is specifically about the embedder, and it's what the desktop's "Memory & context"
   pane already polls.) The desktop/TUI/CLI badges *displaying* this are still open — see the
   parity section below.
6. **Persist-time taint holes — `converse.rs` DONE, `corpus.rs` deliberately NOT changed as
   originally scoped:**
   - **DONE (2026-07-05, `<pending commit>`):** `converse.rs`'s legacy conversational path
     (`converse`/`converse_stream`) hardcoded `Taint::Trusted` on both the completion call and the
     stored reply regardless of attachment content, even though `Attachment`'s own doc comment
     already called attachments "otherwise untrusted input." Fixed: any attachment other than an
     already-vetted pinned memory now taints the turn `Untrusted`, matching the agentic loop's
     belt-and-suspenders pattern. Verified with two new tests (an untrusted-attachment turn stores
     an `untrusted`-tainted reply; a plain turn keeps the existing `trusted` default).
   - **`corpus.rs:9-13`'s hardcoded `Taint::Trusted` on every uploaded-document chunk turned out to
     be a documented, reasoned design choice, not an oversight** — its own comment explicitly weighs
     the single-user-local trade-off ("the user deliberately brought this file into their own
     project... a shared/multi-tenant deployment would default these untrusted"). Overriding a
     considered prior decision under a blanket "default to Untrusted" rule (as originally scoped
     here) would be a real product-UX change — STRATEGY.md's P5/gap-#6 language wants this category
     closed, but the *right* mechanism (an explicit consent toggle vs. always-Untrusted vs. leaving
     it as-is) is a product call, not something to decide unilaterally mid-implementation. Left
     un-touched; flagged for an explicit decision before changing it.
7. **Recall-surface document filter — narrower than originally scoped, needs a framing fix, not a
   blanket exclusion.** `conscious.rs`'s `is_doc` filter (lines 140-151, 358-376) excludes document
   chunks from the two *always-loaded, framed-as-authoritative* consciousness blocks specifically
   because that framing is what makes injected content dangerous ("attacker text inside a
   merely-uploaded document would become trusted instruction" — the code's own comment). The three
   broader surfaces (`memory_recall` tool, the flywheel's per-turn context, the recall ribbon) are
   *retrieved reference material*, not always-loaded authoritative fact — which is corpus.rs's whole
   documented purpose for ingesting documents at all (`ingest_document`'s doc comment: "usable
   reference material"). Blanket-excluding document chunks from these three surfaces would defeat
   that purpose, not fix a bug. The real remaining gap is narrower: when a document chunk IS
   surfaced there, it should be clearly labeled as "content from an uploaded document" (the way
   `attachments_context` already explicitly primes the model to treat attachment content as
   untrusted reference, not instructions) rather than blended in as an unqualified fact — a prompt/
   labeling fix at the three call sites, not a filter. Not yet implemented.
8. **Ledger the two remaining I1-invariant bypasses**: batch `recall_inner`'s per-hit access-count
   bump into one `memory.access_batch` entry per minute (this field is the sole input to
   consolidation's demotion decision — it shouldn't sit outside the signed history); document
   `backfill_binary()`'s exemption as intentional (pure derived index state). Not yet implemented.

**Desktop / TUI / CLI (parity work — pure client plumbing against routes that already exist; no backend design needed, land any time, ideally first since it's the cheapest win in the whole plan)**
- **DONE (2026-07-05, `055e648`):** wired the four already-shipped skill routes
  (`skill_improve`/`teach`/`revert`/`activate`) into the CLI (`engram skills improve/teach/revert/
  activate`) and fixed both the CLI's `skills show` and the TUI's skill detail pane to print the real
  `incumbent_score`/`candidate_score`/`replays` numbers instead of a bare event count. Added
  `engram memory identity-edit/identity-add/identity-remove/identity-revert` against the existing
  `/v1/consciousness/*` routes, and TUI provenance display (`from memory #<id>` / `user-authored` /
  `pinned`) on consciousness lines. Verified end-to-end against a live daemon, not just `cargo test`.
  TUI keybindings for triggering improve/teach interactively (vs. CLI's argument-based flow) are
  still open — the TUI can now *display* real scores and provenance but not yet *drive* improve/teach
  through an interactive modal; that's a small follow-on, not a design gap.
- Add a text search/filter box to the desktop Recent-memories panel (skills got one; memory didn't —
  an oversight, not a deliberate cut) and to the TUI (which has neither today).
- Add an embedder-health badge ("Configured: X — Active: Y") to Settings (desktop), the TUI, and
  `engram status --json` (CLI) — turns a silent daemon-log-only degradation into a visible fact on
  every surface. (Depends on the embedder default-flip work below, which introduces the fallback
  state this badge needs to surface.)

---

### Phase B — Truth over time (extends Phase 1.5's "searchable/filterable memories" with real temporal semantics)

**Backend**
1. **Bi-temporal fact versioning**: add `valid_from_ms`, `valid_until_ms` to `facts`, extending
   `supersede()` in place (§5's locked decision). Add `Memory::recall_as_of(query, regions, k, scope,
   as_of_ms)` as an additive filter alongside today's default (current-only) recall path — verify
   explicitly that the default path still sees exactly one current version per fact across all three
   recall arms once historical rows persist instead of disappearing.
2. **Replace the 3-rule literal-prefix supersession whitelist** (`converse.rs`'s `RULES` table,
   Identity-only) with embedding-similarity-gated contradiction detection: on every Identity/Semantic
   write, check for a high-cosine-but-not-identical prior row in the same region+ring; if found,
   write a `pending_supersessions` row (never silently apply — §5) using `dissent.rs`'s exact
   citation-and-strip-hallucination discipline, generalized into a small shared helper so both
   `dissent.rs` and this feature use one proven verification implementation instead of two ad hoc
   ones.
3. **Add an `agent` scope-kind ring** (a new `scope_kind` value alongside `user`/`project`/`session`)
   so that once multiple durable named `AgentDef`s (Phase 2's already-shipped primitive) work a
   project concurrently, their memory writes are attributable and filterable per agent — without
   this, "verifiable expertise per named agent" has no way to actually attribute a memory to the
   agent that produced it. This is the one gap all three design candidates and every judge pass
   missed entirely; mem0's own scope model names `agent_id` as a standard dimension for exactly this
   reason.
4. Add `scope_kind`/`scope_id` grouping to `Stats` (`store.rs:965-977`, currently global-only) so a
   per-project (and now per-agent) memory breakdown becomes queryable on every surface.
5. **Bridge the two disconnected "procedural memory" stores**: on a `skill.promote` ledger event,
   write a `Region::Procedural` memory record referencing the skill id/version/replay-score (scoped
   `user`, or the new `agent` ring if promoted by a specific named agent). On `skill.revert`,
   invalidate that record via the same `valid_until_ms` mechanism rather than leaving it stale. This
   is the direct, low-cost fix for "procedural memory is dead in practice" — it doesn't invent a new
   verification signal, it surfaces the one that already exists (the skill registry's replay/promote
   loop) through the same API consciousness, recall, and future dissent-grounding already use.
6. **Make `consolidate()`'s `tier` field do something**: today `tier='cold'` is written every hour and
   read nowhere — a label with zero retrieval effect, exactly the decorative-theater pattern the
   product's own biggest named risk warns against. Add a small RRF deprioritization (not exclusion)
   for cold-tier rows.
7. **Add the missing third of the "sleep cycle" triad — automatic, conservative, reversible
   forgetting** (opt-in, off by default, same pattern as `auto_distill_skills`): memories that are
   simultaneously already-superseded (per the new `valid_until_ms`), older than a long window, and
   never once recalled, get soft-forgotten via the existing `forget()` — no new deletion mechanism,
   just a new automatic trigger for one that's already reversible and ledgered.

**Desktop / TUI / CLI**
- Fact-history view: a "History" expander on a memory's detail view (all three surfaces) showing its
  validity timeline and supersession chain.
- A `pending_supersessions` inbox (all three surfaces): old fact / new fact / model's cited reason /
  accept / reject — the visible half of turning silent auto-supersede into a confirmable event, and
  the same UI language `pending_preference` updates (gap #4, when it's picked back up) should reuse.
- `engram memory recall --as-of <date>` (CLI, with a TUI keybinding mapping to the identical query —
  §5's locked single verb).
- Per-project *and* per-agent memory breakdown once the `Stats` grouping lands.
- Make the recall ribbon's ledger link real everywhere: today `recallOpen()` only re-focuses a graph
  node — add a modal/pane showing the actual signed ledger entry (hash/sig/seq), on desktop, TUI, and
  as `engram ledger show <seq>` on CLI.
- **Fix the per-project persona correctness bug as its own owned task, not a caveat**: `agent_handler`
  (the live desktop chat path) never reads `persona_for_session` — only the legacy `/v1/converse`
  path does. A user editing "project persona" in Settings today gets zero effect on real chats with
  no warning. Either wire `persona_for_session` into the live path, or retire the field in favor of
  consciousness's per-project block (which *is* wired in) — do this **before** adding any CLI/TUI
  control for it, so no surface ships a working editor for a dead setting.

---

### Phase C — Long-task continuity (the backbone for Phase 2's durable named agents)

This phase exists because retrieval quality (Phase A/B) does nothing for a long-running mission —
today, `agent.rs`'s `maybe_compact` is **destructive**: on overflow it replaces the transcript with an
LLM summary and discards the original detail permanently, with no write-back to memory. A multi-day
mission re-discovers almost everything each time it compacts or crosses a task-card boundary. This is
a load-bearing gap independent of everything else in this plan, and it doesn't get fixed unless it's
built explicitly — carry it forward regardless of how the rest of the plan gets prioritized.

**Backend**
1. **Give compaction a durable exhaust**: before `maybe_compact` (`agent.rs:795-887`) discards the
   pre-tail slice, write it as one or more `Region::Episodic` rows tagged with the task/mission id and
   a monotonic `page_seq`, taint inherited from the run. This is the MemGPT-style "page out" half —
   nothing is lost, it's evicted to a place that can be paged back in.
2. **Add a page-in tool**: `memory_recall_page(task_id, page_seq)` — fetch a specific evicted page
   verbatim by id, alongside the existing similarity-based `memory_recall`. Update the `agent.compact`
   ledger entry to include the new memory ids it just wrote, closing the forensic gap where today only
   token *counts* are logged, not what was elided or where it went.
3. **Granular mission breadcrumbs**: replace the single ~440-character post-run episodic sentence
   with additional per-plan-milestone episodic writes, triggered off plan-step completions — a long
   mission leaves a chain of timestamped breadcrumbs instead of one lossy final line.
4. **Extend cross-run mission relay** beyond today's single-hop, 4000-character truncated prior
   answer: also inject a scoped recall of the mission's breadcrumbs, so hop N+2 can see hop N's detail
   again, not just hop N+1's truncated answer.
5. **Do not extend `run_mission`'s concurrent ephemeral fan-out.** It's the one place in the codebase
   that already looks like the rejected ephemeral-swarm pattern (ungoverned `tokio::spawn` fan-out with
   a generic "mission" actor, no durable per-subtask identity). This plan does not add capability to
   it; long-task continuity is built on the durable task-card/kanban primitive instead, consistent
   with Phase 2's named-agent model.

**Desktop / TUI / CLI**
- A visible "earlier steps paged to memory — click to view" marker in the chat surface when
  compaction fires, on desktop and TUI, so what was previously invisible context loss becomes a
  visible, recallable event.

**Sequencing note:** land this *before or alongside* Phase 2's kanban/named-agent work, not after —
durable agents doing multi-day work are exactly what needs paged, evictable-and-recallable context,
and Phase 2 will need it immediately.

---

### Phase D — Grounded reflection (a Phase 3 companion to dissent, not a new mechanism)

The one genuinely new reasoning capability in this plan. Ships **last**, **opt-in, default off**,
and only after the citation-verification helper (§6 Phase B, item 2) is extracted and proven on the
lower-stakes contradiction-detection feature first.

**Backend**
- Extend the existing hourly consolidation tick: when it finds a small, bounded, co-scoped candidate
  set of Trusted-only facts (the pairwise-cosine greedy grouping described in §5 — no new clustering
  infrastructure), make exactly one bounded LLM call using `dissent.rs`'s exact pattern: list
  candidates numbered, require the model to state what each cited fact specifically contributes, drop
  the output entirely if any claim isn't grounded. Write the result as a new `Region::Semantic` row
  whose metadata stores the source fact ids + their ledger sequences, plus the `parent_id`/`tree_level`
  groundwork noted in §4 for a possible future RAPTOR-style layer.
- Never fires on Untrusted-tainted inputs. Config-flag gated, matching the existing
  `auto_distill_skills` opt-in pattern.

**Desktop / TUI / CLI**
- A "Reflections" view distinct from ordinary memories, permanently — not just at creation time —
  showing each synthesized fact with its cited sources as clickable chips. A reflection fact must
  never be visually indistinguishable from a directly-witnessed one, at recall time or in the UI, on
  any surface, for as long as it exists. This is the one feature in the whole plan closest to the
  product's own named biggest risk (decorative intelligence eroding the trust pitch) — the permanent
  visual/data distinction is not optional polish, it's the thing that keeps this feature off the cut
  list.

---

## 7. Sequencing summary

```
Task 0 (DONE)  →  scope-index benchmark re-verified against current schema; claim did not
                  reproduce (crates/engram-bench/SCALE-BENCHMARK.md); no fix needed, dropped

Phase A (foundation, no new UI surface)
  embedder default flip + migration batching + benchmark
  persist-time taint closure (corpus.rs, converse.rs) + recall-surface filter fix
  ledger the two access-bump bypasses
  ── in parallel: CLI/TUI skill + consciousness parity (cheapest win, no backend dependency) ──

Phase B (truth over time)
  bi-temporal versioning + contradiction-detection-with-confirmation
  agent scope ring + procedural-memory bridge (skill.promote → Region::Procedural)
  tier-scoring fix + opt-in conservative auto-prune
  fix the inert per-project-persona bug (owned task, before any CLI/UI control for it)

Phase C (long-task continuity) — build alongside/before Phase 2's kanban work, not after
  paged working set (page-out on compact, page-in tool)
  granular mission breadcrumbs + extended cross-run relay

Phase D (grounded reflection) — last, opt-in, depends on Phase B's shared citation-verification helper
  hourly bounded reflection synthesis, permanently-distinguished in every UI
```

## 8. Explicit non-goals (reaffirming the existing cut list — nothing here proposes any of these)

No skills marketplace. No serverless/cloud backend. No ephemeral multi-agent swarm expansion (Phase C
explicitly declines to extend `run_mission`). No 3D/anatomical brain visualization. No external vector
database or graph-database service — every schema change in this plan is additive columns/tables
inside the existing single `brain.db` file. No new persistent-memory paradigm that isn't an extension
of a primitive that already exists and is already ledgered (`Taint::join`, `superseded_by`, the hourly
consolidation tick, `dissent.rs`'s citation pattern).

## 9. Top risks carried into execution

- **Migration-stall risk** (embedder default flip): must ship with batching in the same release, or
  every existing installed brain hits an opaque multi-minute boot stall.
- **Reflection is the highest decorative-theater risk in this plan** even with citation-verification —
  entailment (does the cited source actually support the claim), not just citation-presence, must be
  checked, and the permanent visual distinction in the UI is load-bearing, not cosmetic.
- **Contradiction-detection adds an LLM call to ordinary Identity/Semantic writes** — must be
  async/best-effort (the write completes immediately; the pending-review surfaces moments later), or
  it taxes every normal conversation.
- **Taint-default change on document uploads is a visible behavior change** (uploaded docs won't
  count as trusted until confirmed) — needs one clear one-time explanatory UI moment or it reads as a
  regression instead of the security fix it is.
- **The paged working set touches `agent.rs`'s hottest path** (every long-running turn) — needs
  replay-based testing against existing long-transcript fixtures and a feature flag back to today's
  pure-summarize behavior before it ships.
