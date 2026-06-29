# Web & live-data capabilities — why tasks failed, and the fix

> Origin: Engram kept failing real-world web tasks (the trigger: "find the cheapest flight
> Hamburg/Weeze → Tangier" returned nothing). This documents the root cause, the Hermes comparison,
> what was built, and the remaining roadmap. Companion to [ROADMAP.md](ROADMAP.md).

## Is it because Hermes uses Python? No.

Engram's skill substrate is **already polyglot**. A `Process` skill runs any interpreter
(`python3` by default, also `node`/`bash`/`go`/`ruby`) as `<interpreter> script < input` over the
shell backend ([skills_runtime.rs](../crates/engram-agent/src/skills_runtime.rs):191). The agent can
already author one at runtime via the `skill_author` tool
([skills_tools.rs](../crates/engram-agent/src/skills_tools.rs):205). It can `import`, call an API,
parse JSON, and rank results today. Language was never the gap.

Nuance worth knowing: under the **default local shell backend** the skill subprocess inherits
engramd's full environment (no `env_clear` in `run_process_skill`) — so a Python skill can read an
API key from `os.environ` right now, which is how the flight skill below gets its key without first
building a secret store.

## Root cause: a stack of capability gaps

In order of blast radius:

1. **No structured flight/travel data path (decisive).** Engram had zero flight/travel/maps/weather
   API skill. Its only route to flight data was driving the consumer SPA (Google Flights /
   Skyscanner / Kiwi) through a headless browser. Hermes-class travel results come from hitting a
   **structured endpoint**, not scraping a JS SPA.
2. **The fallback browser is instantly bot-flagged.** `browser_cdp.rs` launched headless Chrome with
   no User-Agent override and `navigator.webdriver === true`, no fingerprint/locale mask, no proxy.
   So Cloudflare/DataDome on exactly the sites flight prices live on serve a bot wall — and Engram
   then silently extracts the wall/consent text and "answers" from it.
3. **Extraction is a tag-stripper, not readability.** `html_to_text`
   ([tools.rs](../crates/engram-agent/src/tools.rs):187) drops `<script>`/`<style>` and collapses
   whitespace; CDP extract returns raw `body.innerText` truncated to 6000 chars. No main-content
   detection, so the model gets nav/footer/consent noise even when a page loads.
4. **No automatic vision fallback / SPA-wait on the one-shot path.** Vision exists
   (`vision_analyze`) but nothing auto-routes to a screenshot when text extraction returns a block
   page or empty body.

Net: Engram scraped a JS-heavy, bot-walled travel SPA with a fingerprintable browser and a
tag-stripper, with no structured-API fallback and no auto-vision recovery — so it read a consent page
and failed.

## Gap table: Hermes → Engram → fix

| Capability | Hermes | Engram (before) | Fix |
|---|---|---|---|
| Structured flight/travel data | dedicated registry tool hitting a typed endpoint | **missing** — only SPA scraping | **`flight_search` skill** ✅ built |
| Keyless live-data proof / API template | tools as registry entries; PTC composes them | **missing** | **`weather` skill** (Open-Meteo, keyless) ✅ built |
| Stealth / anti-detection browser | native Camoufox sidecar (proxy geoip, humanize) | headless Chrome, `webdriver=true`, default UA | **min-viable stealth on CDP** ✅ built; Camoufox sidecar = later |
| Readability extraction | Firecrawl/Exa/Tavily → clean markdown | tag-strip + 6000-char cut | `readability` Rust crate in fetch/extract — *roadmap* |
| Search with snippets | keyed Search API (Tavily/Brave/Serp) | scrapes Brave/DDG HTML, title::url only | keyed Brave/SerpAPI backend — *roadmap* |
| Vision read-the-page fallback | auto screenshot→multimodal when text insufficient | manual `vision_analyze` only | auto-trigger on empty/bot-wall — *roadmap* |
| Generic typed-API skill template | new APIs added as tools | bespoke each time | **`weather` doubles as the template** ✅ |
| Per-skill secret injection | `env_passthrough` opt-in per tool | inherits whole env, all-or-nothing | scoped secret env + scrub — *roadmap* |

## What was built (this pass)

### 1. `flight_search` skill — the #1 gap
[crates/engramd/src/flight_search.py](../crates/engramd/src/flight_search.py), seeded (signed at
boot with the brain key) by `ensure_flight_skill` in
[seed.rs](../crates/engramd/src/seed.rs), wired at [main.rs](../crates/engramd/src/main.rs).

- **Process / python3, stdlib only** (no `pip` in the sandbox), `Net` capability (taint-gated).
- Reads `{origin,destination,depart,return,adults,currency,direct}` (IATA codes, `YYYY-MM-DD` or
  `YYYY-MM`) on stdin; writes ranked fares + cheapest-nonstop + a summary on stdout.
- **Multi-provider**, picked by which env var is set:
  - `TRAVELPAYOUTS_TOKEN` — Aviasales Data API, **free**, cached cheapest fares. Best first option.
    Signup: <https://www.travelpayouts.com> (Tools → Data API).
  - `AMADEUS_CLIENT_ID` + `AMADEUS_CLIENT_SECRET` — real-time offers. Signup:
    <https://developers.amadeus.com/register>. **Set `AMADEUS_ENV=production`** — the test host
    returns a cached static subset and may return nothing for a route like NRN→TNG.
- With **no** key set it still succeeds and returns an actionable `how_to_fix` payload.

**Activate:** set one env var on the daemon (`TRAVELPAYOUTS_TOKEN=…`) and restart. The `when_to_use`
cue routes flight questions to the skill before any browser attempt.

### 2. `weather` skill — keyless live data + the API template
[crates/engramd/src/weather.py](../crates/engramd/src/weather.py), seeded by `ensure_weather_skill`.
Uses **Open-Meteo (free, no key)**, so it works the moment it's seeded — verified live (returned real
Tangier conditions + 3-day forecast). It is the reference scaffold to copy when minting new typed-API
skills (maps, finance, a SerpAPI Google-Flights fallback, …): stdlib-only, JSON-in/JSON-out,
fail-soft, `Net` capability, key-from-env.

