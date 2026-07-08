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
    // --- batch 3: maps/dev/data depth (keyless / compute / one keyed) ---
    SeedSkill { id: "geocode", category: "research", net: true,
        description: "Forward + reverse geocoding via Nominatim/OpenStreetMap (keyless). Input JSON {query} (place -> coords) or {latitude,longitude} (coords -> address).",
        when_to_use: "the user wants the coordinates of a place, or the address at a lat/lon",
        source: include_str!("skills/geocode.py") },
    SeedSkill { id: "directions", category: "research", net: true,
        description: "Driving distance + time between two places via OSRM (keyless); falls back to straight-line distance if routing is unavailable. Input JSON {from, to} (names or [lon,lat]).",
        when_to_use: "the user asks how far apart two places are or how long it takes to drive between them",
        source: include_str!("skills/directions.py") },
    SeedSkill { id: "sunrise_sunset", category: "research", net: true,
        description: "Sunrise, sunset, and twilight times for a place/coords (keyless). Input JSON {location} or {latitude,longitude}, optional {date}.",
        when_to_use: "the user asks about sunrise, sunset, golden hour, or day length for a place",
        source: include_str!("skills/sunrise_sunset.py") },
    SeedSkill { id: "wayback", category: "research", net: true,
        description: "Find an archived snapshot of a URL in the Internet Archive Wayback Machine (keyless). Input JSON {url, timestamp?}.",
        when_to_use: "the user wants an old/archived version of a web page, or asks if a page was archived",
        source: include_str!("skills/wayback.py") },
    SeedSkill { id: "books", category: "research", net: true,
        description: "Search books via OpenLibrary (keyless): title, authors, year, ISBN. Input JSON {query, limit?}.",
        when_to_use: "the user asks about a book, its author, or wants to find books on a topic",
        source: include_str!("skills/books.py") },
    SeedSkill { id: "stackoverflow", category: "software", net: true,
        description: "Search Stack Overflow Q&A (keyless StackExchange API): titles, scores, answer counts, links. Input JSON {query, limit?}.",
        when_to_use: "the user has a programming error or how-to question that likely has a Stack Overflow answer",
        source: include_str!("skills/stackoverflow.py") },
    SeedSkill { id: "pypi", category: "software", net: true,
        description: "Look up a Python package on PyPI (keyless): version, summary, license, requires-python, homepage. Input JSON {package}.",
        when_to_use: "the user asks about a Python/pip package — its latest version, license, or what it is",
        source: include_str!("skills/pypi.py") },
    SeedSkill { id: "npm", category: "software", net: true,
        description: "Look up an npm package (keyless): latest version, description, license, repository. Input JSON {package}.",
        when_to_use: "the user asks about an npm/Node package — its latest version, license, or what it is",
        source: include_str!("skills/npm.py") },
    SeedSkill { id: "dns", category: "software", net: true,
        description: "DNS lookup via Google DNS-over-HTTPS (keyless): A/AAAA/MX/TXT/NS/CNAME/SOA records. Input JSON {name, type?}.",
        when_to_use: "the user asks for DNS records, the IP behind a domain, or MX/TXT/NS records",
        source: include_str!("skills/dns.py") },
    SeedSkill { id: "pwned", category: "software", net: true,
        description: "Check if a password has appeared in known data breaches WITHOUT sending it — Have I Been Pwned k-anonymity (keyless). Input JSON {password}. Never transmits or echoes the password.",
        when_to_use: "the user wants to know if a password is compromised or has been seen in a breach",
        source: include_str!("skills/pwned.py") },
    SeedSkill { id: "color", category: "compute", net: false,
        description: "Color utilities (no network): hex <-> rgb <-> hsl, WCAG luminance + contrast, best black/white text. Input JSON {color}.",
        when_to_use: "the user gives a color and wants its rgb/hsl, or which text color contrasts best",
        source: include_str!("skills/color.py") },
    SeedSkill { id: "csv_stats", category: "compute", net: false,
        description: "Per-column statistics for CSV text (no network): count, min/max/mean/median/sum for numeric columns; unique/top for text. Input JSON {csv, delimiter?}.",
        when_to_use: "the user pastes CSV/tabular data and wants summary statistics per column",
        source: include_str!("skills/csv_stats.py") },
    SeedSkill { id: "regex", category: "compute", net: false,
        description: "Test a regular expression against text (no network): matches, groups, positions, fullmatch. Input JSON {pattern, text, flags?}.",
        when_to_use: "the user wants to test, debug, or apply a regex to some text",
        source: include_str!("skills/regex.py") },
    SeedSkill { id: "json_tools", category: "compute", net: false,
        description: "Inspect/transform JSON (no network, a tiny jq): pretty, minify, keys, length, or get a dotted path. Input JSON {json, op?, path?}.",
        when_to_use: "the user wants to pretty-print, minify, or pull a value out of some JSON",
        source: include_str!("skills/json_tools.py") },
    SeedSkill { id: "diff", category: "compute", net: false,
        description: "Unified diff between two texts (no network): the diff plus added/removed counts. Input JSON {a, b, label_a?, label_b?}.",
        when_to_use: "the user wants to compare two pieces of text or see what changed between them",
        source: include_str!("skills/diff.py") },
    SeedSkill { id: "youtube", category: "research", net: true,
        description: "Search YouTube videos (needs YOUTUBE_API_KEY; free tier). Input JSON {query, limit?}. Without a key, returns how_to_fix.",
        when_to_use: "the user asks to find YouTube videos or a channel's content on a topic",
        source: include_str!("skills/youtube.py") },
    // --- batch 4: finance, startup/strategy frameworks, marketing/growth/SEO, security, dev tools ---
    SeedSkill { id: "compound_interest", category: "finance", net: false,
        description: "Compound interest / future value calculator with optional periodic contributions (ordinary annuity). Input JSON {principal, rate, years, compounds_per_year?, contribution?}.",
        when_to_use: "the user asks to project savings growth, compound interest, or future value of an investment with regular deposits",
        source: include_str!("skills/compound_interest.py") },
    SeedSkill { id: "loan_amortization", category: "finance", net: false,
        description: "Loan amortization schedule and payoff simulator with optional extra monthly payments; yearly summary for loans over 3 years. Input JSON {principal, annual_rate, years, extra_payment?}.",
        when_to_use: "the user asks for a loan/mortgage payment amount, amortization schedule, or payoff time with extra payments",
        source: include_str!("skills/loan_amortization.py") },
    SeedSkill { id: "npv_irr", category: "finance", net: false,
        description: "Net present value, internal rate of return (bisection root-find), and simple payback period for a cashflow series. Input JSON {rate, cashflows}.",
        when_to_use: "the user asks to evaluate an investment's NPV, IRR, or payback period from a series of cashflows",
        source: include_str!("skills/npv_irr.py") },
    SeedSkill { id: "unit_economics", category: "finance", net: false,
        description: "LTV, LTV:CAC ratio, and CAC payback period from CAC, ARPU, gross margin, and churn. Input JSON {cac, arpu, gross_margin_pct, churn_rate_pct}.",
        when_to_use: "the user asks whether their unit economics or LTV:CAC ratio are healthy",
        source: include_str!("skills/unit_economics.py") },
    SeedSkill { id: "runway_burn", category: "finance", net: false,
        description: "Cash runway in months and a projected runway end date from cash balance and burn (direct or revenue minus expenses). Input JSON {cash_balance, monthly_burn} or {cash_balance, monthly_revenue, monthly_expenses}.",
        when_to_use: "the user asks how many months of cash/runway they have left or when they'll run out of money",
        source: include_str!("skills/runway_burn.py") },
    SeedSkill { id: "cap_table", category: "finance", net: false,
        description: "Models a financing round: price per share, new shares issued, post-money valuation, and per-shareholder dilution. Input JSON {existing_shareholders: [{name, shares}], new_investment, pre_money_valuation}.",
        when_to_use: "the user asks how a new funding round dilutes existing shareholders or wants a post-round cap table",
        source: include_str!("skills/cap_table.py") },
    SeedSkill { id: "valuation_vc", category: "finance", net: false,
        description: "VC Method startup valuation: derives post-money/pre-money valuation and investor ownership from target exit value and required return multiple. Input JSON {target_exit_value, required_return_multiple, investment_amount}.",
        when_to_use: "the user asks to value a startup or check if a deal is fundable using the VC method",
        source: include_str!("skills/valuation_vc.py") },
    SeedSkill { id: "break_even", category: "finance", net: false,
        description: "Break-even units and revenue from fixed costs, price, and variable cost per unit. Input JSON {fixed_costs, price_per_unit, variable_cost_per_unit}.",
        when_to_use: "the user asks how many units or how much revenue they need to break even",
        source: include_str!("skills/break_even.py") },
    SeedSkill { id: "tam_sam_som", category: "strategy", net: false,
        description: "TAM/SAM/SOM market sizing from a direct TAM or population x spend, plus a plain-English narrative. Input JSON {tam, sam_pct_of_tam, som_pct_of_sam} or {total_population, avg_annual_spend, sam_pct_of_tam, som_pct_of_sam}.",
        when_to_use: "the user asks to size their total, serviceable, or obtainable market (TAM/SAM/SOM)",
        source: include_str!("skills/tam_sam_som.py") },
    SeedSkill { id: "kpi_calc", category: "data", net: false,
        description: "Computes churn rate, MRR growth, NPS (from counts or raw scores), or gross margin, dispatched by a 'metric' field. Input JSON {metric: churn_rate|mrr_growth|nps|gross_margin, ...metric-specific fields}.",
        when_to_use: "the user asks to calculate churn rate, MRR growth, NPS, or gross margin",
        source: include_str!("skills/kpi_calc.py") },
    SeedSkill { id: "business_model_canvas", category: "strategy", net: false,
        description: "Builds a 9-block Osterwalder Business Model Canvas from optional list[str] fields per block; empty blocks get guiding prompts, plus completeness_pct. Input JSON {key_partners?, key_activities?, key_resources?, value_propositions?, customer_relationships?, channels?, customer_segments?, cost_structure?, revenue_streams?}.",
        when_to_use: "the user wants to map a business model into the 9 standard canvas blocks",
        source: include_str!("skills/business_model_canvas.py") },
    SeedSkill { id: "lean_canvas", category: "strategy", net: false,
        description: "Builds Ash Maurya's 9-block Lean Canvas from optional list[str] fields; empty blocks get startup-specific prompts, plus completeness_pct. Input JSON {problem?, customer_segments?, unique_value_proposition?, solution?, channels?, revenue_streams?, cost_structure?, key_metrics?, unfair_advantage?}.",
        when_to_use: "the user wants to scope a lean/early-stage startup's problem, solution, and channels",
        source: include_str!("skills/lean_canvas.py") },
    SeedSkill { id: "swot_analysis", category: "strategy", net: false,
        description: "Echoes strengths/weaknesses/opportunities/threats and cross-generates a capped TOWS matrix (SO/WO/ST/WT templated strategies) where both sides have items. Input JSON {strengths?, weaknesses?, opportunities?, threats?} (list[str] each).",
        when_to_use: "the user wants to turn a SWOT into actionable cross-strategy scaffolding",
        source: include_str!("skills/swot_analysis.py") },
    SeedSkill { id: "okr_writer", category: "strategy", net: false,
        description: "Flags numeric objectives, scores each key result for measurability/vagueness, warns if key-result count isn't 2-5, returns score_pct. Input JSON {objective, key_results: [str]}.",
        when_to_use: "the user wants to check whether an objective and its key results are well-formed",
        source: include_str!("skills/okr_writer.py") },
    SeedSkill { id: "rice_score", category: "strategy", net: false,
        description: "Ranks backlog items by RICE score (reach*impact*confidence/effort) descending; impact accepts a number or massive/high/medium/low/minimal label. Input JSON {items: [{name, reach, impact, confidence, effort}]}.",
        when_to_use: "the user wants to prioritize a backlog by reach, impact, confidence, and effort",
        source: include_str!("skills/rice_score.py") },
    SeedSkill { id: "ice_score", category: "strategy", net: false,
        description: "Ranks items by ICE score (impact+confidence+ease)/3 descending, each on a 1-10 scale. Input JSON {items: [{name, impact, confidence, ease}]}.",
        when_to_use: "the user wants quick prioritization without a reach/effort dimension",
        source: include_str!("skills/ice_score.py") },
    SeedSkill { id: "five_whys", category: "strategy", net: false,
        description: "Walks a Five Whys root-cause-analysis chain: returns the next 'why' prompt until 5 answers are collected, then flags a root-cause candidate and checks it isn't a person-blame statement. Input JSON {problem, whys?: [str]}.",
        when_to_use: "the user wants to walk through a root-cause-analysis chain step by step",
        source: include_str!("skills/five_whys.py") },
    SeedSkill { id: "pitch_deck_outline", category: "strategy", net: false,
        description: "Returns the fixed 11-slide YC/Sequoia-style pitch deck outline with per-slide guidance and a stage-tailored emphasis note. Input JSON {company_name, one_liner?, stage?}.",
        when_to_use: "the user wants the slide-by-slide structure of a fundraising pitch deck",
        source: include_str!("skills/pitch_deck_outline.py") },
    SeedSkill { id: "north_star_metric", category: "strategy", net: false,
        description: "With no input, returns the 4-criteria North Star Metric framework plus examples by company type; given a candidate, scores it pass/warn/fail per criterion. Input JSON {candidate_metric?, description?}.",
        when_to_use: "the user wants to choose or validate a company's North Star Metric",
        source: include_str!("skills/north_star_metric.py") },
    SeedSkill { id: "pareto_analysis", category: "data", net: false,
        description: "Sorts items descending by value, computes cumulative_pct, and identifies the vital-few prefix reaching >=80% of total value (the 80/20 rule). Input JSON {items: [{name, value}]}.",
        when_to_use: "the user wants to find the 80/20 vital-few causes or contributors in a dataset",
        source: include_str!("skills/pareto_analysis.py") },
    SeedSkill { id: "keyword_density", category: "marketing", net: false,
        description: "Computes count/density/first-position for given keywords in text, or auto-surfaces the top 10 non-stopword words if none given. Input JSON {text, keywords?: [str]}.",
        when_to_use: "the user wants to check SEO keyword usage or placement in drafted content",
        source: include_str!("skills/keyword_density.py") },
    SeedSkill { id: "readability_score", category: "marketing", net: false,
        description: "Computes Flesch Reading Ease and Flesch-Kincaid Grade level via a stdlib syllable heuristic (no APIs). Input JSON {text}.",
        when_to_use: "the user wants to judge if copy is easy enough to read for the target audience",
        source: include_str!("skills/readability_score.py") },
    SeedSkill { id: "meta_tags_audit", category: "marketing", net: true,
        description: "Fetches a page (bounded 500KB) and audits its title, meta description, OG tags, canonical link, and H1 count with length checks. Input JSON {url}.",
        when_to_use: "the user wants to audit a live page's on-page SEO tags before or after publishing",
        source: include_str!("skills/meta_tags_audit.py") },
    SeedSkill { id: "robots_sitemap_check", category: "marketing", net: true,
        description: "Fetches robots.txt, lists Disallow/Allow/Sitemap directives, and parses the first declared sitemap's entry count and type. Input JSON {url}.",
        when_to_use: "the user wants to verify crawlability and sitemap health for a domain",
        source: include_str!("skills/robots_sitemap_check.py") },
    SeedSkill { id: "schema_markup_gen", category: "marketing", net: false,
        description: "Builds valid JSON-LD schema.org markup for Article/Product/FAQPage/LocalBusiness/Organization, flagging missing recommended fields. Input JSON {type, fields}.",
        when_to_use: "the user wants a structured-data snippet to paste into a page's head",
        source: include_str!("skills/schema_markup_gen.py") },
    SeedSkill { id: "utm_builder", category: "marketing", net: false,
        description: "Merges UTM campaign params into a URL via urllib.parse without clobbering existing query params. Input JSON {base_url, source, medium, campaign, term?, content?}.",
        when_to_use: "the user wants to build a trackable campaign link for email, social, or ads",
        source: include_str!("skills/utm_builder.py") },
    SeedSkill { id: "headline_analyzer", category: "marketing", net: false,
        description: "Scores a headline's length, numbers, question/how-to framing, and power-word usage into a 0-100 score with suggestions. Input JSON {headline}.",
        when_to_use: "the user wants to A/B test or polish a blog or ad headline before publishing",
        source: include_str!("skills/headline_analyzer.py") },
    SeedSkill { id: "email_subject_check", category: "marketing", net: false,
        description: "Flags truncation risk, ALL-CAPS, exclamation overuse, spam-trigger phrases, and emoji into a 0-100 deliverability score. Input JSON {subject}.",
        when_to_use: "the user wants to vet an email subject line before sending a campaign",
        source: include_str!("skills/email_subject_check.py") },
    SeedSkill { id: "hashtag_suggest", category: "marketing", net: false,
        description: "Deterministic hashtag ideas from a topic: CamelCase full-topic tag, per-keyword tags, and Tips/101/Ideas template variants, plus static per-platform count guidance. Input JSON {topic, platform?}.",
        when_to_use: "the user wants hashtag suggestions for a social post or topic",
        source: include_str!("skills/hashtag_suggest.py") },
    SeedSkill { id: "social_char_limits", category: "marketing", net: false,
        description: "Static reference table of social-platform character limits (X, Instagram, LinkedIn, Facebook, YouTube, TikTok, Threads, Pinterest); reports remaining/fits for given text. Input JSON {platform?, text?}.",
        when_to_use: "the user asks if a caption or post fits a platform's character limit",
        source: include_str!("skills/social_char_limits.py") },
    SeedSkill { id: "content_brief", category: "marketing", net: false,
        description: "Templated content-writing brief: title options, outline, word-count target by goal, meta-description template, and FAQ prompts. Input JSON {topic, target_keyword?, audience?, goal?}.",
        when_to_use: "the user wants an outline or brief before writing a blog post or article",
        source: include_str!("skills/content_brief.py") },
    SeedSkill { id: "growth_funnel", category: "marketing", net: false,
        description: "AARRR funnel conversion analysis: stage-to-stage and overall conversion rates plus the biggest-leverage drop-off stage. Input JSON {acquisition, activation, retention, referral, revenue} (raw counts).",
        when_to_use: "the user shares funnel-stage counts and wants conversion rates or where to focus growth efforts",
        source: include_str!("skills/growth_funnel.py") },
    SeedSkill { id: "cohort_retention", category: "marketing", net: false,
        description: "Cohort retention/churn curve analysis: per-period retention %, churn %, month-over-month change, and a heuristic 'is the curve flattening' signal. Input JSON {cohort_counts: [int,...]}.",
        when_to_use: "the user has monthly active-user counts for a cohort and wants a retention or churn breakdown",
        source: include_str!("skills/cohort_retention.py") },
    SeedSkill { id: "ab_significance", category: "marketing", net: false,
        description: "Two-proportion z-test for A/B tests (math.erf-based normal CDF): z-score, p-value, significance, and relative uplift. Input JSON {control_conversions, control_total, variant_conversions, variant_total, confidence?}.",
        when_to_use: "the user has A/B test conversion counts and wants to know if the result is statistically significant",
        source: include_str!("skills/ab_significance.py") },
    SeedSkill { id: "sample_size_calc", category: "marketing", net: false,
        description: "A/B test sample-size-per-variant calculator using lookup-table z-scores for power/significance. Input JSON {baseline_rate_pct, minimum_detectable_effect_pct, power?, significance?}.",
        when_to_use: "the user is planning an A/B test and wants to know how many samples they need first",
        source: include_str!("skills/sample_size_calc.py") },
    SeedSkill { id: "password_strength", category: "security", net: false,
        description: "Entropy-based password strength meter that never echoes the raw password back; flags common-password/sequential/repeated patterns, labels very weak to very strong. Input JSON {password}.",
        when_to_use: "the user wants to know if a password is strong enough, reused, or common",
        source: include_str!("skills/password_strength.py") },
    SeedSkill { id: "jwt_decode", category: "security", net: false,
        description: "Decodes a JWT's header/payload and expiry claims — signature is NOT verified, cannot prove authenticity. Input JSON {token}.",
        when_to_use: "the user wants to inspect a JWT's claims or check if it's expired",
        source: include_str!("skills/jwt_decode.py") },
    SeedSkill { id: "subnet_calc", category: "security", net: false,
        description: "CIDR/subnet calculator (stdlib ipaddress): network/broadcast/netmask/usable host range for IPv4 or IPv6. Input JSON {cidr} or {ip, prefix}.",
        when_to_use: "the user needs subnet math, host counts, or usable IP ranges for a CIDR block",
        source: include_str!("skills/subnet_calc.py") },
    SeedSkill { id: "security_headers", category: "security", net: true,
        description: "Fetches a URL and grades (A-D) presence of 6 standard security response headers (HSTS, CSP, X-Frame-Options, etc.). Input JSON {url}.",
        when_to_use: "the user wants to audit a website's HTTP security header hygiene",
        source: include_str!("skills/security_headers.py") },
    SeedSkill { id: "secrets_scan", category: "security", net: false,
        description: "Regex-scans text for leaked AWS/GitHub/Slack/Stripe/Google keys and PEM private keys; matches are always masked, never echoed in full. Input JSON {text}.",
        when_to_use: "the user wants to check a config file, log, or pasted text for accidentally committed secrets",
        source: include_str!("skills/secrets_scan.py") },
    SeedSkill { id: "cve_lookup", category: "security", net: true,
        description: "Queries the keyless public NVD API for CVE details (description, CVSS score/severity, link) by id or keyword. Input JSON {cve_id?} or {keyword?}.",
        when_to_use: "the user asks about a specific CVE or wants to search for known vulnerabilities by keyword",
        source: include_str!("skills/cve_lookup.py") },
    SeedSkill { id: "ssl_cert_check", category: "security", net: true,
        description: "Opens a real TLS connection and reports a certificate's issuer/subject/validity window/days-until-expiry, catching verification failures cleanly. Input JSON {host, port?}.",
        when_to_use: "the user wants to check when a site's TLS certificate expires or whether it's valid",
        source: include_str!("skills/ssl_cert_check.py") },
    SeedSkill { id: "semver", category: "software", net: false,
        description: "Parses, bumps, or compares a semantic version per semver.org 2.0.0 precedence rules. Input JSON {version, op?: parse|bump|compare, bump?, other?}.",
        when_to_use: "the user needs to validate, bump, or compare semantic version strings",
        source: include_str!("skills/semver.py") },
    SeedSkill { id: "cron_explain", category: "software", net: false,
        description: "Explains a 5-field cron expression in plain English and computes the next N fire times. Input JSON {expr, count?}.",
        when_to_use: "the user pastes a cron expression and wants to know what it means or when it'll next run",
        source: include_str!("skills/cron_explain.py") },
    SeedSkill { id: "commit_lint", category: "software", net: false,
        description: "Validates a commit message header against Conventional Commits, returning errors/warnings and parsed type/scope/subject. Input JSON {message}.",
        when_to_use: "the user wants to lint or check a commit message before committing",
        source: include_str!("skills/commit_lint.py") },
    SeedSkill { id: "gitignore_gen", category: "software", net: false,
        description: "Generates a .gitignore for one or more stacks (node/python/rust/go/java/macos/windows/vscode/jetbrains/docker/terraform). Input JSON {stacks: [str]}.",
        when_to_use: "the user is starting a project and needs a .gitignore for their tech stack",
        source: include_str!("skills/gitignore_gen.py") },
    SeedSkill { id: "license_gen", category: "software", net: false,
        description: "Generates full OSS license text (or Apache-2.0's NOTICE header) with author/year filled in. Input JSON {license: MIT|Apache-2.0|BSD-3-Clause|ISC|Unlicense, author, year?}.",
        when_to_use: "the user wants to add a LICENSE file or source header to a project",
        source: include_str!("skills/license_gen.py") },
    SeedSkill { id: "http_status", category: "software", net: false,
        description: "Looks up an HTTP status code's reason phrase, category, and meaning. Input JSON {code}.",
        when_to_use: "the user asks what an HTTP status code means",
        source: include_str!("skills/http_status.py") },
    SeedSkill { id: "useragent_parse", category: "software", net: false,
        description: "Best-effort heuristic parsing of a User-Agent string into browser/OS/device type/bot flag — not authoritative. Input JSON {ua}.",
        when_to_use: "the user has a raw User-Agent string and wants to know the browser, OS, or device it represents",
        source: include_str!("skills/useragent_parse.py") },
    SeedSkill { id: "linear_forecast", category: "data", net: false,
        description: "Fits an OLS linear regression (manual stdlib formulas) to a series and forecasts future points with slope/intercept/R-squared. Input JSON {series: [...]} or {x: [...], y: [...]}, periods?.",
        when_to_use: "the user has a numeric time series and wants a simple trend line and forward forecast",
        source: include_str!("skills/linear_forecast.py") },
    SeedSkill { id: "gpio_pinout", category: "reference", net: false,
        description: "Returns the well-known static pin layout (number/name/function) for a dev board. Input JSON {board: raspberry_pi_40pin|arduino_uno|arduino_nano|esp32_devkit}.",
        when_to_use: "the user is wiring a Raspberry Pi, Arduino, or ESP32 project and needs the pinout reference",
        source: include_str!("skills/gpio_pinout.py") },
    SeedSkill { id: "citation_format", category: "reference", net: false,
        description: "Formats a basic single-author citation in APA/MLA/Chicago style for a book, article, or website. Input JSON {style: APA|MLA|Chicago, type: book|article|website, fields}.",
        when_to_use: "the user needs a quick, correctly-punctuated citation for a source",
        source: include_str!("skills/citation_format.py") },
    // --- batch 5: dev/API tooling, growth/ecommerce, defensive security, a GitHub write action ---
    SeedSkill { id: "openapi_lint", category: "software", net: false,
        description: "Validates a JSON OpenAPI/Swagger spec's required top-level keys and per-endpoint responses, warning on missing operationId/description. Input JSON {spec: str|dict} (JSON only, no YAML).",
        when_to_use: "the user wants to validate an OpenAPI/Swagger spec before publishing or code-gen",
        source: include_str!("skills/openapi_lint.py") },
    SeedSkill { id: "curl_builder", category: "software", net: false,
        description: "Builds a shell-escaped curl command from a method/url/headers/body/query, auto-adding Content-Type for a JSON body. Input JSON {method?, url, headers?, body?, query?}.",
        when_to_use: "the user wants to turn an API call description into a ready-to-paste curl command",
        source: include_str!("skills/curl_builder.py") },
    SeedSkill { id: "adr_template", category: "software", net: false,
        description: "Fills a Michael Nygard-style Markdown Architecture Decision Record, with bracketed prompts for any omitted section. Input JSON {title, status?, context?, decision?, consequences?}.",
        when_to_use: "the user wants to draft a new Architecture Decision Record",
        source: include_str!("skills/adr_template.py") },
    SeedSkill { id: "changelog_gen", category: "software", net: false,
        description: "Groups Conventional Commit messages into Keep-a-Changelog sections (Added/Fixed/Changed/...), isolating breaking changes and dropping internal commit types. Input JSON {commits: [str]}.",
        when_to_use: "the user wants to generate a release changelog from a list of commit messages",
        source: include_str!("skills/changelog_gen.py") },
    SeedSkill { id: "code_complexity", category: "software", net: false,
        description: "Computes per-function McCabe cyclomatic complexity for Python source via an AST walk, flagging functions over 10 as refactor candidates. Input JSON {code: str}.",
        when_to_use: "the user wants to assess refactor priority or complexity hotspots in a Python file or snippet",
        source: include_str!("skills/code_complexity.py") },
    SeedSkill { id: "dockerfile_lint", category: "software", net: false,
        description: "Heuristic Dockerfile review: unpinned FROM tags, missing USER, apt-get bloat, ADD misuse, missing HEALTHCHECK, hardcoded secrets in ENV/ARG. Input JSON {dockerfile: str}.",
        when_to_use: "the user wants to review a Dockerfile for common best-practice and security issues",
        source: include_str!("skills/dockerfile_lint.py") },
    SeedSkill { id: "git_branch_lint", category: "software", net: false,
        description: "Validates a branch name against a <prefix>/<slug> convention, recognizes reserved names, and always returns a best-effort suggested fix. Input JSON {branch: str}.",
        when_to_use: "the user wants to enforce or check a branch-naming convention",
        source: include_str!("skills/git_branch_lint.py") },
    SeedSkill { id: "graphql_query_validate", category: "software", net: false,
        description: "Lightweight (non-grammar) sanity check on a GraphQL query: brace balance, operation type, duplicate operation names, rough field count. Input JSON {query: str}.",
        when_to_use: "the user wants a quick sanity check on a hand-written GraphQL query before sending it",
        source: include_str!("skills/graphql_query_validate.py") },
    SeedSkill { id: "chmod_calc", category: "software", net: false,
        description: "Converts between Unix permission representations — symbolic (rwxr-xr-x), octal (755), and per-owner/group/other booleans. Input JSON {mode: str|int}.",
        when_to_use: "the user wants to translate between chmod octal and symbolic notation",
        source: include_str!("skills/chmod_calc.py") },
    SeedSkill { id: "exit_code_lookup", category: "software", net: false,
        description: "Looks up a shell/POSIX exit code's standard meaning, including signal-number decoding for 129-255. Input JSON {code: int}.",
        when_to_use: "the user wants to decode an unfamiliar process or shell exit code",
        source: include_str!("skills/exit_code_lookup.py") },
    SeedSkill { id: "keyword_suggest", category: "marketing", net: true,
        description: "Free keyword/autocomplete ideation via Google's public (keyless) suggest endpoint. Input JSON {query, limit?}.",
        when_to_use: "the user wants quick keyword or search-query ideas for a seed term",
        source: include_str!("skills/keyword_suggest.py") },
    SeedSkill { id: "ad_copy_check", category: "marketing", net: false,
        description: "Checks ad headline/description length against Google Search, Meta, or LinkedIn ad-platform character limits. Input JSON {platform, headline?, description?}.",
        when_to_use: "the user wants to validate ad copy length before submitting it to an ad platform",
        source: include_str!("skills/ad_copy_check.py") },
    SeedSkill { id: "drip_campaign_planner", category: "marketing", net: false,
        description: "Templated multi-email lifecycle sequence (onboarding/nurture/win-back/abandoned-cart) with send delays, purpose, and subject-line prompts per email. Input JSON {campaign_type, num_emails?}.",
        when_to_use: "the user wants to scaffold a new lifecycle or drip email sequence",
        source: include_str!("skills/drip_campaign_planner.py") },
    SeedSkill { id: "youtube_metadata_optimize", category: "marketing", net: false,
        description: "Grades a YouTube title/description/tags for length, keyword placement, and tag-budget issues into a 0-100 score with suggestions. Input JSON {title, description?, tags?, target_keyword?}.",
        when_to_use: "the user wants a pre-publish check on YouTube video metadata",
        source: include_str!("skills/youtube_metadata_optimize.py") },
    SeedSkill { id: "ecommerce_margin_calc", category: "finance", net: false,
        description: "Computes net profit, net margin, and break-even price from selling price, COGS, marketplace/processing fees, and shipping. Input JSON {selling_price, cogs, marketplace_fee_pct?, payment_processing_fee_pct?, shipping_cost?, other_fixed_costs?}.",
        when_to_use: "the user is pricing an e-commerce SKU or checking margin health",
        source: include_str!("skills/ecommerce_margin_calc.py") },
    SeedSkill { id: "pricing_psychology", category: "marketing", net: false,
        description: "Generates charm-pricing (.99/.95/.97) and round-number prestige-pricing variants from a price, each with a one-line rationale. Input JSON {price}.",
        when_to_use: "the user is choosing between charm and prestige price points for a product",
        source: include_str!("skills/pricing_psychology.py") },
    SeedSkill { id: "core_web_vitals_grade", category: "software", net: false,
        description: "Grades LCP/INP/CLS (and supplementary FCP/TTFB) against Google's published thresholds and reports overall Core Web Vitals pass/fail. Input JSON {lcp_ms?, inp_ms?, cls?, fcp_ms?, ttfb_ms?}.",
        when_to_use: "the user wants to interpret real-user or lab Core Web Vitals measurements",
        source: include_str!("skills/core_web_vitals_grade.py") },
    SeedSkill { id: "landing_page_checklist", category: "marketing", net: false,
        description: "Scores a landing page against a canonical 14-item conversion checklist and surfaces up to 3 prioritized fixes for what's missing. Input JSON {present_elements: [str]}.",
        when_to_use: "the user wants to audit a landing page's conversion-critical elements",
        source: include_str!("skills/landing_page_checklist.py") },
    SeedSkill { id: "hmac_sign", category: "security", net: false,
        description: "Signs or verifies a message with HMAC (sha256/sha1/sha512/md5), constant-time compare, never echoes the secret. Input JSON {message, secret, algorithm?, action?: sign|verify, signature?}.",
        when_to_use: "the user wants to compute or check a webhook/API HMAC signature",
        source: include_str!("skills/hmac_sign.py") },
    SeedSkill { id: "firewall_rule_reference", category: "reference", net: false,
        description: "Static cheatsheet of common hardening rules for ufw, iptables, AWS security groups, or nginx. Input JSON {tool?: ufw|iptables|aws_security_group|nginx}.",
        when_to_use: "the user wants standard firewall or security-group rule syntax while hardening a server",
        source: include_str!("skills/firewall_rule_reference.py") },
    SeedSkill { id: "github_issue_create", category: "software", net: true,
        description: "Creates a real GitHub issue via the REST API — a mutating write, same risk class as the email skill's send action. Needs GITHUB_TOKEN/GH_TOKEN. Input JSON {repo:'owner/name', title, body?, labels?}.",
        when_to_use: "the user explicitly asks to file a bug or task directly into a GitHub repo's issue tracker",
        source: include_str!("skills/github_issue_create.py") },
    SeedSkill { id: "edge_case_gen", category: "data", net: false,
        description: "Generates boundary/edge-case test values with rationale for int/float/string/email/date fields, min/max-aware. Input JSON {type, min?, max?}.",
        when_to_use: "the user is drafting test cases or validation-test inputs for a given field type",
        source: include_str!("skills/edge_case_gen.py") },
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
            assert_eq!(
                m.version, 1,
                "{}: idempotent install must not create a v2",
                s.id
            );
            assert_eq!(m.runtime, Runtime::Process, "{}", s.id);
            assert_eq!(m.interpreter.as_deref(), Some("python3"), "{}", s.id);
            assert_eq!(
                m.capabilities.contains(&Capability::Net),
                s.net,
                "{}: Net capability must match the table",
                s.id
            );
            assert!(
                !bytes.is_empty(),
                "{}: embedded source must not be empty",
                s.id
            );
        }
        // Spot-check: flight_search carries the real script and needs trust (Net + Process).
        let (signed, bytes) = reg.load_active("flight_search").unwrap();
        assert!(signed.manifest.requires_trust());
        assert!(bytes
            .windows(b"prices_for_dates".len())
            .any(|w| w == b"prices_for_dates"));
    }

    /// Every seed script must actually be valid Python — a syntax error would otherwise only
    /// surface at first real invocation. Skips (rather than fails) if python3 isn't on PATH.
    #[test]
    fn seed_skills_are_valid_python() {
        let dir = tempfile::tempdir().unwrap();
        let probe = std::process::Command::new("python3")
            .arg("--version")
            .output();
        if probe.is_err() {
            eprintln!("skipping seed_skills_are_valid_python: python3 not found on PATH");
            return;
        }
        let mut failures = Vec::new();
        for s in SEED_SKILLS {
            let path = dir.path().join(format!("{}.py", s.id));
            std::fs::write(&path, s.source).unwrap();
            let out = std::process::Command::new("python3")
                .arg("-m")
                .arg("py_compile")
                .arg(&path)
                .output()
                .expect("failed to spawn python3");
            if !out.status.success() {
                failures.push(format!(
                    "{}: {}",
                    s.id,
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "invalid seed script(s):\n{}",
            failures.join("\n")
        );
    }

    /// The WASM defaults and the Process-skill library share one registry and must not collide.
    #[test]
    fn seed_skills_coexist_with_wasm_defaults() {
        let (_d, reg) = registry();
        ensure_seed(&reg).unwrap();
        ensure_seed_skills(&reg).unwrap();
        let ids = reg.skills().unwrap();
        for want in [
            "shout",
            "ask",
            "flight_search",
            "weather",
            "email",
            "wikipedia",
            "currency",
            "calc",
            "datetime",
        ] {
            assert!(ids.iter().any(|s| s == want), "missing seeded skill {want}");
        }
    }
}
