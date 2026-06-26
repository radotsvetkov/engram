<!-- The canonical product + design spec for the Engram desktop app. Produced from
competitor research (Hermes Desktop, Claude Code Desktop, ChatGPT — June 2026) and an
adversarial multi-direction design challenge, then grounded against the codebase. -->

# Engram Desktop — Product & Design Spec

> Engram proves what it did, remembers who you are, and costs nothing at rest.

## Product vision

## 1. The One-Line Thesis

**Engram is the only AI workspace you can *trust on your own terms*: a local agent that proves what it did, remembers who you are, and costs nothing when you're not using it.** Claude and Hermes give you a faster horse — more panes, more models, more sessions. Engram gives you a *second brain with a conscience*: every action it takes is cryptographically signed and verifiable offline, and it visibly grows a model of you over time. They optimize for throughput. We optimize for *trust and intimacy*.

## 2. Target Persona — "Dana, the Sovereign Operator"

Dana, 38, runs technical operations for a small fintech and moonlights as an independent consultant. Her day is a relay race across regulated client data, half-written automations, API keys she's terrified of leaking, and a backlog of "I'll remember that" context that she never does. Today she cobbles it together: ChatGPT for thinking, Claude Code for the repo, a password manager for keys, a Notion doc as her "what did the agent actually touch?" paper trail, and a gnawing anxiety every time an agent runs a shell command she didn't fully read.

**Her frustration:** she can't *prove* what her AI did. When a client asks "did anything leave our environment?" she has a shrug and a screenshot. And every tool forgets her the moment the tab closes — she re-explains her stack, her preferences, her constraints, daily.

**Her aspiration:** one calm instrument she can hand sensitive work to and *vouch for* — to her clients, her compliance officer, and herself.

**A day:** 8:40am, she opens Engram; the daemon was asleep, costing nothing overnight. The trust spine reads *"ledger verified · 1,204."* She switches to the *Apex Capital* project — its sessions, keys, and memory snap in together. She drags a kanban card from Review to Done; the card carries its signed ledger slice. A task wants to hit an external API; the taint indicator shows the boundary and asks once. At 6pm she exports a glass-box receipt for the client. The agent now *knows* she always wants diffs before writes — she never told it twice.

## 3. Jobs-To-Be-Done (ranked)

1. **"Prove to me — and to others — exactly what the agent did."** (The trust job. Nobody else does this.)
2. **"Remember me across sessions so I stop re-explaining myself."** (The intimacy job.)
3. **"Keep my work in coherent project worlds I switch between cleanly."** (The context job.)
4. **"Let me run real work without fear it leaks or runs amok."** (The safety job — taint, approvals.)
5. **"Be there instantly, cost nothing idle."** (The economics job — zero-idle.)
6. **"Let me drive fast — chat, models, palette, terminal — without ceremony."** (The fluency job; parity table-stakes.)

## 4. The Indispensable Hook

**The receipt you can verify, and the brain that remembers.** The single feeling that makes Dana unable to go back: *certainty.* Every other tool asks her to trust a black box. Engram hands her a signed, hash-chained ledger she can verify offline, on a machine that never saw Engram. The ambient **"ledger verified · N"** spine becomes a heartbeat she checks the way she checks a lock.

The daily dependency is the **growing model of you**: a visible Memory that thickens — identity, episodic, semantic regions; hot/warm/cold tiers — so the agent gets measurably *more yours* every week. Leaving Engram means abandoning a brain that knows you and starting over with amnesiacs. That's the lock-in Jobs understood: not a feature, a *relationship you'd grieve to lose.*

The innovation to lean on: **the Receipt** as a first-class object — a verifiable, portable, glass-box artifact of any run. No competitor can copy it without re-architecting around a signed ledger. It's our defensible "blue bubble."

## 5. The Product's Persona / Voice

