# Engram - Product Roadmap

*The output of a vision pressure-test: founder vision + engineer synthesis, challenged by an
architect / strategist / design / skeptic panel, then consolidated. The scope gate for every
feature is one sentence:* **"Does this produce a signature a competitor can't?"** *If it doesn't
deepen verifiable memory, verifiable expertise, or verifiable dissent, it is sprawl and gets cut.*

## Status - 2026-06-27: roadmap shipped

All phases below are implemented and deployed. Phase 0 (no-bluff basics, real Telegram connect,
templates, in-app dialogs); Phase 1 (Consciousness - signed, editable, always-loaded working memory
driving every run); Phase 1.5 (recall ribbon under each answer, now persisted across reload; the 2D
brain animates growth); Phase 2 (durable named role-scoped agents signing their own steps + cards,
assignable on the kanban with **hand-off trails**; honest Skills version-ladder with verified/unverified
pills); Phase 3 (auditable dissent - grounded, cited, model-gated, or silent). Beyond the roadmap, by
explicit user request: the brain is now a **2.5D orbitable** canvas graph (drag/zoom, settles to rest,
click-to-open; honest - clusters not anatomy), the Workspace settings panel is a real project manager,
and the LLM API key is policy-bound to memory only (`skip_serializing`). Two adversarial code reviews
(backend + frontend) ran over the new code; findings fixed. The one remaining item, exercising the
live model-judgment paths (dissent, etc.), is gated on the user connecting a provider key.

## Status - 2026-06-28: desktop-capability pass (vs Hermes + Claude Desktop)

