//! Swarms - skills composing to solve a bigger problem than any one of them.
//!
//! The simplest, most useful composition is a **pipeline**: the input flows through a
//! sequence of skills, each one's output becoming the next one's input. Because every
//! step runs through the same signed, capability-sandboxed, fuel-bounded host, a swarm
//! is exactly as safe as a single skill - and the whole run is recorded in the ledger
//! with a per-step trace. Richer topologies (fan-out, voting) build on this same loop.

use std::sync::Arc;

use engram_gateway::Gateway;
use engram_memory::{Memory, Region};
use serde::Serialize;
use serde_json::json;

use crate::host::{RunCtx, SkillHost};
use crate::registry::{Registry, RegistryError};

#[derive(Debug, Clone, Serialize)]
pub struct StepTrace {
    pub skill: String,
    pub version: u32,
    pub fuel_used: u64,
    pub host_calls: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineOutcome {
    pub output: Vec<u8>,
    pub steps: Vec<StepTrace>,
}

/// Run `steps` as a pipeline: input → step[0] → step[1] → … → output. Each step is the
/// active signed version of that skill, run with memory/gateway access when provided.
pub async fn run_pipeline(
    host: &SkillHost,
    registry: &Registry,
    steps: &[String],
    input: &[u8],
    memory: Option<Arc<Memory>>,
    gateway: Option<Arc<Gateway>>,
) -> Result<PipelineOutcome, RegistryError> {
    if steps.is_empty() {
        return Err(RegistryError::NotFound("empty pipeline".into()));
    }
    let vk = *registry.verifying();
    let mut data = input.to_vec();
    let mut trace = Vec::with_capacity(steps.len());

    for id in steps {
        let version = registry
            .active_version(id)?
            .ok_or_else(|| RegistryError::NotFound(format!("{id} (no active version)")))?;
        let (signed, wasm) = registry.load(id, version)?;
        let mut ctx = RunCtx::pure();
        if let Some(m) = &memory {
            ctx = ctx.memory(m.clone(), Region::ALL.to_vec());
        }
        if let Some(g) = &gateway {
            ctx = ctx.gateway(g.clone());
        }
        let out = host
            .run_signed_async(&signed, &wasm, &vk, &data, ctx)
            .await?;
        trace.push(StepTrace {
            skill: id.clone(),
            version,
            fuel_used: out.fuel_used,
            host_calls: out.host_calls,
        });
        data = out.output;
    }

    registry.ledger().append(
        "swarm.run",
        "swarm",
        json!({ "steps": steps, "stages": trace.len() }),
    )?;
    Ok(PipelineOutcome {
        output: data,
        steps: trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SkillSigner;
    use crate::registry::NewSkill;
    use engram_core::Ledger;

    const ALLOC: &str = r#"
        (global $heap (mut i32) (i32.const 16384))
        (func (export "alloc") (param $n i32) (result i32)
            (local $p i32) (local.set $p (global.get $heap))
            (global.set $heap (i32.add (global.get $heap) (local.get $n))) (local.get $p))"#;

    fn upcase() -> Vec<u8> {
        let wat = format!(
            r#"(module (memory (export "memory") 1) {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (local $i i32) (local $b i32)
                    (block $done (loop $loop
                        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
                        (local.set $b (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
                        (if (i32.and (i32.ge_u (local.get $b) (i32.const 97)) (i32.le_u (local.get $b) (i32.const 122)))
                            (then (i32.store8 (i32.add (local.get $ptr) (local.get $i)) (i32.sub (local.get $b) (i32.const 32)))))
                        (local.set $i (i32.add (local.get $i) (i32.const 1))) (br $loop)))
                    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))"#
        );
        wat::parse_str(&wat).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pipeline_threads_output_through_steps() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("k")).unwrap());
        let reg = Registry::open(dir.path(), signer, ledger).unwrap();
        reg.install(
            NewSkill {
                id: "shout".into(),
                category: "transform".into(),
                description: "uppercase".into(),
                capabilities: vec![],
                metric: "exact".into(),
            },
            &upcase(),
        )
        .unwrap();
        let host = SkillHost::new();

        let out = run_pipeline(
            &host,
            &reg,
            &["shout".into(), "shout".into()],
            b"hello",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.output, b"HELLO");
        assert_eq!(out.steps.len(), 2);
        assert!(reg.ledger().verify().unwrap() >= 1);
    }
}
