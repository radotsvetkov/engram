//! Seed skills — the procedural memory a fresh brain is born with.
//!
//! On first boot (when no skills exist yet) the daemon installs one tiny, real WASM
//! skill and records a few accepted runs for it, so the dashboard immediately shows a
//! runnable skill and the learning loop has history to replay a candidate against.

use engram_skills::{Capability, NewSkill, Registry};

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
/// egress capability). Demonstrates a sandboxed skill reaching the LLM — taint-gated,
/// metered, and audited — from the dashboard.
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
    let skill = NewSkill {
        id: "shout".into(),
        category: "transform".into(),
        description: "Uppercase the input text.".into(),
        capabilities: vec![],
        metric: "exact_match".into(),
    };
    let version = registry.install(skill, &wasm)?;
    for (input, gold) in [("hello", "HELLO"), ("engram", "ENGRAM"), ("rust", "RUST")] {
        registry.record_run("shout", version, input.as_bytes(), gold.as_bytes(), 1.0)?;
    }
    tracing::info!(version, "seeded skill 'shout'");

    let ask = wat::parse_str(ASK_WAT)?;
    registry.install(
        NewSkill {
            id: "ask".into(),
            category: "thinking".into(),
            description: "Forward the input to the model through the gateway.".into(),
            capabilities: vec![Capability::Llm],
            metric: "helpfulness".into(),
        },
        &ask,
    )?;
    tracing::info!("seeded skill 'ask'");
    Ok(())
}