**A quiet, precise instrument — a trusted second brain, never a chatbot mascot.** Engram is *calm*. It speaks in short, certain, declarative lines: "Verified." "3 files touched." "Nothing left this machine." It never exclaims, never anthropomorphizes cutely, never says "Sure!" Motion is *settled and physical* — things snap, breathe, and rest; nothing bounces for attention. The trust spine glows once when a chain verifies, then goes still. Copy is the voice of a Swiss watchmaker who happens to know you: spare, exact, warm at the edges. Confidence shown, never claimed.

## 6. Three Things We Must NOT Copy

1. **Hermes' "surface every model you know."** That's a junk drawer. **Instead:** a curated, opinionated model picker — a few right choices, with hot-swap mid-thought; advanced models behind one fold.
2. **Claude Code's drag-anywhere multi-pane grid.** Powerful, but it makes *you* the window manager. **Instead:** one calm primary surface with a *contextual* right rail that appears only when there's something to see (a diff, a preview, a receipt). The agent arranges; you don't.
3. **The sprawl of disconnected config screens (both have ~9 settings categories scattered).** **Instead:** one **Cmd-, command-center popup** with clean categories (Workspace, Appearance, Voice, Memory, Gateways, Tools, Keys, Integrations, Advanced), searchable from the same Cmd-K palette. One door, not nine.

The throughline: **competitors give you controls; we make decisions so you can stay in flow.** Restraint as a feature.

## 7. The Emotional Arc

- **First run (minute 1):** *Relief, then quiet awe.* One overlay, skippable. First action runs — and the spine lights: *"ledger verified · 1."* She thinks: *it's already keeping receipts.* The brain shows its first faint memory forming.
- **Week 1 (daily use):** *Growing trust.* Projects snap cleanly. The agent stops asking things it should know. She catches herself glancing at the trust spine for reassurance — and getting it. She exports her first receipt and feels, for the first time, that she can *vouch* for an AI.
- **Month 1 ("I depend on this"):** *Quiet dependency.* The Memory is visibly *hers* — a deepening model she'd never rebuild elsewhere. She's handed Engram work she'd never trust to a black box. When a colleague shows her Hermes, she sees more buttons and feels the lack: *no receipt, no memory of me, and it's burning money sitting idle.* She doesn't switch. She can't — Engram is the only one that *remembers her and can prove its work.*

That's the moat made emotional: **certainty you can verify, intimacy you'd grieve to lose, and an instrument calm enough to disappear into.**

---

*A local AI workspace that proves what it did, remembers who you are, and costs nothing at rest.*

---

## 1. PRODUCT THESIS · PERSONA · HOOK

**Thesis.** Every competitor *shows you activity*; Engram *vouches for it*. Hermes and Claude Code give you a faster horse — more models, more panes, more sessions. Engram gives you a second brain with a conscience: one calm primary surface where every action the agent (or you) takes is hash-chained, Ed25519-signed, and verifiable offline on a machine that never ran Engram — and where the agent visibly grows a model of *you* that you'd grieve to leave. We optimize for trust and intimacy, not throughput. Restraint is the feature: the agent arranges the screen, you don't.

**Persona — "Dana, the Sovereign Operator."** 38, runs technical operations at a small fintech, moonlights as an independent consultant across regulated client data. Today she stitches together ChatGPT, Claude Code, a password manager, and a Notion doc as her "what did the agent touch?" paper trail — with a knot of anxiety every time an agent runs a shell command she didn't fully read. She can't *prove* what her AI did, and every tool forgets her the moment the tab closes. Her aspiration: one calm instrument she can hand sensitive work to and *vouch for* — to her clients, her compliance officer, and herself.

**The indispensable hook.** *The receipt you can verify, and the brain that remembers you.* The feeling that makes Dana unable to go back is **certainty**: the ambient `ledger verified · N` spine is a heartbeat she checks like a lock, and any task exports as a portable glass-box proof file she can validate offline. The daily dependency is the **recall ribbon** — every answer shows which memories of her it drew on, so the agent feels measurably *more hers* each week. Leaving means abandoning a brain that knows her and a proof trail no black box can reproduce.

---

