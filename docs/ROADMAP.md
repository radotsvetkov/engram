# Engram — Product Roadmap

*The output of a vision pressure-test: founder vision + engineer synthesis, challenged by an
architect / strategist / design / skeptic panel, then consolidated. The scope gate for every
feature is one sentence:* **"Does this produce a signature a competitor can't?"** *If it doesn't
deepen verifiable memory, verifiable expertise, or verifiable dissent, it is sprawl and gets cut.*

## North star

Engram is the personal agent whose **conduct AND reasoning you can verify** — a glass-box brain
that signs not just what it did, but what it has come to believe about you and where its own
expertise disagreed with itself. The wedge is the signed BLAKE3/Ed25519 ledger applied to three
things no competitor can follow without first building that ledger:

- **Verifiable memory** — a tiny, always-loaded working context continuously distilled from the
  deep brain-graph, shown and signed.
- **Verifiable expertise** — skills that *measurably* improve via the replay→A/B→promote→revert
  loop, with the before/after gate signed.
- **Verifiable dissent** — an expertise-grounded objection to a plan, signed alongside the plan and
  the user's choice.

**One brain, not a crew.** The brain metaphor only earns its place where it pays rent
(working-vs-long-term memory; skill-cluster expertise) and is cut everywhere it's decoration.

## Verdicts on the proposed ideas

| Idea | Call | Why |
|---|---|---|
| Finish basics (connect-flows, MCP UI, project/session polish, templates) | **Endorse** | A UI that bluffs about its capabilities is an active trust violation. This *is* the vision, not a prerequisite. |
| Model picker "is static" | **Already done** | Provider/model/key hot-swap + `/v1/config/test` shipped (b30cfab). Only fix: honest "offline mock" state vs a fake live list. |
| "Consciousness" over SOUL/persona | **Refine** | Endorse the *mechanism* (distilled, signed, editable working memory). Ship the mechanism, **not the metaphysics** — no "conscious self" claims; they undercut verify-don't-trust. |
| 3D / anatomical / orbitable brain (incl. 2.5D point-cloud) | **Cut** | Anatomical position is *invented* (recall-relevance has no lobe) — decorative dishonesty in an honesty product. Orbit needs WebGL/Three.js (breaks no-build/no-CDN) or fragile per-frame camera math. Animate the **2D** brain instead. |
| Separate-mind subagents / assignable crew / per-agent subconscious | **Cut** | The scope bomb; the commodity 2026 quadrant (CrewAI/AutoGen). Duplicates context, kills zero-idle, *is* the black box we sell against. Keep only the existing bounded `delegate_task`. |
| Specialists as **context-lens "hats"** over the one shared brain, assignable to kanban cards | **Refine** | The only on-wedge framing of "multi-agent." A hat = bias recall + prefer a skill-cluster + prepend a distilled lens; the card's ledger slice records which hat ran. **Attach a lens, never mint a named agent.** |
| Skills-as-growing-expertise as the visible centerpiece | **Refine** | The unique self-improving mechanic, currently *invisible*. But `learn.rs` scores by exact-byte match vs gold output — so honest growth exists only for deterministic/tool-shaped skills. Show the real promote/revert moment; show "unverified" where there's no scored signal. Don't oversell "harder variant reuses prior solution" yet. |
| Auditable dissent (specialist objects; plan+objection+grounds+choice all signed) | **Refine → Phase 3** | Fuses both moats into "an adversary you can audit." But ungrounded prompt-dissent is "the base model in a costume." Fires **only** when grounded in real evidence (replay-win record + conflicting recalled memories). Gated on the expertise signal being real first. |
| Friendly MCP UI + a few templates | **Endorse** | Raw `mcp.json` is a trust leak; a form over the existing `/v1/config/mcp-test` is high-ROI. Templates = a *small bounded* starter set, never a marketplace. |
| Integration breadth (Signal/WhatsApp/Teams/Slack/Discord) | **Cut** | Six logos = me-too. Ship **one** (Telegram) end-to-end as the reference; leave the rest as the honest generic webhook. |

## Phases

**Phase 0 — Trust integrity: stop the UI from bluffing.** Highest-ROI, least glamorous, ships
first. One real Telegram connect/test/status flow (live, no restart, signed `channel.connect`);
honest offline-mock model state; friendly MCP form over `/v1/config/mcp-test`; deterministic,
obviously-saved projects/sessions; a small bounded template set; plainly state the key-custody
threat boundary in the verify view.
*Done when:* a second person can install it, connect Telegram, configure a live model + an MCP
server through friendly forms, create a project, and find **no surface that claims something the
system can't do** — every connect action a signed ledger entry.

**Phase 1 — Consciousness: working memory you can see, edit, and verify.** Distill a tiny (5–9 line)
always-loaded working context from the identity/semantic memories; drive the agent run from it
(retire SOUL.md as source of truth); render it as an editable panel where each line is
region-tinted and click-throughs to its source memory; every distillation + user edit is a signed,
revertible ledger entry. No self-awareness copy.

**Phase 1.5 — The 2D brain delivers "watch it grow," honestly.** Animate node-in / tier-promotion /
live count on the existing 2D Canvas brain; ~30–40 lines of pointer-parallax + depth-blur for a
dimensional read (no camera matrix); a **recall ribbon** under each answer showing which memories
were pulled in, each clickable to its node + ledger slice; searchable/filterable memories+skills.

**Phase 2 — Expertise made visible (and honest).** A Skills view with each skill's version ladder;
on a promotion, animate the *real* before/after replay score + ◇ ledger chip ("v3 beat v2:
0.82→0.94 on 11 replays"); "unverified — N runs" where there's no gold signal; specialist **hats**
on kanban cards as attachable context-lenses (bias recall + prefer cluster + prepend lens), ledger
records which hat ran. No spawn-a-named-agent flow.

**Phase 3 — Auditable dissent (gated on real expertise signal).** When an assigned lens has high
replay-confidence and the instruction conflicts with what that expertise predicts, surface a single
inline "Specialist note: based on N prior wins, X tends to fail; these 2 memories say Y; proceed?"
citing the specific skills + memories; sign plan + objection + grounds + choice as one ledger
artifact. Dissent fires **only** from cited evidence — never prompt-only theater.

## Biggest risk

**Sprawl-driven trust erosion.** Engram's one defensible asset is end-to-end honesty between what
the UI claims and what the system does. The most dangerous near-term move is shipping prompt-only
dissent or a vibes-based expertise meter before the replay signal is real — the day a user realizes
the "challenge" is theater, every honest receipt is poisoned. Kill separate-minded subagents;
protect "nothing in the UI bluffs"; keep the brain 2D until position means something; scope
expertise to where exact-match is valid.
