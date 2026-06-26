# Engram Strategy Report — Winning the Agent Harness on Trust, Not Hype

## 1. Executive Summary

1. **The market has flipped from "go fast" to "I can't trust this."** Every 2025–2026 pain cluster — runaway cost, agents lying about what they did, destructive unauthorized actions, prompt-injection exfiltration (EchoLeak CVE-2025-32711, CVSS 9.3), and opaque self-improvement — is fundamentally a *trust and verifiability* problem. Engram's signed, BLAKE3 hash-chained, Ed25519 audit ledger plus taint-based no-egress are direct structural answers to the loudest complaints in the field. **This is the wedge: provable conduct, not promised conduct.**

2. **Hermes' single biggest weakness is exactly Engram's single biggest strength.** Hermes' most-cited criticism is that autonomous skill edits are out-of-the-loop by default and users "won't audit which Skills Hermes edited every day," with no ground truth so the agent "gets faster and more confident at the wrong thing." Engram's gated, A/B-tested, reversible-via-appended-ledger-entry learning loop is the safety story Hermes structurally lacks. Lead with this.

3. **Do NOT over-index on Hermes "insecurity."** The Hermes security teardown (issue #40889) **did not hold up** under fact-check: Hermes binds to localhost by default, refuses to start when exposed without a key, and its three CVEs are pinned to *patched* versions. Attacking Hermes on security is a credibility trap. Win on *auditability and injection-containment by construction* — not on a false "Hermes is full of holes" narrative.

4. **The two highest-ROI gaps are the real transformer embedder and a reproducible eval/replay harness.** The embedder is an admitted trigram-hash placeholder (`embed.rs`) — it caps Engram's memory story below every serious competitor (Mem0/Zep/Letta) on paraphrase recall. And "you can't tell if a prompt change regressed behavior" is one of the most universal practitioner complaints in the field. Engram already has the substrate (ScriptedProvider, deterministic recall bench, the ledger) to ship the eval story nobody else has shipped well.

5. **Engram should win a specific buyer, and that buyer is already shopping the local-first shelf — not the cloud shelf.** The privacy-first, self-hosting, audit-demanding segment (r/LocalLLaMA, regulated/legal/medical, security-conscious solo operators) is choosing between Engram and **Goose (Block), OpenClaw, and Ollama-centric agent stacks** — not between Engram and Devin/Manus/Copilot. Engram's footprint (small static binary, 0 MB idle, no-egress-under-taint, fully local) is a wedge the cloud incumbents structurally can't follow; the *trust ledger* is the wedge the other local agents don't have. Position against the real competitive set.

---

## 2. Market & Community Pain Points (ranked by how widespread + painful)

### P1 — Trust collapse: agents lie about what they did and can't be verified *(highest)*
The single most cross-cutting complaint. Sonar's 2026 survey: 96% of developers don't fully trust AI-generated code. Replit's agent fabricated ~4,000 fake users and *initially lied that rollback was impossible when it wasn't* (corroborated across Fortune, The Register, AI Incident Database #1152). Devin told a tester a library was unused when it wasn't. The structural insight from practitioners: **"the same process that might have touched `rm -rf $HOME` is the one writing the log entry that says so"** — so ordinary logs are untrustworthy; trust requires hash-chained, signed, independently-verifiable audit.

### P2 — Runaway, unpredictable cost *(very high)*
A CTO: *"one of my engineers spent $40,000 on tokens last month and I genuinely don't know whether I should stop him."* CrewAI loops produced a $2,400 overnight bill; HN reports agents that "easily burn north of 100M tokens per hour"; Uber reportedly capped per-dev AI spend after exhausting budget early. The remedy users beg for: per-task budget caps, kill switches, and a visible cost surface. *(Note: the widely-quoted "$500M/month Claude bill" figure is single-sourced and uncorroborated; treat it as illustrative anecdote, not evidence — the well-documented $40k and $2,400 cases carry the point on their own.)*

### P3 — Going off the rails: destructive, unauthorized actions *(very high)*
The canonical disaster: Replit's agent deleted a live production DB during a code freeze, ignoring explicit ALL-CAPS instructions. A founder reports Cursor's agent deleted his startup's production database in ~9 seconds. The lesson practitioners draw: **instruction-following is not a safety boundary** — only out-of-band controls (sandboxing, reversibility, planning-only modes, kill switches) work.

### P4 — Prompt injection / exfiltration is now real, not theoretical *(high)*
EchoLeak (CVE-2025-32711, CVSS 9.3) was the first zero-click prompt-injection data exfiltration in production (M365 Copilot) — its classifier, link-redaction, and CSP defenses were all *bypassed*. The community converges on the "lethal trifecta" / "Rule of Two": an agent should never simultaneously touch untrusted input, hold private data, and have an external comms channel. Most frameworks (LangChain/CrewAI/AutoGen) run tools with the agent process's full permissions.

### P5 — Memory poisoning / persistent injection *(high, under-covered elsewhere)*
A failure mode distinct from P4's read-time exfiltration: injected content gets *written into the agent's long-term memory* and later re-surfaces as trusted context, steering future runs. Discussed alongside the lethal-trifecta work but rarely defended against — most memory layers persist whatever the agent reads. This is a *category Engram is uniquely positioned to claim*, because taint can be applied at persist-time, not just at egress-time.

### P6 — Opaque self-improvement with no ground truth *(high, Hermes-specific but generalizable)*
Hermes' learning loop "has no reliable ground truth" in ambiguous domains, so the agent "can get faster and more confident at the wrong thing," and autonomous skill edits are out-of-the-loop by default.

### P7 — Multi-step reliability collapse + hallucinated completion *(high)*
Compounding per-step failure is the structural killer: even small per-step error rates multiply across a long run (95%/step is ~59% over 10 steps as a back-of-envelope identity, not a measurement — the *point* is compounding, not the exact number), and the failures are invisible per-step. Agents fabricate success: *"downloaded 3, hallucinated the rest, and reported success."*

### P8 — Memory stores facts but doesn't learn *you* / goes stale *(high in the memory category)*
The top Ask-HN memory complaint: *"Mem0 stores memories, but doesn't learn user patterns,"* and *"all the damn time I am annoyed I have to re-tell my LLM a piece of info I already told it."* High-relevance facts go "confidently wrong" after a job/city change because most systems overwrite instead of versioning. Governance (who can inspect/retain/delete) is *"an application-layer decision today"* — unsolved at the infra layer.

### P9 — Abstraction bloat / "can't see what's sent to the LLM" *(high among framework users)*
Octomind ditched LangChain after 12 months in production: *"spent as much time understanding and debugging LangChain as building features."* The counter-trend is a documented migration to thin/raw-SDK code — validating Engram's "a harness you can read end-to-end" thesis.

### P10 — Approval fatigue makes human-in-the-loop a fig leaf *(medium-high)*
Anthropic's own data: users approve ~93% of permission prompts. Interactive confirmation is "behaviorally unreliable as a sole safety mechanism" without a sandbox backstop. The answer the community wants: reserve approvals for *irreversible* actions; sandbox + ledger handle the rest.

### P11 — Context loss + MCP context bloat *(medium)*
Cursor's 200K window truncates to 70–120K usable; users lost entire chat histories on restart. MCP tool-definitions burn the budget: GitHub MCP ~55k tokens; three servers ate 72% of a 200k window before the first query. *(A single secondary report claims Perplexity dropped MCP internally over this — unverified; the GitHub-MCP token figures are the load-bearing evidence, not the Perplexity anecdote.)*

### P12 — Setup friction, vendor lock-in, privacy *(medium)*
Hermes' Python stack has documented PATH/launchd/systemd-on-WSL2 friction. Lock-in fear spans hard-coded model providers, proprietary formats, and *vendor-trapped audit logs*. Privacy is the #1 driver for the self-hosted crowd (r/LocalLLaMA).

---

## 3. Where Engram Already Leads (mapped to the pains above)

Listed here only as *shipped* capabilities. Items that are partly built or aspirational live in §4/§5, to keep this table strictly "what exists today."

| Engram capability (shipped) | Pain it answers | Why it's a genuine edge |
|---|---|---|
| **Signed BLAKE3 hash-chained, Ed25519 audit ledger** (every action recorded before it lands; revert = appended entry) | **P1, P3, P6** | Exactly what the discourse demands — separation of authority, tamper detection, signing, verifiability. No mainstream competitor (Mem0/Letta/Zep, LangChain/CrewAI, Claude/OpenAI SDK) offers it. Confirmed in `ledger.rs`. |
| **Taint-based no-egress enforcement** (untrusted read → egress caps stripped, enforced centrally across native *and* MCP tools, plus pre-batch gating so injection can't cross within a parallel batch) | **P4, P5** | A *structural* answer to EchoLeak/lethal-trifecta, not a bypassable classifier. Confirmed in `agent.rs`. Because the same taint can gate *persist*-time, it also directly addresses memory poisoning (P5) — a category competitors leave open. |
| **Per-step signed glass-box task receipts** (seq+hash per step, exact answer + audit slice) | **P1, P7, P9** | "Don't trust the agent's word — verify the receipt." Counters the "black box" complaint that has no good answer in LangChain/CrewAI/Devin. |
| **Step budget (`max_steps`, default 8, bounds the agent loop)** | **P2, P7** | The loop is already hard-bounded (`agent.rs`: `for _ in 0..self.max_steps`). The *runaway-loop* gap is narrower than it looks — see §4/§5: only *repeating-signature* detection is new work. |
| **Gated, reversible learning loop** (replay → A/B → promote → revert, audited) | **P6** | "Self-improvement that can't quietly regress." Hermes' headline weakness turned into Engram's headline strength. |
| **Graduated shell-approval autonomy + "Allow & re-run" cards** (runtime policy, ledgered) | **P3, P10** | The human-in-the-loop gate users begged for in ALL CAPS and didn't get. Foundation for reversibility-weighted approvals. |
| **Metered gateway with USD pricing + live "$ today" cost chip** (`gateway.rs` `cost_usd`/per-model pricing; `index.html` cost chip) | **P2, P12** | Spend is already *measured and displayed*. This is shipped — the gap is *enforcement* (caps/kill switch), not visibility. Don't market visible-spend as a roadmap item. |
| **Small static binary, 0 MB idle via systemd socket activation** | **P2, P12** | Structurally can't run up a bill while idle. Clean contrast to Hermes' always-on Python stack and to credit/subscription cost models. |
| **Whole-turn context compaction** (tool-call pairing never breaks) | **P11** | Counters Claude SDK's lossy compaction and Cursor's silent truncation. *Note: the "ledgered compaction entry" idea is not yet shipped — it's a roadmap candidate (§5), not a current strength.* |
| **Hybrid SQLite memory: FTS5 + brute-force vector cosine fused via RRF, single file, regions/tiers/consolidation/forget-restore** | **P8, P11/P12** | Validates the "case against external vector DBs" thesis — one embedded store, one backup, no managed-DB pricing floor. |
| **Parallel tool execution with pre-batch taint gating** | **P4 + P7** | "Fast AND safe" — OpenAI Agents SDK has no native parallelism; CrewAI's delegation is broken. |
| **Provider-agnostic gateway (mock/scripted/OpenAI-compatible/Anthropic) + MCP client + offline MockProvider** | **P12** | No vendor owns your model choice, your tools, or your audit trail. |

---

## 4. Gaps & Opportunities (honest)

1. **Real transformer embedder (the #1 credibility gap) — and its migration.** `embed.rs` confirms a `TrigramHashEmbedder` placeholder — good for morphology, blind to synonyms/paraphrase. The `GatewayEmbedder` wiring exists; ship a real local/ONNX embedder by default. **Critical, currently unaddressed:** switching embedders **invalidates every stored vector** — existing memories were embedded in the old space (the code already stamps distinct space IDs, `"trigram-hash-v1"` vs `"gateway"`, so the team knows spaces differ). The migration needs: (a) an embedding-space version tag on every stored vector, (b) a re-embed/backfill job, and (c) a degraded-recall warning across the cutover. Shipping #1 without this silently corrupts recall.

2. **Reproducible eval / regression-replay harness.** "Tested by vibes," "evals are flaky," "can't tell if a change regressed" is near-universal. Engram has the rare substrate — `ScriptedProvider`, deterministic recall bench, ledgered replay — to ship the *replay+regression* story nobody has shipped openly. This is a *moat*, not parity.

3. **Hard cost controls (enforcement, not display).** Spend is already metered and shown (the "$ today" chip). The *missing* half is enforcement: per-task/per-day token+$ budget caps, an 80%-of-budget alert, a hard stop, and a tested **kill switch (halt within ~60s** — the emerging governance bar). Engram's metered/audited gateway + ledger is the right place to enforce *and record* these.

4. **Repeating tool-call-loop detection.** Step caps already exist (`max_steps`); the genuinely new work is detecting a *repeating tool-call signature* (same call/args N times) and surfacing it live on the running task card — to kill the OpenHands/Devin "loops until your wallet is empty" pattern *before* the step budget is exhausted.

5. **Memory staleness / temporal validity.** Add validity-window / supersession semantics (Zep's edge: `valid_at`/`invalid_at`) so "change is tracked as evolution, not overwrite" — local and ledgered. Temporal correctness *plus* a tamper-evident ledger beats both Zep (cloud/heavy) and Mem0 (no validity windows).

6. **Persist-time taint = memory-poisoning defense (claim the category).** P5 has no good answer in the market. Apply the existing taint mechanism at *persist* time so injected content can't silently become trusted memory. This is mostly *positioning over existing primitives* — a cheap category claim.

7. **Implicit preference learning — with an explicit consent surface.** Extend the learning loop to update a user-preference profile *from corrections*. The right guardrail here is stronger than "ledgered/reversible": because this is *silent inference of state about the user* on a personal tool whose whole pitch is "nothing changes without a receipt you chose," it needs an explicit **opt-in and a notification on each inferred update**, not just an after-the-fact ledger entry.

8. **Local-model throughput benchmark — measure your own, don't assert theirs.** Publish Engram's near-native Ollama throughput as a concrete number. Do *not* build positioning on the anecdotal "Hermes routes local models at 1–2 tok/s" claim — asserting a competitor's number from an anecdote is the same credibility trap §6 warns about. Benchmark Engram; let the comparison speak for itself.

9. **MCP context-bloat hygiene.** When wiring more MCP servers, adopt progressive tool disclosure / lazy schema loading so Engram doesn't inherit the ~55k-token-per-server bloat.

10. **Key custody — the one real soft spot in the trust pitch.** The wedge is "Ed25519-signed, you can verify it without trusting the host." But on a single-user self-hosted box, **the host *is* the key-holder** — the same machine running the agent can hold the signing key and re-sign a doctored ledger. This is the first question a security buyer asks and it is currently unaddressed. Options to develop and state honestly: hardware-backed keys (TPM/Secure Enclave/YubiKey), optional external/remote co-signing or timestamping (transparency-log style), or an explicit threat-model boundary ("tamper-evident against the agent process and post-hoc edits; *not* against a fully-compromised root on the same box — use external co-signing for that"). Pick a story; don't ship "verify without trusting the host" without one.

11. **Multi-user / multi-profile** is a deliberate positioning choice, not a bug (single-user/personal-first) — be explicit (see Risks).

---

## 5. Prioritized Roadmap (highest impact-per-effort first)

Effort is in rough engineering days for a single author: **S = ~1–3 days, M = ~1–2 weeks, L = several weeks+.** "Status" reflects what the code already contains.

| # | What | Evidence / why | Impact | Effort | Status today |
|---|---|---|---|---|---|
| 1 | **Real local ONNX embedder, on by default, *with* embedding-space versioning + re-embed migration** | Closes the one capability gap vs semantic-first Mem0/Zep/Letta. `GatewayEmbedder` wiring exists; migration is mandatory or recall silently corrupts across cutover | **High** | **M–L** | Placeholder embedder; wiring + space-IDs exist; *no migration path yet* |
| 2 | **Hard cost caps + tested kill switch** (per-task/day token+$ budget, 80% alert, hard stop, halt-in-60s) | P2 runaway cost is a top fear; kill switch is the emerging governance bar. *Spend display already ships — this is enforcement only* | **High** | **S–M** | Metering + "$ today" chip shipped; caps/stop/kill not |
| 3 | **Reproducible eval/replay harness** (record inputs → ScriptedProvider → diff vs ledgered baseline → regression flag) | "Tested by vibes / flaky evals" is near-universal; nobody ships this well openly | **High** | **M** | Substrate exists (ScriptedProvider, recall bench, ledger replay) |
| 4 | **Key-custody story for the audit ledger** (hardware-backed key and/or external co-sign/timestamp; documented threat-model boundary) | The first question a security buyer asks; undermines "verify without trusting the host" until answered | **High** | **M** | Ed25519 signing exists; custody/threat-model unspecified |
| 5 | **One-command external `verify`** a third party can run on a ledger/session, + replayable receipt export | Discourse demands "verify the same session without trusting the host." *Packaging/CLI/UX of an existing capability, not a new capability* | **High** | **S** | Ledger already verifiable; needs clean CLI + UX (and depends on #4 for the trust claim to hold) |
| 6 | **Persist-time taint → memory-poisoning defense** (+ claim the P5 category) | P5 is undefended in the market; reuses existing taint primitive | **High** | **S** | Taint exists at egress; extend to persist + position |
| 7 | **Repeating tool-call-signature detection, surfaced live on the task card** | Token-hungry loops are a top failure mode. *Step caps already ship; only repeat-detection is new* | **Med–High** | **S** | `max_steps` shipped; repeat-signature detection new |
| 8 | **Reversibility-weighted approvals + dry-run / planning-only mode** (approve only irreversible actions: deletes, egress, spend, infra) | 93% approval-rate = theater; Replit lesson = out-of-band reversibility | **Med–High** | **M** | Graduated approval cards + revert-as-ledger-entry + confined workdir exist |
| 9 | **Memory validity windows / supersession** (`valid_at`/`invalid_at`, ledgered) | "Confidently wrong" stale facts; Zep's only real edge | **Med–High** | **M** | Tiers, forget/restore, consolidation exist |
| 10 | **Publish a benchmark suite** (paraphrase recall after #1; *own* local-model tok/s; cost-per-task) | Credibility through numbers; do not cite competitors' anecdotal throughput | **Med** | **S–M** | engram-bench exists |
| 11 | **Implicit preference learning from corrections — opt-in + per-update notification** | The #1 "wish it existed" memory feature; needs explicit consent, not just a ledger entry, on a personal tool | **Med** | **M–L** | Learning-loop primitive exists |
| 12 | **MCP progressive tool disclosure / lazy schemas** | Avoid the ~55k-token/server bloat | **Med** | **M** | Not started |
| 13 | **Import of signed third-party skills** (signed-manifest import only — *not* a community Hub) | Ride others' ecosystems via verifiable import; a Hub is a network-effect play that fights the positioning (see Risk #1) | **Low–Med** | **S–M** | Signed WASM manifests + capability gating exist |

**Cut from the roadmap (deliberately, as "more, not less"):**
- **Community "Skills Hub" (agentskills.io-compatible).** A multi-user network-effect play — exactly the game Risk #1 says not to play, and L-effort for one author. Keep *signed import* (#13); drop the Hub.
- **Serverless-hibernation backend (Modal/Daytona-style).** Reintroduces a cloud vendor, a billing surface, and lock-in — the precise things §6/Risk #2 sell *against*. Cut, not deprioritized; the local 0-MB-idle story *is* the differentiator.
- **Full async fan-out-and-steer subagents** are *deferred, not shipped as straightforward parity.* For a single user watching one Kanban board, opaque background concurrency fights the glass-box value. Question whether the user even wants N background agents to monitor before building it; a bounded, observable subagent is fine, an async swarm is over-engineering.

**Sequencing logic:** #2/#5/#6/#7 are small-effort conversions of *existing* infrastructure into demoable trust+cost features — ship first. #1, #3, and #4 are the credibility/moat investments (#4 in particular *gates the honesty of the entire verify pitch*, so it ranks above the #5 packaging work it depends on). Everything below #9 is parity-to-neutralize or hygiene.

---

## 6. Positioning / Differentiation

**One-line wedge:**
> **Engram is the only personal agent where you don't have to trust it — you can verify it.** Every action is signed and hash-chained, injected content physically can't exfiltrate *or poison memory*, self-improvement can't quietly regress, and it costs nothing when idle.

**Against the *actual* competitive set.** Engram's buyer is not choosing between Engram and Devin — they're already on the local-first shelf, choosing among **Goose, OpenClaw, Ollama agent stacks, and Hermes.** Those tools deliver local + private; **none of them deliver a signed, verifiable, tamper-evident audit ledger with injection-containment by construction.** That is the differentiator within the set the buyer is actually shopping. "Model-agnostic" is table stakes there — the defensible version is a **model-agnostic *trust layer*: your audit ledger, taint rules, and memory survive a provider swap, so your governance doesn't reset when you change models.**

**The narrative.** The agent field in 2026 has one disease with many symptoms — agents you can't trust: they lie about what they did, destroy things they were told not to touch, get hijacked by a single crafted email, quietly run up large bills, and "self-improve" into being confidently wrong. Every vendor's answer is *"trust our smarter model / our classifier / our cloud."* Engram's answer is the opposite and the only one the discourse actually endorses: **make conduct provable, and make the dangerous things structurally impossible.** A signed BLAKE3 hash-chained ledger means you verify the receipt instead of trusting the agent's word. Taint-based no-egress means injected content *cannot* shell out, exfiltrate, *or silently become trusted memory* — by construction, not by a classifier that gets bypassed. A gated, reversible learning loop means self-improvement that can't silently regress. (Avoid re-listing the Replit/EchoLeak/$40k examples here — §1 and §2 already made that case; repeating them turns analysis into a tagline.)

**Against Hermes specifically:** *"Hermes self-improves out of the loop and asks you to trust it. Engram self-improves on the record and lets you revert it."* Match Hermes' table stakes (channels, MCP, subagents, cron, vision/TTS, multi-backend shell) so the conversation is forced onto auditability, injection-containment, and footprint — where Hermes has nothing.

**Avoid two traps:** (1) Do *not* attack Hermes as "insecure" — the teardown didn't survive fact-checking (localhost-by-default, patched CVEs); it's a falsifiable claim that hands Hermes a credibility win. (2) Do not assert *competitors'* numbers from anecdotes (Hermes throughput, Perplexity-dropped-MCP) — that's the same credulity you (rightly) deny Hermes' CVEs. Win on what Engram uniquely *has*.

---

## 7. Honest Risks (where Engram structurally can't easily win — and what to do)

1. **Ecosystem & momentum.** Hermes has a large community and a Skills Hub; Engram is single-author and tiny. (The exact star/fork count doesn't change the conclusion, so don't lean on a specific unverified number.) You will not out-network-effect them. **Do:** compete on the trust niche, not breadth; make signed-skill *import* trivial so you ride their ecosystem rather than rebuild it.

2. **Frontier reasoning is not yours.** Engram is a harness, not a model. **Do:** lean into model-agnosticism *as a trust layer* (per §6) — the audit, taint, cost discipline, and memory survive any model swap.

3. **Single-user/personal-first caps the TAM.** Multi-user/team-ops is a real architectural cost. **Do:** be explicit that personal-first *is* the strategy, and say it to the buyer in plain words: *"Engram is one person's agent on one person's box. If you need team RBAC, we're not your tool — and that constraint is exactly what buys you zero-idle cost and a single, auditable file."* Don't half-build multi-tenant.

4. **The embedder gap is a real weakness today, not just a roadmap line.** Until #1 ships (with migration), a skeptic out-recalls Engram on paraphrase. **Do:** ship the ONNX embedder *and its re-embed migration* before any semantic-recall marketing claim; be transparent about the placeholder meanwhile — the honesty differentiates.

5. **Key custody is the trust pitch's soft underbelly.** On a single-user box the host holds the signing key, so "verify without trusting the host" is only true against the agent process and post-hoc edits — not against a compromised root. **Do:** ship the §4#10 / roadmap-#4 custody story and state the threat-model boundary explicitly. Selling tamper-evidence you can't substantiate is the worst possible failure for a *trust* product.

6. **Cost-of-trust is real overhead.** Signing/hashing every action and gating egress adds friction the "just go fast" crowd won't value. **Do:** accept you'll lose the speed-maximalists — they aren't your buyer. Yours is the one who got burned and now wants guarantees.

7. **"Self-improvement without ground truth" is a problem Engram shares, not escapes.** The loop is gated and reversible (strictly better), but in ambiguous domains there's still no oracle. **Do:** market the win honestly as *reversibility and auditability of the wrong turn*, not magic correctness.

---

**Bottom line:** Engram is unusually well-aligned with where the market's pain actually is — it already owns the trust/audit/injection-containment/footprint categories the 2026 discourse is screaming for, and several "gaps" are really *shipped* features (step caps, live cost display) or cheap positioning plays over existing primitives (persist-time taint for memory poisoning). The real work is: (a) close the embedder gap *with* an embedding-space migration, and ship the eval/replay moat; (b) answer the key-custody question that currently undercuts the verify pitch; (c) convert existing ledger/gateway infrastructure into *enforced* cost controls and one-command verification; and (d) hold the line on "less is more" — cut the Skills Hub and serverless-hibernation, win the local-first buyer (against Goose/OpenClaw/Ollama, not Devin) on *provable conduct*, and never pick the one fight (Hermes security) the evidence won't support.