## 2. THE INNOVATIVE CORE

Three signature features, each true in today's code, each a real UI.

**(A) The Trust Spine + the Receipt — verifiable agency.** A persistent element in the **bottom status bar**, present on every screen, reading `◇ Ledger verified · 1,204` (center), the **zero-idle daemon dot** `engramd · sleeping` (left, dims to sell zero-resident-cost as a feature), and **taint state** `No egress` / `Egress: 2 allowed` (right). The spine glows teal once when a new entry chains, then goes still — confidence shown, never claimed. Every task card and any message carries a `◇` chip; right-click → **Export Receipt** writes a self-contained file that `verify_file` validates with the daemon stopped. This is the moat that exists today (ledger.rs:253) and that no competitor can bolt on.

**(B) The Recall Ribbon — intimacy felt every message.** Beneath each agent turn, a thin row of memory chips shows *what was pulled into this answer*: `episodic · "prefers diffs before writes"`, `identity · "fintech, regulated data"`. Each chip is colored by region and clickable → jumps to that memory in the Memory view with its provenance receipt. This rides directly on existing recall output (`semantic_rank`, region, match-reason in store.rs:114). It is the single highest-leverage element in the product: it makes the brain *felt* without a theatrical visualization sitting on placeholder embeddings.

**(C) The Memory Atlas — honest now, theatrical later.** The Memory view is a calm, legible **region/tier atlas** reading the existing `MemoryAtlas` stats: three regions (identity / episodic / semantic) as columns, hot/warm/cold tiers as bands, node counts and salience as bars. Click any memory → provenance (which signed session created it), pin, or forget. We explicitly **do not** ship a 3D force-graph at launch — its "associative edges" would be noise over a trigram-hash placeholder embedder (embed.rs:73). The force map is a Phase-3 upgrade *after* the gateway embedder makes clusters real. Sell what's true.

---

## 3. INFORMATION ARCHITECTURE

One window. Three primary zones plus two thin bars. The agent arranges; you never become the window manager.

```
┌──┬──────────────┬─────────────────────────────────┬───────────────┐
│  │ SIDEBAR 260px │  TOP BAR 28px: Proj / Session · Trace·Normal·Brief · ⊙usage · ⌘K │
│R │ ────────────  ├─────────────────────────────────┼───────────────┤
│A │ ▾ Project ⌘P  │                                 │  RIGHT RAIL   │
│I │ ★ Favorites   │         STAGE                   │  (contextual, │
│L │ ────────────  │   chat · kanban · memory ·      │   collapsible)│
│56│ Sessions      │   schedule · skills             │  Preview ·    │
│  │  ◦ Running    │                                 │  Diff ·       │
│  │  ◦ Waiting    │   recall ribbon under answers   │  Receipt      │
│  │  ◦ Done       │                                 │               │
│  │  + New ses.   │                                 │               │
│  ├──────────────┴─────────────────────────────────┴───────────────┤
│◇ │ STATUS 24px: engramd·sleeping │ ◇ Ledger verified·1,204 │ No egress │
└──┴──────────────────────────────────────────────────────────────────┘
```

