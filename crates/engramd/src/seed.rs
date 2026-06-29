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

/// The flight-search skill source, embedded at build time and installed as a signed Process skill.
/// This is the structured-API answer to Engram's repeated web failures: consumer metasearch sites
/// (Google Flights / Skyscanner / Ryanair) are JS SPAs behind bot detection, so scraping them fails
/// — a flight DATA API does not. Stdlib-only Python, so it needs no `pip install` in the sandbox.
const FLIGHT_SEARCH_PY: &str = include_str!("flight_search.py");

/// A keyless live-data skill (Open-Meteo) that also serves as the reference `http_api` skill
/// template: stdlib-only Python, JSON-in/JSON-out, fail-soft. Because it needs no API key it works
/// the moment it is seeded — proving the Process-skill → live-API path the flight skill also rides.
const WEATHER_PY: &str = include_str!("weather.py");

/// An email skill (wraps the `himalaya` CLI). Its `Net` capability + Process runtime mean SENDING is
/// refused on a tainted run — a prompt-injection arriving inside an email it just read cannot drive a
/// compose+send. The showcase for "automation you can leave running over a real inbox".
const EMAIL_PY: &str = include_str!("email.py");

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

/// Install the `flight_search` skill if absent. Unlike [`ensure_seed`] this is NOT gated on an empty
/// registry — an existing brain (already carrying `shout`/`ask`) must still pick up this new
/// capability — so it guards on the specific skill id instead. Idempotent.
///
/// The skill holds the `Net` capability, so it is taint-gated and refused on an untrusted run, and
/// (under the docker shell backend) runs `--network none` unless trusted — same posture as any other
/// egress skill. Credentials reach it from the daemon's environment (a Process skill inherits it):
/// `TRAVELPAYOUTS_TOKEN` (free) or `AMADEUS_CLIENT_ID`/`AMADEUS_CLIENT_SECRET`. With neither set the
/// skill still succeeds and returns an actionable "how_to_fix" payload.
pub fn ensure_flight_skill(registry: &Registry) -> Result<(), Box<dyn std::error::Error>> {
    if registry.skills()?.iter().any(|id| id == "flight_search") {
        return Ok(());
    }
    let skill = NewSkill {
        id: "flight_search".into(),
        category: "research".into(),
        description: "Find real flights (cheapest fare + cheapest nonstop) between two airports via a \
                      flight DATA API instead of scraping. Input: a JSON object \
                      {origin,destination,depart,return,adults,currency,direct} with IATA codes and \
                      YYYY-MM-DD (or YYYY-MM) dates. Output: ranked JSON fares with a summary."
            .into(),
        capabilities: vec![Capability::Net],
        metric: "helpfulness".into(),
        runtime: Runtime::Process,
        interpreter: Some("python3".into()),
        when_to_use: Some(
            "the user asks for flights, airfare, plane tickets, or the cheapest/fastest way to fly \
             between two cities — reach for this before any browser/search attempt"
                .into(),
        ),
    };
    let version = registry.install(skill, FLIGHT_SEARCH_PY.as_bytes())?;
    tracing::info!(version, "seeded skill 'flight_search'");
    Ok(())
}

/// Install the keyless `weather` skill if absent. Idempotent, id-guarded (like
/// [`ensure_flight_skill`]) so existing brains pick it up. Works with no configuration — the simplest
/// proof that Engram can pull live structured data from a typed API instead of scraping.
pub fn ensure_weather_skill(registry: &Registry) -> Result<(), Box<dyn std::error::Error>> {
    if registry.skills()?.iter().any(|id| id == "weather") {
        return Ok(());
    }
    let skill = NewSkill {
        id: "weather".into(),
        category: "research".into(),
        description: "Get current conditions and a multi-day forecast for any place via the free, \
                      keyless Open-Meteo API. Input: JSON {location} (a place name) or \
                      {latitude,longitude}, optional {days}. Output: JSON current+daily + summary."
            .into(),
        capabilities: vec![Capability::Net],
        metric: "helpfulness".into(),
        runtime: Runtime::Process,
        interpreter: Some("python3".into()),
        when_to_use: Some(
            "the user asks about weather, temperature, rain, or the forecast for a place"
                .into(),
        ),
    };
    let version = registry.install(skill, WEATHER_PY.as_bytes())?;
    tracing::info!(version, "seeded skill 'weather'");
    Ok(())
}

