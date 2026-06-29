# Engram coverage roadmap — Hermes audit → ultimate coverage

> Produced by a 23-agent audit: every Hermes tool group + all 18 skill packages (incl. `computer_use`)
> were read from source, mapped to Engram, then run through a product-owner / developer / architect
> triad + an adversarial challenge loop. Companion to [WEB-CAPABILITIES.md](WEB-CAPABILITIES.md).

## TL;DR — the reframe

**Reliability beats verticals.** The biggest wins are not new integrations (email, Discord, …) but
the run-loop primitives that stop *any* task dying midway: a fuzzy edit engine, a clarify tool, search
snippets, an in-session todo, output overflow-to-disk, and the approval gate. Adding 40 integrations on
a loop that thrashes on whitespace or blows its context window just multiplies the failure surface.

**Engram's edge is trust, not feature count.** Hermes ships a capability and asks the model nicely not
to misuse it (regex at the display layer, prose "always confirm", fail-open auto-downloaded binaries,
plaintext `.env`). Engram ships the *same* capability behind a **structural gate**: untrusted content
is tainted at ingestion and physically cannot cross the egress+code-exec trifecta; every skill is
capability-scoped by an Ed25519 signature; secrets live in the Keychain; every consequential action is
a signed, replayable ledger entry. The discipline that keeps this from becoming "refuses everything":
ship **taint de-escalation** (human approval clears taint, provenance-scoped) *with* the gate.

## Correction — what Engram already has (audit under-credited)

The audit brief under-listed Engram's tools, so a few rows are marked "missing" that in fact exist:

| Audit said | Reality |
|---|---|
| in_session_todo_list (missing) | `UpdatePlanTool` exists ([lib.rs](../crates/engram-agent/src/lib.rs)) — improve, don't build |
| Content/file search (missing) | `GrepTool` + `GlobTool` exist — improve to structured matches |
| TTS / Transcription (missing) | `TextToSpeechTool` + `TranscribeTool` exist |
| Sub-agent delegation (missing) | `DelegateTool` exists (depth-bounded, taint-propagating) |
| Office/doc extraction | PDF/DOCX/XLSX ingest exists (desktop build) — already broader than Hermes |

Net: Engram's true gaps are narrower than 62 rows imply — concentrate on the P0 reliability set below.

## The 80/20 build order (ranked)

0. **Pull the real failure trace first** (investigation, no build). The whole program is justified by
   one failed flight run. Confirm the root cause (now structurally fixed by the `flight_search` skill)
   before over-investing — one run is an anecdote.
1. **Fuzzy find-and-replace + did-you-mean** — *in-tree Rust*. Highest-leverage reliability lever; stops
   every edit journey thrashing on whitespace/indent drift. `edit_file` already rejects ambiguous
   matches; add whitespace-tolerant fuzzy strategies + suggestions. Touches the edit path — treat as
   surgery, not a feature.
2. **Clarify tool** — *in-tree Rust* (typed, ≤4 choices). Cheapest fix for the ambiguous-request death
   class (the original flight ask had two origins + vague dates and should have asked). Records the
   answer to memory so future runs don't re-ask.
3. **Search snippets + finish readability** — *improve web stack*. Readability is **done** (this
   session); still pending: snippets in `web_search` (today it returns title+url only, forcing a
   `web_fetch` per result). Low blast radius, underpins every research journey.
4. **in_session_todo** — *improve `UpdatePlanTool`*. Re-inject after compaction so long tasks survive
   truncation; it already surfaces in the kanban UI.
5. **Overflow-to-disk + turn budget** — *in-tree Rust (reuse the artifacts subsystem)*. A single huge
   tool output blows the context window and kills the run; spill to a signed re-readable artifact.
6. **Taint de-escalation + approval/clarify gate** — *security core*. Approval gating unlocks safe
   egress for everything below — **but ship de-escalation in the same stroke** or the trifecta becomes
   the #1 "Engram refused the thing I asked" complaint and users disable it.
7. **Interrupt / cancel** — *in-tree Rust*. `browser_*`/`shell`/`skill_run` can't be cleanly stopped;
   the product feels uncontrollable without `/stop`.
