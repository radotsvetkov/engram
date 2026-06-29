//! Seed skills - the procedural memory a fresh brain is born with.
//!
//! On first boot (when no skills exist yet) the daemon installs one tiny, real WASM
//! skill and records a few accepted runs for it, so the dashboard immediately shows a
//! runnable skill and the learning loop has history to replay a candidate against.

use engram_skills::{Capability, NewSkill, Registry, Runtime};

/// An ASCII-uppercase skill: pure compute, no capabilities, deterministic.
const SHOUT_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 16384))
  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))
  (func (export "run") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32) (local $b i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
        (local.set $b (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
        (if (i32.and (i32.ge_u (local.get $b) (i32.const 97))
                     (i32.le_u (local.get $b) (i32.const 122)))
          (then (i32.store8 (i32.add (local.get $ptr) (local.get $i))
                            (i32.sub (local.get $b) (i32.const 32)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
            (i64.extend_i32_u (local.get $len)))))
"#;

/// An "ask" skill: forwards its input to the model through the gateway (the `llm`
/// egress capability). Demonstrates a sandboxed skill reaching the LLM - taint-gated,
/// metered, and audited - from the dashboard.
const ASK_WAT: &str = r#"
(module
  (import "engram" "llm" (func $llm (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 16384))
  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))
  (func (export "run") (param $ptr i32) (param $len i32) (result i64)
    (local $n i32)
    (local.set $n (call $llm (local.get $ptr) (local.get $len) (i32.const 4096) (i32.const 4096)))
    (if (i32.gt_s (local.get $n) (i32.const 4096)) (then (local.set $n (i32.const 4096))))
    (if (i32.lt_s (local.get $n) (i32.const 0)) (then (local.set $n (i32.const 0))))
    (i64.or (i64.shl (i64.extend_i32_u (i32.const 4096)) (i64.const 32))
            (i64.extend_i32_u (local.get $n)))))
"#;

/// Install seed skills if the registry is empty. Idempotent.
pub fn ensure_seed(registry: &Registry) -> Result<(), Box<dyn std::error::Error>> {
    if !registry.skills()?.is_empty() {
        return Ok(());
    }
    let wasm = wat::parse_str(SHOUT_WAT)?;
    let skill = NewSkill::wasm(
        "shout",
        "transform",
        "Uppercase the input text.",
        vec![],
        "exact_match",
    );
    let version = registry.install(skill, &wasm)?;
    for (input, gold) in [("hello", "HELLO"), ("engram", "ENGRAM"), ("rust", "RUST")] {
        registry.record_run("shout", version, input.as_bytes(), gold.as_bytes(), 1.0)?;
    }
    tracing::info!(version, "seeded skill 'shout'");

    let ask = wat::parse_str(ASK_WAT)?;
    registry.install(
        NewSkill::wasm(
            "ask",
            "thinking",
            "Forward the input to the model through the gateway.",
            vec![Capability::Llm],
            "helpfulness",
        ),
        &ask,
    )?;
    tracing::info!("seeded skill 'ask'");
    Ok(())
}

/// A built-in skill installed at boot. All seeds are stdlib-only Process(python3) scripts (JSON in on
/// stdin, JSON out on stdout) under `src/skills/`, so they need no `pip install` in the sandbox.
struct SeedSkill {
    id: &'static str,
    category: &'static str,
    description: &'static str,
    when_to_use: &'static str,
    /// Whether the skill reaches the network (gets the taint-gated `Net` capability).
    net: bool,
    source: &'static str,
}

/// The built-in skill LIBRARY. Keyless/free-API and pure-compute scripts, so they work the moment
/// they're seeded — no key friction. Each is signed on install; the user can disable, improve, upload
/// alongside, or download as a template. **Adding a skill is one row here + a script under
/// `src/skills/`** — the whole catalogue lives in this table.
const SEED_SKILLS: &[SeedSkill] = &[
    SeedSkill { id: "flight_search", category: "research", net: true,
        description: "Find real flights (cheapest fare + cheapest nonstop) between two airports via a flight DATA API instead of scraping. Input JSON {origin,destination,depart,return,...} (IATA codes, YYYY-MM-DD). Needs TRAVELPAYOUTS_TOKEN (free) or AMADEUS_* in the daemon env; without one it returns how_to_fix.",
        when_to_use: "the user asks for flights, airfare, plane tickets, or the cheapest/fastest way to fly between two cities",
        source: include_str!("skills/flight_search.py") },
    SeedSkill { id: "weather", category: "research", net: true,
        description: "Current conditions + a multi-day forecast for any place via the free, keyless Open-Meteo API. Input JSON {location} or {latitude,longitude}, optional {days}.",
        when_to_use: "the user asks about weather, temperature, rain, or the forecast for a place",
        source: include_str!("skills/weather.py") },
    SeedSkill { id: "email", category: "communication", net: true,
        description: "List, read, and send email via the himalaya CLI. Input JSON {action: list|read|send, ...}. Sending is egress and is refused on a tainted run. Needs himalaya + a configured account.",
        when_to_use: "the user asks to check, read, search, or send email / messages in their mailbox",
        source: include_str!("skills/email_tool.py") },
    SeedSkill { id: "wikipedia", category: "research", net: true,
        description: "Look up a topic on Wikipedia (keyless): search + the best match's summary and link. Input JSON {query, lang?}.",
        when_to_use: "the user asks who/what/where something is, or wants a factual summary of a topic, person, or place",
        source: include_str!("skills/wikipedia.py") },
    SeedSkill { id: "currency", category: "finance", net: true,
        description: "Convert money or get exchange rates (keyless, open.er-api.com). Input JSON {from, to, amount} — omit 'to' for all rates.",
        when_to_use: "the user asks to convert currencies or wants an exchange rate",
        source: include_str!("skills/currency.py") },
    SeedSkill { id: "crypto", category: "finance", net: true,
        description: "Current cryptocurrency prices via CoinGecko (keyless). Input JSON {ids, vs} (accepts tickers like btc/eth or CoinGecko ids).",
        when_to_use: "the user asks for the price of bitcoin, ethereum, or any cryptocurrency",
        source: include_str!("skills/crypto.py") },
    SeedSkill { id: "dictionary", category: "language", net: true,
        description: "Define a word (keyless, dictionaryapi.dev): phonetics + meanings by part of speech. Input JSON {word, lang?}.",
        when_to_use: "the user asks what a word means, for a definition, or for synonyms",
        source: include_str!("skills/dictionary.py") },
    SeedSkill { id: "rss", category: "research", net: true,
        description: "Read the latest items from any RSS/Atom feed (news, blogs, podcasts, releases). Input JSON {url, limit?}.",
        when_to_use: "the user wants the latest headlines/posts from a news site, blog, podcast, or any feed URL",
        source: include_str!("skills/rss.py") },
    SeedSkill { id: "github", category: "software", net: true,
        description: "Look up a public GitHub repo (keyless): stats, the latest release, or recent open issues. Input JSON {repo:'owner/name', what:'info|release|issues'}. GITHUB_TOKEN raises the rate limit.",
        when_to_use: "the user asks about a GitHub repository — its stars, latest release, or open issues",
        source: include_str!("skills/github.py") },
    SeedSkill { id: "calc", category: "compute", net: false,
        description: "Safely evaluate a math expression (no network): + - * / // % **, parentheses, and math functions (sqrt, sin, log, pi...). Parsed with a strict whitelist, never eval(). Input JSON {expr}.",
        when_to_use: "the user asks to compute, calculate, or evaluate a math expression",
        source: include_str!("skills/calc.py") },
    SeedSkill { id: "datetime", category: "compute", net: false,
        description: "Time across timezones + date math (no network, stdlib zoneinfo). Input JSON {action: now|convert|diff, tz/from/to/...}.",
        when_to_use: "the user asks the time in a timezone, to convert a time between zones, or the days between two dates",
        source: include_str!("skills/datetime_tool.py") },
    // --- batch 2: broad coverage (keyless / compute / keyed verticals) ---
    SeedSkill { id: "air_quality", category: "research", net: true,
        description: "Air quality (US AQI + pollutants) for any place via the free, keyless Open-Meteo air-quality API. Input JSON {location} or {latitude,longitude}.",
        when_to_use: "the user asks about air quality, pollution, AQI, or smog for a place",
        source: include_str!("skills/air_quality.py") },
    SeedSkill { id: "arxiv", category: "research", net: true,
        description: "Search arXiv for scientific papers (keyless): title, authors, abstract, link. Input JSON {query, max?}.",
        when_to_use: "the user asks for research papers, preprints, or scientific literature on a topic",
        source: include_str!("skills/arxiv.py") },
    SeedSkill { id: "country", category: "research", net: true,
        description: "Facts about a country via the keyless World Bank API: capital, region, income level, coordinates, latest population. Input JSON {name} (name or ISO code).",
        when_to_use: "the user asks about a country's capital, region, population, or basic facts",
        source: include_str!("skills/country.py") },
    SeedSkill { id: "holidays", category: "productivity", net: true,
        description: "Public holidays for a country/year (keyless, Nager.Date). Input JSON {country (ISO-2), year?}.",
        when_to_use: "the user asks about public holidays, bank holidays, or days off in a country",
        source: include_str!("skills/holidays.py") },
    SeedSkill { id: "hackernews", category: "research", net: true,
        description: "Top / new / best Hacker News stories (keyless). Input JSON {limit?, kind?: top|new|best}.",
        when_to_use: "the user asks what's trending on Hacker News or wants top tech stories",
        source: include_str!("skills/hackernews.py") },
    SeedSkill { id: "unfurl", category: "research", net: true,
        description: "Unfurl a URL into its title, description, and preview image (Open Graph; keyless, stdlib). Input JSON {url}.",
        when_to_use: "the user shares a link and wants its title/preview, or you need a page's metadata",
        source: include_str!("skills/unfurl.py") },
    SeedSkill { id: "ip_lookup", category: "research", net: true,
        description: "Geolocate and describe an IP address (keyless, ip-api): country, city, ISP, coords. Input JSON {ip?} (omit for the caller).",
        when_to_use: "the user asks where an IP is, or who an IP address belongs to",
        source: include_str!("skills/ip_lookup.py") },
    SeedSkill { id: "translate", category: "language", net: true,
        description: "Translate text between languages — keyless via MyMemory, or DeepL if DEEPL_API_KEY is set. Input JSON {text, to, from?}.",
        when_to_use: "the user asks to translate text into another language",
        source: include_str!("skills/translate.py") },
    SeedSkill { id: "text", category: "compute", net: false,
        description: "Text utilities (no network): word/char/line/sentence counts and transforms (upper, lower, title, slug, reverse). Input JSON {text, op?}.",
        when_to_use: "the user wants to count, slugify, or transform some text",
        source: include_str!("skills/text.py") },
    SeedSkill { id: "hash", category: "software", net: false,
        description: "Hashes (md5/sha1/sha256/sha512) and base64/hex/url encode-decode (no network). Input JSON {text, decode?}.",
        when_to_use: "the user asks to hash a string, or to base64/hex/url encode or decode something",
        source: include_str!("skills/hash.py") },
    SeedSkill { id: "genid", category: "software", net: false,
        description: "Generate UUIDs, secure passwords, or URL-safe tokens (no network, cryptographic randomness). Input JSON {what?: uuid|password|token, count?, length?}.",
        when_to_use: "the user asks for a UUID, a random/secure password, or a token",
        source: include_str!("skills/genid.py") },
    SeedSkill { id: "unit_convert", category: "compute", net: false,
        description: "Convert units of length, mass, temperature, data, speed, or time (no network). Input JSON {value, from, to}.",
        when_to_use: "the user asks to convert between units (km/mi, kg/lb, C/F, GB/MB, etc.)",
        source: include_str!("skills/unit_convert.py") },
    SeedSkill { id: "news", category: "research", net: true,
        description: "Recent news headlines on a topic (needs NEWSAPI_KEY; free tier). Input JSON {query, limit?}. Without a key, returns how_to_fix.",
        when_to_use: "the user asks for recent news or headlines about a topic or company",
        source: include_str!("skills/news.py") },
    SeedSkill { id: "stocks", category: "finance", net: true,
        description: "Stock quote — price, change, volume (needs ALPHAVANTAGE_KEY; free tier). Input JSON {symbol}. Without a key, returns how_to_fix.",
        when_to_use: "the user asks for a stock price or quote for a ticker symbol",
        source: include_str!("skills/stocks.py") },
];

/// Install any built-in skill from [`SEED_SKILLS`] that isn't already present. Id-guarded against the
/// FULL set (including disabled ones via `skills_all`) so a skill the user turned off or improved is
/// never resurrected or downgraded. Idempotent — safe to call on every boot.
pub fn ensure_seed_skills(registry: &Registry) -> Result<(), Box<dyn std::error::Error>> {
    let have: std::collections::HashSet<String> = registry.skills_all()?.into_iter().collect();
    for s in SEED_SKILLS {
        if have.contains(s.id) {
            continue;
        }
        let skill = NewSkill {
            id: s.id.into(),
            category: s.category.into(),
            description: s.description.into(),
            capabilities: if s.net { vec![Capability::Net] } else { vec![] },
            metric: "helpfulness".into(),
            runtime: Runtime::Process,
            interpreter: Some("python3".into()),
            when_to_use: Some(s.when_to_use.into()),
        };
        let version = registry.install(skill, s.source.as_bytes())?;
        tracing::info!(skill = s.id, version, "seeded skill");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::Ledger;
    use engram_skills::{manifest, SkillSigner};
    use std::sync::Arc;

    fn registry() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap());
        let reg = Registry::open(dir.path(), signer, ledger).unwrap();
        (dir, reg)
    }

    #[test]
    fn seed_skills_install_signed_and_idempotent() {
        let (_d, reg) = registry();
        ensure_seed_skills(&reg).unwrap();
        ensure_seed_skills(&reg).unwrap(); // second call is a no-op (no v2 for any skill)
        // Every skill in the table is installed, signed, Process/python3, v1, with the right caps.
        for s in SEED_SKILLS {
            let (signed, bytes) = reg
                .load_active(s.id)
                .unwrap_or_else(|_| panic!("seed skill '{}' was not installed", s.id));
            manifest::verify(&signed, &bytes, reg.verifying())
                .unwrap_or_else(|_| panic!("'{}' must be a valid signed skill", s.id));
            let m = &signed.manifest;
            assert_eq!(m.version, 1, "{}: idempotent install must not create a v2", s.id);
            assert_eq!(m.runtime, Runtime::Process, "{}", s.id);
            assert_eq!(m.interpreter.as_deref(), Some("python3"), "{}", s.id);
            assert_eq!(
                m.capabilities.contains(&Capability::Net),
                s.net,
                "{}: Net capability must match the table",
                s.id
            );
            assert!(!bytes.is_empty(), "{}: embedded source must not be empty", s.id);
        }
        // Spot-check: flight_search carries the real script and needs trust (Net + Process).
        let (signed, bytes) = reg.load_active("flight_search").unwrap();
        assert!(signed.manifest.requires_trust());
        assert!(bytes.windows(b"prices_for_dates".len()).any(|w| w == b"prices_for_dates"));
    }

    /// The WASM defaults and the Process-skill library share one registry and must not collide.
    #[test]
    fn seed_skills_coexist_with_wasm_defaults() {
        let (_d, reg) = registry();
        ensure_seed(&reg).unwrap();
        ensure_seed_skills(&reg).unwrap();
        let ids = reg.skills().unwrap();
        for want in [
            "shout", "ask", "flight_search", "weather", "email", "wikipedia", "currency", "calc",
            "datetime",
        ] {
            assert!(ids.iter().any(|s| s == want), "missing seeded skill {want}");
        }
    }
}