/// Install the `email` skill if absent. Idempotent, id-guarded. Holds `Net` (sending is egress, so a
/// tainted run can't send), interpreter python3. Needs the `himalaya` CLI + a configured account to
/// do anything live; absent that it returns an actionable how_to_fix.
pub fn ensure_email_skill(registry: &Registry) -> Result<(), Box<dyn std::error::Error>> {
    if registry.skills()?.iter().any(|id| id == "email") {
        return Ok(());
    }
    let skill = NewSkill {
        id: "email".into(),
        category: "communication".into(),
        description: "List, read, and send email via the himalaya CLI. Input: JSON \
                      {action: list|read|send, ...}. Output: JSON. Sending is egress and is refused \
                      on a tainted run (injection-over-email cannot drive a send)."
            .into(),
        capabilities: vec![Capability::Net],
        metric: "helpfulness".into(),
        runtime: Runtime::Process,
        interpreter: Some("python3".into()),
        when_to_use: Some(
            "the user asks to check, read, search, or send email / messages in their mailbox".into(),
        ),
    };
    let version = registry.install(skill, EMAIL_PY.as_bytes())?;
    tracing::info!(version, "seeded skill 'email'");
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
    fn flight_skill_seeds_signed_and_idempotent() {
        let (_d, reg) = registry();
        ensure_flight_skill(&reg).unwrap();
        ensure_flight_skill(&reg).unwrap(); // second call must be a no-op (no v2)
        assert!(reg.skills().unwrap().iter().any(|s| s == "flight_search"));

        let (signed, bytes) = reg.load_active("flight_search").unwrap();
        // The embedded script is signed by the brain key and verifies against its bytes.
        manifest::verify(&signed, &bytes, reg.verifying()).expect("seed must be a valid signed skill");
        let m = &signed.manifest;
        assert_eq!(m.version, 1, "idempotent install must not create a v2");
        assert_eq!(m.runtime, Runtime::Process);
        assert_eq!(m.interpreter.as_deref(), Some("python3"));
        assert!(m.capabilities.contains(&Capability::Net), "needs Net to call the flight API");
        assert!(m.requires_trust(), "a Net+Process skill must be taint-gated");
        // The bytes are the real script, not an empty placeholder.
        assert!(
            bytes.windows(b"prices_for_dates".len()).any(|w| w == b"prices_for_dates"),
            "embedded source should be the flight_search script"
        );
    }

    /// Seeding the WASM defaults must leave the flight skill installable independently — the two
    /// paths share a registry and must not collide.
    #[test]
    fn flight_skill_coexists_with_default_seed() {
        let (_d, reg) = registry();
        ensure_seed(&reg).unwrap();
        ensure_flight_skill(&reg).unwrap();
        ensure_weather_skill(&reg).unwrap();
        ensure_email_skill(&reg).unwrap();
        let ids = reg.skills().unwrap();
        for want in ["shout", "ask", "flight_search", "weather", "email"] {
            assert!(ids.iter().any(|s| s == want), "missing seeded skill {want}");
        }
    }

    #[test]
    fn weather_skill_seeds_signed_and_idempotent() {
        let (_d, reg) = registry();
        ensure_weather_skill(&reg).unwrap();
        ensure_weather_skill(&reg).unwrap();
        let (signed, bytes) = reg.load_active("weather").unwrap();
        manifest::verify(&signed, &bytes, reg.verifying()).expect("seed must be a valid signed skill");
        assert_eq!(signed.manifest.version, 1, "idempotent install must not create a v2");
        assert!(signed.manifest.capabilities.contains(&Capability::Net));
        assert!(bytes.windows(b"open-meteo".len()).any(|w| w == b"open-meteo"));
    }
}