### 3. Minimum-viable browser stealth
[browser_cdp.rs](../crates/engram-agent/src/browser_cdp.rs) — applied once per session in
`apply_stealth`, best-effort (a failure degrades to prior behaviour, never breaks browsing):
- launch flag `--disable-blink-features=AutomationControlled` (drops `navigator.webdriver` at source),
- `Emulation.setUserAgentOverride` with a real Chrome UA (default headless UA says `HeadlessChrome`),
- `Page.addScriptToEvaluateOnNewDocument` masking `navigator.webdriver`/`plugins`/`languages`/`chrome`,
- `Emulation.setTimezoneOverride` for a stable locale.

This is the cross-cutting fix: it helps *every* bot-walled site, not just flights.

### 4. Readability extraction (dependency-free)
[tools.rs](../crates/engram-agent/src/tools.rs) `html_to_text` — rewritten from a bare tag-stripper
into a real readability pass, with **no new dependencies** (honours the small-build constraint):
prefers the `<main>`/`<article>` region, drops boilerplate (nav/header/footer/script/style/forms),
preserves block structure as newlines, decodes HTML entities, and leads with the page `<title>`.
`web_fetch` returns this directly, so every web read is cleaner. 5 unit tests, green.

## Remaining roadmap (ranked, with recipes)

1. **Auto vision-read fallback** (S). When extraction returns empty/very-short text or matches a
   bot-wall/consent heuristic, auto-capture a screenshot and route it through the already-wired vision
   model, returning that as the observation.
2. **Search-API backend with snippets** (S). Add a keyed Brave Search API (or SerpAPI) backend to
   `web_search` returning `title+url+snippet`; keep the HTML scrape as last-resort fallback.
3. **Camoufox-class stealth sidecar** (weeks). A patched-Firefox sidecar over REST with residential
   proxy geoip + humanized input, for the hardest anti-bot walls. The min-viable CDP stealth above
   covers the common cases first.
4. **Scoped per-skill secrets** (M). Inject only the secrets a skill needs and `env_clear` the rest on
   the local backend, so keys are least-privilege instead of all-or-nothing.

> A full Hermes-vs-Engram capability audit (every tool + skill) and the prioritized "ultimate
> coverage" roadmap are tracked separately — see the audit deliverable.

## Verification

- `cargo test -p engramd --bin engramd seed::` — flight + weather seeds install signed, idempotent,
  coexist with `shout`/`ask` (3 tests, green).
- `weather.py` returns live Open-Meteo data; `flight_search.py` no-key/bad-input paths verified.
- `cargo test -p engram-agent --lib` — 49 tests green incl. 5 readability tests; stealth compiles
  under `--features browser-cdp`.
- Full `cargo test -p engramd --bin engramd` — 35 tests green.