A gap analysis against [Nous Hermes](https://github.com/nousresearch/hermes-agent) (config/
connectivity breadth) and [claude-desktop-debian](https://github.com/aaddrick/claude-desktop-debian)
(native desktop integration) drove a pass to make the desktop a *real* app, not a webview over a
URL, and to cover/exceed the table-stakes config surface - while staying on-wedge (no marketplace,
one channel end-to-end, no swarms; the signed ledger stays the differentiator).

- **Native shell** ([`desktop/src-tauri/src/main.rs`](../desktop/src-tauri/src/main.rs)): system
  tray (Show/Hide/Restart/Open-at-login/Quit), close-to-tray, native menu bar (clipboard shortcuts
  work in the webview on macOS), global summon hotkey, single-instance lock + focus, run-at-login,
  window-state persistence, the `engram://` deep-link scheme, dock-reopen. Plugins driven from Rust
  so they work over the daemon-origin webview with no IPC.
- **Connectivity breadth**: the gateway now routes Anthropic (native) plus any OpenAI-compatible
  backend through one path - OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Together, xAI, Perplexity,
  Gemini (OpenAI endpoint), and local Ollama/LM Studio/vLLM/llama.cpp - each with a built-in default
  endpoint and a one-click picker preset. Verified end-to-end (real provider auth errors, not mock).
- **First-run onboarding wizard** + honest offline state; **bounded MCP templates** (filesystem,
  fetch, git, memory, sequential, time); **settings export/import** (secrets excluded by design).
- **`engramd doctor`** self-diagnostic (the local equivalent of `engramd verify`) + `help`/`--version`.
- **Two honesty/robustness fixes**: `/health` `offline` now reflects the *live* provider (the old
  env heuristic could claim offline while a real model was connected); a daemon restart re-execs in
  place and carries the memory-only API key through the successor's environment, so reloading
  boot-time settings never silently drops a connected provider back to the mock (key still never on
  disk). Both covered by tests; `cargo test --workspace` + `-p engramd --features http` green, clippy clean.

## North star

Engram is the personal agent whose **conduct AND reasoning you can verify** - a glass-box brain
that signs not just what it did, but what it has come to believe about you and where its own
expertise disagreed with itself. The wedge is the signed BLAKE3/Ed25519 ledger applied to three
things no competitor can follow without first building that ledger:

- **Verifiable memory** - a tiny, always-loaded working context continuously distilled from the
  deep brain-graph, shown and signed.
- **Verifiable expertise** - skills that *measurably* improve via the replay→A/B→promote→revert
  loop, with the before/after gate signed.
- **Verifiable dissent** - an expertise-grounded objection to a plan, signed alongside the plan and
  the user's choice.

**One brain, not a crew.** The brain metaphor only earns its place where it pays rent
(working-vs-long-term memory; skill-cluster expertise) and is cut everywhere it's decoration.

## Verdicts on the proposed ideas

| Idea | Call | Why |
|---|---|---|
| Finish basics (connect-flows, MCP UI, project/session polish, templates) | **Endorse** | A UI that bluffs about its capabilities is an active trust violation. This *is* the vision, not a prerequisite. |
| Model picker "is static" | **Already done** | Provider/model/key hot-swap + `/v1/config/test` shipped (b30cfab). Only fix: honest "offline mock" state vs a fake live list. |
| "Consciousness" over SOUL/persona | **Refine** | Endorse the *mechanism* (distilled, signed, editable working memory). Ship the mechanism, **not the metaphysics** - no "conscious self" claims; they undercut verify-don't-trust. |
| 3D / anatomical / orbitable brain (incl. 2.5D point-cloud) | **Cut** | Anatomical position is *invented* (recall-relevance has no lobe) - decorative dishonesty in an honesty product. Orbit needs WebGL/Three.js (breaks no-build/no-CDN) or fragile per-frame camera math. Animate the **2D** brain instead. |
| ~~Separate-mind subagents~~ / **fragile in-process swarms** | **Cut** | Founder ALSO rejects these ("without fragile in-process subagent swarms"). Ephemeral in-process orchestration duplicates context, fights zero-idle, and is the black box we sell against. |
| **Durable, named, role-scoped agents collaborating via the kanban** (founder's clarified intent - the Hermes model) | **Endorse (revised)** | NOT swarms: *durable, configurable* agents - each a strict role + system prompt (narrow focus ⇒ less drift/hallucination) + its own model (right model per task) + optional skill-cluster - that collaborate on one mission via the **durable, signed** kanban board. Each agent's actions are signed (ledger actor = the agent), so the collaboration is **fully auditable** - glass-box multi-agent, the opposite of an opaque swarm. This is on-wedge: *a team you can audit.* |
| Skills-as-growing-expertise as the visible centerpiece | **Refine** | The unique self-improving mechanic, currently *invisible*. But `learn.rs` scores by exact-byte match vs gold output - so honest growth exists only for deterministic/tool-shaped skills. Show the real promote/revert moment; show "unverified" where there's no scored signal. Don't oversell "harder variant reuses prior solution" yet. |
| Auditable dissent (specialist objects; plan+objection+grounds+choice all signed) | **Refine → Phase 3** | Fuses both moats into "an adversary you can audit." But ungrounded prompt-dissent is "the base model in a costume." Fires **only** when grounded in real evidence (replay-win record + conflicting recalled memories). Gated on the expertise signal being real first. |
| Friendly MCP UI + a few templates | **Endorse** | Raw `mcp.json` is a trust leak; a form over the existing `/v1/config/mcp-test` is high-ROI. Templates = a *small bounded* starter set, never a marketplace. |
| Integration breadth (Signal/WhatsApp/Teams/Slack/Discord) | **Cut** | Six logos = me-too. Ship **one** (Telegram) end-to-end as the reference; leave the rest as the honest generic webhook. |

## Phases

**Phase 0 - Trust integrity: stop the UI from bluffing.** Highest-ROI, least glamorous, ships
first. One real Telegram connect/test/status flow (live, no restart, signed `channel.connect`);
honest offline-mock model state; friendly MCP form over `/v1/config/mcp-test`; deterministic,
obviously-saved projects/sessions; a small bounded template set; plainly state the key-custody
threat boundary in the verify view.
*Done when:* a second person can install it, connect Telegram, configure a live model + an MCP
server through friendly forms, create a project, and find **no surface that claims something the
system can't do** - every connect action a signed ledger entry.

**Phase 1 - Consciousness: working memory you can see, edit, and verify.** Distill a tiny (5-9 line)
always-loaded working context from the identity/semantic memories; drive the agent run from it
(retire SOUL.md as source of truth); render it as an editable panel where each line is
region-tinted and click-throughs to its source memory; every distillation + user edit is a signed,
revertible ledger entry. No self-awareness copy.

**Phase 1.5 - The 2D brain delivers "watch it grow," honestly.** Animate node-in / tier-promotion /
live count on the existing 2D Canvas brain; ~30-40 lines of pointer-parallax + depth-blur for a
dimensional read (no camera matrix); a **recall ribbon** under each answer showing which memories
were pulled in, each clickable to its node + ledger slice; searchable/filterable memories+skills.

**Phase 2 - The auditable team + visible expertise.** *(Revised per founder: durable named agents,
the Hermes model.)*
- **Agent configuration:** define durable, named agents - each with a strict **role + system
  prompt** (narrow focus ⇒ less drift/hallucination), its **own model** (right model per task,
  with an auto-route option), and an optional **skill-cluster**. Persisted, not ephemeral.
- **Collaboration via the kanban:** assign an agent to a card (or let agents pick up / hand off
  cards on a single mission); the durable board is the coordination surface - no in-process swarm.
- **Auditable:** every agent action is signed with the agent as the ledger actor, so the team's
  collaboration is fully visible in the live-run view + Activity. *A team you can audit.*
- **Visible, honest expertise:** a Skills view with each skill's version ladder; on a promotion,
  animate the *real* before/after replay score + ◇ ledger chip ("v3 beat v2: 0.82→0.94 on 11
  replays"); "unverified - N runs" where there's no gold signal.

**Cross-cutting theme - Anti-hallucination (the hard focus).** The reason for narrow roles, and a
first-class goal throughout: (1) **role-scoping** - narrow agents drift far less than one broad
prompt; (2) **grounding** - every factual claim traceable to a recalled memory or a tool result,
shown in the recall ribbon; (3) **verification** - the reflect/adversarial-check pass before a run
finishes; (4) the **glass-box** surfacing unsupported claims. Rides the existing recall + signed-step
machinery; no new trust surface to bluff.

**Phase 3 - Auditable dissent (gated on real expertise signal).** When an assigned lens has high
replay-confidence and the instruction conflicts with what that expertise predicts, surface a single
inline "Specialist note: based on N prior wins, X tends to fail; these 2 memories say Y; proceed?"
citing the specific skills + memories; sign plan + objection + grounds + choice as one ledger
artifact. Dissent fires **only** from cited evidence - never prompt-only theater.

## Biggest risk

**Sprawl-driven trust erosion.** Engram's one defensible asset is end-to-end honesty between what
the UI claims and what the system does. The most dangerous near-term move is shipping prompt-only
dissent or a vibes-based expertise meter before the replay signal is real - the day a user realizes
the "challenge" is theater, every honest receipt is poisoned. Kill separate-minded subagents;
protect "nothing in the UI bluffs"; keep the brain 2D until position means something; scope
expertise to where exact-match is valid.