8. **email (himalaya)** — *signed Process(bash) skill*. The one new vertical worth doing now; showcases
   the taint moat (injection-over-email can't drive compose+exfil). Honest caveat: the binary is small,
   but account onboarding (app-passwords / Gmail-OAuth / TLS) is the real work.

### Status after this session (P0)

| # | Item | Status |
|---|---|---|
| 1 | Fuzzy find-and-replace + did-you-mean | ✅ built (`edit_file`, 3 tests) |
| 2 | Clarify tool (typed, ≤4 options, audited) | ✅ built (`clarify`, 2 tests) |
| 3 | Search snippets (+ readability) | ✅ both built (snippets + readability, tests) |
| 4 | in_session_todo | ◑ partial — `update_plan` now re-renders the full checklist each call (survives compaction in practice); full compaction-pinning is a follow-up |
| 5 | Overflow-to-disk | ✅ built (`spill_if_large`, 2 tests) — turn budget already exists (`token_budget`) |
| 6 | Taint de-escalation + approval gate | ✅ **fully built incl. UI** — `Policy.approved` de-escalates the egress trifecta (daemon-set ONLY; the model can't grant its own), every approved egress is audited (`agent.egress_approved`), and a `request_approval` tool lets the model *ask*. The daemon round-trip is wired: `/v1/tasks/{id}/run[/stream]?approved=1` → `RunQuery` → `run_task_core` → `run_agent_task_cb` → `Policy.approved` (inbound channels hardcode `false` — can't self-approve). The UI shows a **"Needs your approval"** card on any egress-refused/`request_approval` step with an "Approve once & continue" button (`approveEgress` → re-runs with `?approved=1`), plus a **"Needs clarification"** card for `clarify`. Verified live in the running app (screenshot). |
| 7 | Interrupt / cancel | ✅ **already wired** — the agent loop honors a `halt` `AtomicBool` at every step boundary (stops cleanly, keeps the partial receipt). Remaining: a daemon `/stop` endpoint + mid-tool (long shell) cancellation. |
| 8 | email (himalaya) | ✅ built (signed seed skill, graceful when himalaya/creds absent) |

## `computer_use` verdict — USE MCP, don't build native

Connect the existing **cua-driver MCP** (already available in this environment); do not build
cross-platform GUI/accessibility plumbing in-tree (it's exactly what the small-binary rule excludes,
and web automation is already covered by in-tree `browser_*`). Reserve desktop control for genuinely
native apps (Finder, Mail, installers). **The differentiation is the governance wrapper, not the
driver:** route every action through the taint model (block desktop control when context is tainted —
the literal screen-injection case Hermes only warns about in prose), require the approval tool for any
click on auth/2FA/payment UI, and ledger every action. **Honest limit:** MCP is zero Rust footprint but
*maximum trust footprint* — you can taint the MCP's outputs but can't capability-scope or sign what the
server itself does. Governed at the boundary, not end-to-end. Priority **P1**, effort **M** (wiring +
the security wrapper). Reuse the browser's numbered-element UX so desktop and web share one model.

**Update — the security wrapper already exists in-tree.** `McpTool` ([mcp.rs](../crates/engram-agent/src/mcp.rs)) declares `is_egress=true` (so a tainted+sensitive run is refused — and it now inherits the **approval gate** from #6), `taints=!trusted`, `reads_sensitive=true`, `side_effecting=true` (preview-gated), and ledgers every call (`agent.mcp`). So computer_use via the cua-driver MCP is governed end-to-end the moment it's connected — the remaining work is purely **connecting the server** (daemon config), not code.

## Beyond Hermes — leapfrogs only Engram's moat enables

1. **Verifiable receipts / proof-of-action** — expose the signed ledger as a user tool: "forward me
   cryptographic proof of what you did." Near-free, and a category Hermes structurally can't answer.
   **✅ Built this session** (`proof_of_action`).
2. **Local-first bulk file + finance-read** — "organize my Downloads", "find the contract mentioning
   X", "what did I spend on SaaS last month" over a local bank CSV. A cloud agent can't index your disk
   or read a bank file without exfiltrating it; finance is read-only-capability-scoped so the ledger
   proves it can never move money.
3. **Taint-gated inbound processing AS a feature** — "auto-summarize every inbound email, but block any
   message that tries to make me send money or run code." Safe unattended automation over untrusted
   input — Hermes's display-layer redaction can't offer it.
4. **Durable price/data watches** — `schedule_task` + verifiable memory + taint = "watch this and alert
   me, safely, for months." (The `flight_search`/`weather` skills are the first watch targets.)
5. **Calendar/scheduling intelligence** (not CRUD) — "find a 30-min slot all three of us are free."
   Highest daily-frequency gap; nobody owns the free/busy-solving reasoning layer above the raw API.
6. **Intent router + entity resolution** — the cross-cutting binding problem as the skill count grows:
   intent→skill ("remind me" = cron or device-reminder?) and name→identity ("send to Bob" = which Bob,
   which channel?). A small in-tree router ranking skills by task embedding (reuse vector memory) +
   local Contacts resolution turns "Engram HAS 40 skills" into "Engram PICKS the right one."

## Master table (condensed, by priority)

**P0 (reliability core):** fuzzy edit engine · search snippets · finish readability *(done)* · email
skill · clarify tool · approval gating + hardline floor · in_session_todo · output overflow-to-disk ·
taint→trifecta enforcement.

**P1:** read pagination/line-meta · structured ripgrep search · SSRF guard hardening · browser
aria-refs/get_images/console · browser→VLM auto-fallback · unified send_message (Slack/Telegram/SMTP) ·
imessage · Google Workspace *(via MCP)* · github cluster skill · interrupt/cancel · MCP taint-tagging ·
**computer_use via MCP**.

**P2 (32):** V4A multi-file patch · post-edit lint · website allow/deny policy · Camoufox *(MCP)* ·
X/Discord clients · Microsoft 365 · Notion/Airtable *(MCP)* · obsidian/apple-notes · capability-scoped
delegate · advanced cron · editable named-fact memory · verbatim conversation FTS · skill patch-mode ·
remote skills hub · progressive tool disclosure · persistent terminal/pty · env_probe/passthrough ·
image-to-image · local Whisper STT · Piper TTS · kanban dispatcher · methodology skills (TDD/debug) ·
research skills (arxiv/maps/youtube) · creative skills (diagram-as-code/infographic).

**Skip:** Feishu/Yuanbao (regional) · shadow-git checkpoints · jupyter kernel · video/voice gen ·
managed-tool-gateway (anti-positioning) · ephemeral execute_code promotion · dedup loop-breaking.

## Built this session (web-reliability + trust + P0 reliability set)

| Deliverable | Kind | Status |
|---|---|---|
| `flight_search` skill (Travelpayouts/Amadeus) | SKILL | ✅ signed seed, tested |
| `weather` skill (Open-Meteo, keyless) + API template | SKILL | ✅ live-verified, tested |
| `email` skill (himalaya wrapper, taint-gated send) | SKILL | ✅ signed seed, graceful path verified |
| Minimum-viable browser stealth (webdriver mask + real UA) | TOOL | ✅ compiles (browser-cdp) |
| Readability extraction (dependency-free) | TOOL | ✅ 5 tests |
| `proof_of_action` — verifiable receipts (leapfrog #1) | TOOL | ✅ tested |
| Fuzzy `edit_file` engine + did-you-mean | TOOL | ✅ 3 tests |
| `clarify` tool (typed, ≤4 options, audited) | TOOL | ✅ 2 tests |
| `web_search` snippets (DDG + Brave) | TOOL | ✅ tested |
| Observation overflow-to-disk (`spill_if_large`) | CORE | ✅ 2 tests |
| `update_plan` full-checklist render (compaction-resilient) | TOOL | ✅ tested |
| Approval gate + taint de-escalation (`Policy.approved`, audited) | CORE | ✅ 2 tests (boundary preserved) |
| `request_approval` tool (model asks, never grants) | TOOL | ✅ tested |

Verification: `cargo test -p engram-agent --lib --features web` (60 green) · `cargo test -p engramd
--bin engramd` (35 green) · `cargo clippy` clean · `cargo build --workspace` clean.