**Left rail (56px, icon-only, always visible).** Firing-synapse mark at top. Five destinations: **Console** (chat home), **Tasks**, **Schedule**, **Memory**, **Skills**. Pinned at the bottom: the **Trust Spine glyph** `◇N` and the **Settings gear**. Five destinations, not six — no separate Terminal icon (it's a pane, §5).

**Sidebar (260px, collapse `⌘B`).** The projects→sessions→favorites model:
- **Project switcher header** (`⌘P`) — active project name + mark. Switching swaps the *entire* context: sessions, working dir, SOUL.md persona, default gateway/model, keys, and taint policy. This is ChatGPT Projects, but a project here also scopes security and identity.
- **★ Favorites** — a starred band directly under the project: pinned sessions, drag-reorderable, survive archiving.
- **Sessions** — grouped by status (**Running / Waiting on you / Done / Archived-collapsed**), newest first. Each row: 3px left status hairline, title, model glyph, a `◇` micro-badge if its ledger slice verifies, and a 1px **breathing underline** when running (never a spinner). Search-by-id + filter chips at top.
- Footer: **+ New session**, **Archived** disclosure.

**Top bar (28px).** Left: breadcrumb `Project / Session` (inline-editable). Center: **Trace · Normal · Brief** segmented control (transparency dial for tool activity). Right: **usage ring** (context-window fill + session spend), `⌘K` button, **Side-chat** `⌘;`, per-session **Safety pill** (Guarded / YOLO — never global).

**Right rail (contextual, collapsible — appears only when there's something to see).** Tabbed: **Preview** (HTML/PDF/local servers/tool output), **Diff**, **Receipt** (the signed ledger slice for the current turn). Pulls from the right; never covers the conversation.

**Status bar (24px).** The Trust Spine, described in §2(A).

---

## 4. SETTINGS COMMAND-CENTER

Opens with **`⌘,`** or the gear — a **centered modal** (760×560, dimmed backdrop), never a route, so you never lose your place. Two-pane: category list (left), detail panel (right). A **search field at top filters across every setting in every category** to the individual control (type "egress" → jumps to the taint toggle in Tools). `Esc` or `⌘,` again dismisses. Every change saves live with inline `✓ saved`; sensitive changes (keys, taint policy) write a signed ledger entry shown as a tiny receipt. One door, not nine.

Reconciled category list (user's wants + competitor patterns):

1. **Workspace** — projects, working dir, default project, session retention/auto-archive.
2. **Appearance** — theme (graphite default), accent, density (Comfortable/Compact), font, zoom, motion-reduce.
3. **Voice** — STT/TTS engine, voice, push-to-talk key, auto-speak replies.
4. **Memory & Context** — region weights, hot/warm/cold tier thresholds, recall blend (FTS5 ↔ vector slider), context budget per session, forget/pin.
5. **Gateways** — Anthropic / OpenAI / OpenRouter / Ollama, live hot-swap default, per-project default, health pings.
6. **Tools** — MCP servers (add/health/scopes), backend installs, **taint/no-egress policy** with a visual allowlist, dangerous-command policy (default YOLO off).
7. **Keys** — API keys + OAuth accounts, OS-keychain encrypted, last-used receipts, reveal-on-auth.
8. **Integrations** — messaging surfaces, webhooks, external triggers.
9. **Keyboard** — full rebindable map, chords, conflict detection.
10. **Templates** — session/project starters, prompt snippets, persona presets.
11. **Chat** — streaming density, default view-mode, code-block theme, composer history depth.
12. **Messaging** — channels, notification routing, scheduled-job delivery.
13. **SOUL.md** — live persona editor for the active project.
14. **Advanced** — daemon controls (idle-exit seconds), ledger export / verify-offline, vector index rebuild, danger zone (rotate signing key).
15. **About / FAQ** — version, ledger root hash, the moat explained plainly, inline offline-verify how-to.

---

## 5. CHAT · TASKS · PALETTE · TERMINAL

**Chat.** A generous single column (~720px reading width) against a quiet stage. The **composer** is a calm slab pinned to the bottom, growing upward (to ~8 lines). Left of the field: **model/effort picker** (`⌘M`) — a popover showing the **curated** models for the project's gateway plus an "All models" fold (we curate; Hermes dumps everything), with reasoning-effort and fast-mode toggles, **sticky per project**. Then attach (drag-drop anywhere onto the stage highlights a drop target) and **voice** push-to-talk. `↑` recalls composer history; queued messages render as **editable chips** above the field before send. Streaming **tool activity** renders as collapsible **activity lines** (glyph · verb · target · latency · `◇` when chained); Brief collapses them to one ribbon, Trace expands args/output. Under each answer: the **recall ribbon** (§2B). **Side-chat** (`⌘;`) forks a scratch thread into the right rail that shares context but never contaminates the main transcript.

**Tasks / Kanban.** Columns **Queued · Running · Waiting on you · Done** (configurable). Real **drag-and-drop** (pointer + keyboard: grab `Space`, move arrows; 120ms spring, drop-shadow lift). Dragging to **Running** *starts* the agent; dragging to **Done** requires the card's receipt to verify. Each card is a **glass-box signed card**: title, owning-session link, model, a sparkline of tool calls, elapsed/cost, and a **Receipt chip** `◇ chained` / `⚠ unverified`. Click flips to the **ledger slice** — the ordered, hash-chained, Ed25519-signed list of every state change, with a **Verify** button that re-walks the chain offline and stamps `✓ verified · sealed`. A task links bidirectionally to its session ("Open session" reconstitutes the live thread).

**Command Palette (`⌘K`).** One palette, everywhere, toggles to dismiss, visual feedback after run. Indexes **navigation** (every project, session, favorite, task, memory region), **actions** (new session, switch model, change view-mode, start/stop task, toggle YOLO, verify ledger, new cron, export receipt), **settings** (deep-link to any category/control), **skills**, and **memory search** (type a thought → recall hits inline). Mode-prefixes: `>` actions, `/` files, `@` sessions, `:` settings. Each result shows its keybinding (teaches shortcuts passively); state-changing actions confirm with `◇ recorded`.

**Terminal · Files · Preview.** An opinionated **2-up split** (not a free drag-grid — calmer than Claude Code). Toggle panes from the top bar: **Terminal** (real PTY scoped to the project working dir, taint state shown in its prompt), **File tree** (persistent, live-watching, click to open an inline editor with a diff gutter / accept-reject hunks), **Preview** (right rail). Layout persists per project. **The load-bearing differentiator:** human-typed terminal commands flow through the *same* taint/approval/ledger pipeline as the agent's — a person's `rm -rf` gets guarded and signed identically. The box has no human-shaped hole.

---

## 6. VISUAL DESIGN SYSTEM

**Color roles.** Base graphite `--bg-0 #0c0d0f` (app) · `--bg-1 #131618` (panels) · `--bg-2 #1a1e21` (cards/inputs) · hairline `--line #23282c`. Text `--fg #e8edf0` / muted `--fg-2 #9aa4ab` / faint `--fg-3 #5d666c`. Single accent **teal `#45c8a8`** — the *trust color*, used only where trust is real: verified `◇`, the spine glow, active nav, focus ring, send button. Semantic: amber `#e0a32f` (taint armed / waiting) · red `#d6675e` (danger / unverified) · region tints for the atlas only (identity teal / episodic cyan / semantic violet `#8a7cff`). Teal is earned, never decorative; red never dominates.

**Type.** UI: Inter (or SF). Mono: **JetBrains Mono** for hashes, ledger heights, terminal, code, model ids — they should *feel* cryptographic; always dimmed, truncated mid (`a3f2…9c1`). Scale: 11 / 12 / 13 / 15 / 19 / 24. Body 13 / 1.55.

**Spacing & density.** 4px base grid; 8 / 12 / 16 / 24 rhythm. Two densities: Comfortable (32px rows, default), Compact (28px). Dense-but-breathing — a pro instrument, never cramped, never a toy.

**Radius.** 6px chips · 8px cards · 12px modal/composer · full for dots/avatars.

**Motion.** Calm and physical: 120–180ms ease-out for state, spring only on drag (`scale(1.02)` + shadow). **No spinners** — running uses a 1px breathing underline (1.6s). View-mode change cross-fades activity lines (150ms). The two sacred animations: the spine's single 200ms teal breath on each signed entry, and a recall chip brightening when its node is hovered. The sleep dot fades, never blinks. `prefers-reduced-motion` reduces all to opacity fades.

**Iconography.** Thin 1.5px line icons, 16/20px. The firing-synapse mark anchors the rail. The chained-link `◇` is the trust glyph everywhere proof appears.

---

## 7. KEYBOARD MODEL

| Shortcut | Action |
|---|---|
| `⌘K` | Command palette (toggle to dismiss) |
| `⌘,` | Settings command-center (toggle) |
| `⌘P` | Project switcher |
| `⌘/` | Switch session (quick-switcher) |
| `⌘N` | New session |
| `⌘;` | Side-chat / branch current thread |
| `⌘M` | Model / effort picker |
| `⌘B` | Toggle sidebar |
| `⌘⌥P` | Toggle right rail (Preview) |
| `` ⌃` `` | Toggle Terminal pane |
| `⌘⇧V` | Voice / push-to-talk |
| `⌘⇧R` | Verify ledger (offline) |
| `↑ / ↓` | Composer history |
| `Space` then arrows | Grab + move kanban card (a11y) |
| `F` | Flip task card to its receipt |
| `Esc` | Dismiss modal / palette / side-chat |

All shortcuts rebindable in Settings → Keyboard with conflict detection.

---

## 8. PHASED BUILD ROADMAP

**Phase 1 — the shell (all frontend-feasible against today's daemon + 846-line `index.html`).** Ordered build checklist:

1. **Design tokens** — install the §6 color/type/spacing/radius/motion variables as CSS custom properties; retheme the existing dashboard to graphite + teal.
2. **App shell** — the three-zone layout: 56px rail, 260px sidebar, stage, 24px status bar.
3. **Trust Spine in the status bar** — wire `◇ Ledger verified · N` + zero-idle daemon dot + taint state to existing ledger/daemon endpoints. *(Ship the moat first.)*
4. **Command palette (`⌘K`)** — navigation + actions + settings deep-links over current views.
5. **Settings command-center modal (`⌘,`)** — two-pane, cross-category search, the 15 categories wired to existing config.
6. **Chat polish** — generous column, composer slab, `⌘M` curated model picker, collapsible activity lines, Trace/Normal/Brief control, queued-message chips.
7. **Recall ribbon** — render memory chips under each answer from existing recall output (`semantic_rank` + region). *(The intimacy engine.)*
8. **Drag-drop kanban** — pointer + keyboard DnD, glass-box card with `◇` chip, flip-to-receipt + offline **Verify**.
9. **Sidebar sessions** — status grouping, favorites band, project-switcher header (UI scaffold; backend in Phase 2).
10. **Memory Atlas** — honest region/tier view over `MemoryAtlas` stats; pin/forget/provenance.

**Phase 2 — backend depth.** Projects→sessions backend (per-project working dir, SOUL.md, gateway, keys, taint policy, context-swap); favorites persistence; the in-app **Terminal PTY** with the unified taint/approval/**ledger pipeline for human-typed commands**; file tree + inline diff editor; right-rail Preview auto-surfacing; portable **Export Receipt** files.

**Phase 3 — reach + delight.** Voice I/O; messaging integrations + webhooks; templates/scaffolds; the **force-graph Brain Map** upgrade — *gated on the real gateway embedder landing* so clusters and edges are meaningful, not noise; multi-device sync; collaboration/shared receipts.

---

Grounding files for the build: `/Users/radotsvetkov/Desktop/HYT/aut-agent/crates/engramd/assets/index.html` (the 846-line single-page UI to extend — keep the no-build-step approach), `/Users/radotsvetkov/Desktop/HYT/aut-agent/desktop/src-tauri/src/main.rs` (Tauri shell), `/Users/radotsvetkov/Desktop/HYT/aut-agent/crates/engram-core/src/ledger.rs` (offline `verify_file` at line 253 — the Trust Spine + receipts), `/Users/radotsvetkov/Desktop/HYT/aut-agent/crates/engram-memory/src/store.rs` (`MemoryAtlas` + `semantic_rank` at lines 114/366 — Memory Atlas + recall ribbon), `/Users/radotsvetkov/Desktop/HYT/aut-agent/crates/engram-memory/src/embed.rs` (the `trigram-hash-v1` placeholder at line 73 — the constraint that gates the force-graph to Phase 3).
