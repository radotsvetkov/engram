# Autonomous operation — running agents for days without a human clicking "Approve"

> Reverse-engineered from Hermes (NousResearch/hermes-agent) source, then designed for Engram's
> signed-capability + taint substrate. The per-action approval gate ([WEB-CAPABILITIES.md](WEB-CAPABILITIES.md),
> [COVERAGE-ROADMAP.md](COVERAGE-ROADMAP.md) #6) is the *interactive* primitive; this is the *autonomous* one.

## The problem

The one-time "Approve once & continue" button works when you're watching. For a days-long unattended
run, nobody is there to click it — so the escape valve becomes a **deadlock**: the egress is either
blocked forever or hard-refused with no path forward. Scheduled runs prove the gap exactly: cron calls
`run_task_core(&app, &task.id, None, /*approved=*/false)` ([main.rs:905](../crates/engramd/src/main.rs:905)),
so an autonomous run **always** hits the trifecta gate with no escalation.

## How Hermes actually does it (from source)

Hermes has **no live "approve" button in the autonomous path** — that's the whole point. Five real
mechanisms (not prose):

1. **Surface routing (the keystone).** `HERMES_CRON_SESSION` reroutes the approval decision away from
   the human-blocking path; `_is_gateway_approval_context()` returns `False` for cron, so an
   unattended run never falls through to a prompt nobody will answer.
2. **Per-context standing policy, not a per-action click.** Interactive sessions use `approvals.mode`
   (`manual|smart|off`); unattended use a **separate** `approvals.cron_mode` (`deny|approve`, **default
   DENY = fail-closed**). The answer is pre-decided at config time, so days-long runs never stall.
3. **A hardline floor frozen at import.** `_YOLO_MODE_FROZEN` is read from env **once** at module load
   and frozen, so in-process prompt injection can't flip the bypass at runtime. `HARDLINE_PATTERNS`
   (`rm -rf /`, `mkfs`, fork bombs…) are refused even under yolo — a floor no toggle can lift (mirrored
   by `url_safety._ALWAYS_BLOCKED_IPS` for cloud-metadata endpoints).
4. **A persisted glob allowlist (standing approvals).** `config.yaml` `command_allowlist` is
   `fnmatch`-globbed (e.g. `podman *`) plus a per-session approved set — a pre-authorized *class* of
   action never re-prompts.
5. **Stage-don't-block + a carried budget.** Background/skill writes never block; they atomically stage
   to `pending/<sub>/<id>.json` for async review. The only thing that stops a runaway run is a shared,
   thread-safe `IterationBudget` (default 90 parent / 50 subagent, **shared** across subagents so
   fan-out can't escape it) — exhaustion is a clean audited STOP, never a crash.

**Honest caveat — and Engram's opening.** Hermes's approval gate is **shell-command-only**; it has *no*
network/message/payment egress-approval path. Its real egress control for unattended runs is at the
**OS/network layer** (squid/envoy `dstdomain` allowlist, two Docker networks), and its SECURITY.md is
blunt: "the only security boundary against an adversarial LLM is the OS." It also has no wall-clock or
dollar budget. So Hermes proves the **mechanism** (surface-routed, pre-decided, fail-closed standing
policy + frozen bypass + carried budget) but leaves the **egress-under-taint** problem — exactly
Engram's trifecta — unsolved. Engram can do it *in-process and verifiably*.

## The core insight

Per-action human approval couples a **safety** decision to a **liveness** assumption — it only resolves
if a human is watching. Move the human moment **out of the run**: the human decides **once, ahead of
time**, and signs a **policy**; the runtime then evaluates that policy deterministically for days with
no human in the loop. Replace *"is a human available to say yes?"* (liveness) with *"did a human
already sign a policy that says yes to THIS class of action, and is there budget left?"* (a pure
function of signed state). That one click becomes four primitives, layered by blast radius:

- **Pre-authorization** — a signed standing policy (allowlist of egress recipients/domains + action
  classes). Authority is now a resource the run *carries*, not a permission it *requests*.
- **Risk tiering** — **hard-block** a forbidden floor no policy can lift → **auto-proceed** what the
  signed policy allowlists → **queue** everything else. Three outcomes, all computable synchronously.
- **Budgets** — a self-depleting, signed `EgressBudget` (max actions, max spend, `expires_at`) so
  "allowed" is bounded in count, money, and wall-clock; a counter never waits on a human.
- **Async approval + trusted-source marking** — when an action isn't pre-authorized, don't block the
  multi-day run: **stage** it to a durable signed queue, **notify** via an existing channel, and let
  the run continue other work. The human approves a batch later, out of band. Taint still decides
  *which tier* an action lands in.

## Engram design — a signed `AutonomyPolicy` the trifecta gate consults

`Policy.approved` evolves from a transient boolean into a durable, signed, scoped grant.

1. **`AutonomyPolicy` (engram-core, signed).** `{ scope/agent_id, allowed_egress: Vec<EgressRule>
   (exact / `*.`wildcard / `.`suffix host+recipient match, **default-deny**), allowed_action_classes
   (send|post|pay), EgressBudget { max_actions, max_spend_cents, expires_at_ms }, hardline_floor }`.
   Ed25519-signed with the **same key family** that signs skill manifests
   ([manifest.rs](../crates/engram-skills/src/manifest.rs)) and the ledger
   ([ledger.rs](../crates/engram-core/src/ledger.rs)); policy + signature recorded in the ledger.
2. **Where it's defined + signed.** Add an `autonomy_policy` field to the durable named-agent record
   ([main.rs:1479+](../crates/engramd/src/main.rs)), edited in the agent editor UI **when the human is
   present**, signed on save, ledgered `autonomy.policy.set`. Scheduled jobs carry a policy ref or an
   inline per-task policy minted at schedule time.
3. **Run-context bit.** Add `attended: bool` (or `surface: Interactive|Scheduled`) to `ToolCtx`
   ([tool.rs:70](../crates/engram-agent/src/tool.rs)). HTTP/UI sets `attended=true`; the scheduler
   paths set `attended=false` and attach the resolved signed policy. (Mirrors `HERMES_CRON_SESSION`.)
4. **The gate becomes tiered `resolve()`.** Replace `egress_blocked = trifecta && !ctx.policy.approved`
   ([agent.rs:316-332](../crates/engram-agent/src/agent.rs)) with `resolve() -> {Refuse|Proceed|Stage}`:
   - **Tier 0 floor**: destination in `hardline_floor` OR spend > cap OR a secret/credential sink →
     **REFUSE**, ledger `egress.refused_floor`. No policy lifts this.
   - **Tier 1 allowlist**: trifecta armed AND destination matches `allowed_egress` AND class allowed
     AND `budget.remaining > 0` AND not expired → **consume one budget unit, PROCEED**, ledger
     `agent.egress_autonomous`. Budget is a shared counter across delegated subagents (one signed pool).
   - **Tier 2 otherwise**: `attended` → today's live UI prompt; `unattended` → **STAGE**.
   - **Fail-CLOSED** on egress (policy parse/sig error → refuse egress, keep doing non-egress work).
5. **Async approval queue.** `~/.engram/pending/egress/<id>.json` (atomic tmp+rename, signed, ledgered
   `egress.staged`). On staging: notify the human via the existing channel layer
   ([channels.rs](../crates/engramd/src/channels.rs)) — `RequestApprovalTool` already authors exactly
   this description; wire it to enqueue+notify. The run returns the parked action to the model as
   "staged for review" and **continues other work**. The human later `POST /v1/egress/approve/<id>`; a
   scheduler tick drains approved items and executes them (`egress.approved` → `agent.egress_autonomous`).
6. **Freeze the bypass.** The signed policy is loaded out-of-band at run construction from the signed
   agent/job record — **never settable by the model or tainted content mid-run**. Verify the Ed25519
   signature before honoring any allowlist entry; an unsigned/forged policy is treated as empty
   (default-deny). The single most important security property.
7. **Audit.** Every tier outcome appends to the signed ledger with a typed reason code
   (`egress_refused_floor`, `egress_autonomous`, `egress_staged`, `budget_exhausted`, `grant_expired`,
   `destination_not_allowlisted`) — the complete authority + every autonomous send offline-verifiable.

## Build plan

**MVP (unblocks safe days-long operation):**
1. ✅ **Built** — `AutonomyPolicy` signed type + `EgressBudget` + Tier-0 floor + glob matching + sign/verify
   ([engram-core/src/autonomy.rs](../crates/engram-core/src/autonomy.rs), 5 tests).
2. ✅ **Built** — tiered egress decision at the dispatch gate
   ([agent.rs](../crates/engram-agent/src/agent.rs) `egress_decision`): one-time approval → signed
   policy (allowlist + atomic, race-correct budget) → attended-refuse / unattended-stage, all
   ledgered (`agent.egress_autonomous|egress_staged|egress_refused`). 2 tests; existing trifecta tests
   preserved.
3. ✅ **Built** — `Policy.attended` + `Policy.autonomy` + shared `egress_consumed` counter; the daemon
   sets `attended` per surface (UI/HTTP = true, scheduler/inbound = false) and loads + **verifies** the
   signed policy from the durable-agent record at run construction (`AgentDef.autonomy_policy`;
   forged/unsigned → fail-closed). Threaded through every `run_task_core`/`run_agent_task_cb` call site.
4. ✅ **Built** — durable approve-queue (the **signed ledger IS the queue** — no extra store).
   `GET /v1/egress/pending` derives the queue from `agent.egress_staged` minus resolved entries
   (deduped); `POST /v1/egress/approve` **adds the destination to the scoped agent's allowlist and
   re-signs** (so it sends there from now on, no re-approval); `/deny` records the decision. UI: a
   "Pending approvals" modal (Allow/Deny), a clickable banner when items wait, and a command-palette
   entry. 2 daemon tests + live approve→allowlist round-trip + populated-list render verified.
5. ✅ **Built** — policy authoring in the agent editor. Daemon `POST /v1/agents/{id}/policy` builds the
   policy, **signs it with the brain key** (`Registry::sign_autonomy`), stores it on `AgentDef`, and
   ledgers `autonomy.policy.set` (`enabled:false` revokes). The agent-editor UI gained an "Autonomy"
   section (allowlist, Send/Post/Pay, max-actions, expiry-days, hardline floor) prefilled from the
   stored policy. Verified live: author → sign → persist → re-render (screenshot).

**Later:**
6. ✅ **Built** — channel notify. After an UNATTENDED run that staged egress, `run_task_core` posts a
   summary (count + destinations) to the configured channel webhook (`post_webhook`, Slack/Discord/
   generic) so the user learns there's something to approve without opening the app. Best-effort,
   fire-and-forget; no-op when no webhook is configured. Verified with a real local-listener POST test.
7. ⏭️ **Skipped (redundant)** — a per-run iteration/time ceiling. Runaway is already bounded by the
   agent loop's `max_steps` (default 8) and the optional `token_budget`, so this adds little.
8. ✅ **Built** — `verify-autonomy`. `engramd verify-autonomy [HOME]` verifies the signed chain, then
   reconstructs every autonomous send / staged / refused / allowlisted / denied action per agent from
   the ledger — the offline "prove what the agent did unattended" report (no daemon trust needed). Same
   data via `GET /v1/autonomy/report` and an "Autonomy report" command-palette entry + modal. Pure
   `autonomy_report` (unit-tested); `entries_from_file` added to engram-core for key-free reads.
   Verified live: CLI on the real ledger (10 entries, chain intact), endpoint, and UI modal.

## Security review (adversarial red-team)

The egress gate was red-teamed (6 attackers, each probing a distinct bypass, then adversarial
verification). Verdict: **architecturally sound — 5 of 6 properties confirmed safe**, with one
confirmed bug and three real hardening gaps, all now fixed:

**Confirmed safe:** (A) the model cannot influence `approved`/`attended`/`autonomy` — the bypass is
frozen at run construction, loaded before untrusted content; (B) a forged/tampered policy fails closed
(signature covers the whole struct incl. floor); (C) budget is race-correct and delegated sub-agents
share the one counter (can't reset it); (E) a non-allowlisted destination stages, never sends; (F)
`agent_set_policy` hard-codes `scope=agent:{id}` server-side, and minting a mismatched-but-valid record
needs the on-disk signing key **and** filesystem write outside the confined workdir.

**Fixed:**
1. **Trailing-dot FQDN floor bypass** (confirmed, medium) — `paste.evil.com.` evaded a suffix floor
   while matching a `*` allowlist. `EgressRule::matches` (and `host_of`) now strip the trailing dot.
2. **Opaque/MCP egress invisible to the gate** (medium) — destinations with no host arg fell back to
   the tool *name*, which `*` would "match" and the floor would miss. `egress_destination` now returns
   `Option`; an unresolved destination **never auto-allows** — it refuses (if a floor is set) or stages.
3. **Scope not re-bound on load/use** (low) — the policy load and `egress_approve` now require
   `policy.scope == agent:{id}` (defense-in-depth beyond the signature).
4. **Unenforced `max_spend_cents`** (low) — marked RESERVED/not-yet-enforced so it isn't presented as a
   guarantee (it's not surfaced in the UI). Regression tests cover (1) and (2).

## Why Engram autonomous agents are safer than Hermes's

Authority is **cryptographic and scoped**, not ambient: every autonomous egress must match an
Ed25519-signed `AutonomyPolicy` (allowlisted recipient + action class + unspent budget + unexpired
grant) a human signed while present — Hermes's unattended mode is a coarse flag (`cron_mode=approve`
flips *all* non-hardline actions on) with no egress allowlist or budget. The **lethal-trifecta taint
gate still runs underneath** the policy, so even a pre-authorized agent can't exfiltrate private data
to a destination outside its signed allowlist, and a forged/unsigned policy **fails closed** while the
bypass is frozen at run construction (unreachable by injection). And **everything lands in the
append-only signed ledger** — the granted policy, every budget decrement, every staged action, every
refusal-with-reason — so a multi-day run is fully reconstructable and offline-verifiable against the
published public key. Approve a budget once; the agent spends it down deterministically, parks anything
novel for your async review, hard-blocks the forbidden floor, and signs its own receipt for every move.
