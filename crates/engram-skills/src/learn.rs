//! The learning loop - how a skill gets better at what it does.
//!
//! When a candidate version of a skill appears, it is not trusted on faith. It is
//! **replayed against the recorded inputs the skill has actually seen**, scored on
//! the skill's own metric (here: how often its output matches the output that was
//! accepted), and compared head-to-head with the version currently in use. The
//! candidate is promoted only if it *measurably wins* - and only with consent. The
//! decision and the scores are written to the audit ledger, and any promotion is one
//! `set_active` away from being reverted.
//!
//! Crucially the candidate is a *program* (new WASM bytes), not just a config tweak:
//! this is the mechanism by which a skill - however its next version is authored,
//! by search now or by an LLM later - improves itself safely.

use serde::Serialize;
use serde_json::json;

use crate::host::{RunCtx, SkillHost};
use crate::registry::{NewSkill, Registry, RegistryError};

/// The outcome of an improvement attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Candidate beat the incumbent and was activated.
    Promoted {
        id: String,
        from: Option<u32>,
        to: u32,
        incumbent_score: f32,
        candidate_score: f32,
        replays: usize,
    },
    /// Candidate did not beat the incumbent (or consent was withheld); it stays
    /// installed but inactive.
    Rejected {
        id: String,
        active: u32,
        candidate: u32,
        incumbent_score: f32,
        candidate_score: f32,
        replays: usize,
    },
    /// No accepted history to judge against yet - candidate installed, not activated.
    NoData { id: String, candidate: u32 },
}

/// Replay a version against `(input, gold)` pairs and return the fraction whose
/// output exactly matches the accepted output.
pub fn score_version(
    host: &SkillHost,
    registry: &Registry,
    id: &str,
    version: u32,
    runs: &[(Vec<u8>, Vec<u8>)],
) -> Result<f32, RegistryError> {
    if runs.is_empty() {
        return Ok(0.0);
    }
    let (signed, wasm) = registry.load(id, version)?;
    let vk = *registry.verifying();
    let mut correct = 0usize;
    for (input, gold) in runs {
        match host.run_signed(&signed, &wasm, &vk, input, RunCtx::pure()) {
            Ok(o) if &o.output == gold => correct += 1,
            _ => {}
        }
    }
    Ok(correct as f32 / runs.len() as f32)
}

/// Install a candidate version and promote it iff it beats the active version on the
/// recorded replay set (and `consent` is granted).
pub fn improve(
    host: &SkillHost,
    registry: &Registry,
    id: &str,
    candidate: NewSkill,
    candidate_wasm: &[u8],
    consent: bool,
) -> Result<Decision, RegistryError> {
    let candidate_version = registry.install(candidate, candidate_wasm)?;
    let runs = registry.accepted_runs(id)?;
    if runs.is_empty() {
        return Ok(Decision::NoData {
            id: id.to_string(),
            candidate: candidate_version,
        });
    }
    let active = registry
        .active_version(id)?
        .ok_or_else(|| RegistryError::NotFound(format!("{id} (no active version)")))?;

    let incumbent_score = score_version(host, registry, id, active, &runs)?;
    let candidate_score = score_version(host, registry, id, candidate_version, &runs)?;
    let replays = runs.len();

    if candidate_score > incumbent_score && consent {
        registry.set_active(id, candidate_version, "learn", "skill.promote")?;
        registry.ledger().append(
            "skill.learn",
            "learn",
            json!({
                "id": id,
                "promoted": true,
                "from": active,
                "to": candidate_version,
                "incumbent_score": incumbent_score,
                "candidate_score": candidate_score,
                "replays": replays,
            }),
        )?;
        Ok(Decision::Promoted {
            id: id.to_string(),
            from: Some(active),
            to: candidate_version,
            incumbent_score,
            candidate_score,
            replays,
        })
    } else {
        registry.ledger().append(
            "skill.learn",
            "learn",
            json!({
                "id": id,
                "promoted": false,
                "active": active,
                "candidate": candidate_version,
                "incumbent_score": incumbent_score,
                "candidate_score": candidate_score,
                "replays": replays,
            }),
        )?;
        Ok(Decision::Rejected {
            id: id.to_string(),
            active,
            candidate: candidate_version,
            incumbent_score,
            candidate_score,
            replays,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use engram_core::Ledger;
    use std::sync::Arc;

    const ALLOC: &str = r#"
        (global $heap (mut i32) (i32.const 16384))
        (func (export "alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $heap))
            (global.set $heap (i32.add (global.get $heap) (local.get $n)))
            (local.get $p))"#;

    // v1: echo (returns input unchanged) - wrong for an uppercasing task.
    fn echo() -> Vec<u8> {
        let wat = format!(
            r#"(module (memory (export "memory") 1) {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
                            (i64.extend_i32_u (local.get $len)))))"#
        );
        wat::parse_str(&wat).unwrap()
    }

    // v2: ASCII uppercase in place - the correct program.
    fn upcase() -> Vec<u8> {
        let wat = format!(
            r#"(module (memory (export "memory") 1) {ALLOC}
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
                            (i64.extend_i32_u (local.get $len)))))"#
        );
        wat::parse_str(&wat).unwrap()
    }

    fn new_skill() -> NewSkill {
        NewSkill {
            id: "upcase".into(),
            category: "transform".into(),
            description: "uppercase the input".into(),
            capabilities: vec![Capability::MemoryRead], // declared but unused here
            metric: "exact_match".into(),
        }
    }

    fn setup() -> (SkillHost, Registry, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let signer = Arc::new(
            crate::manifest::SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap(),
        );
        let reg = Registry::open(dir.path(), signer, ledger).unwrap();
        (SkillHost::new(), reg, dir)
    }

    #[test]
    fn promotes_a_better_program() {
        let (host, reg, _d) = setup();

        // Install v1 (echo) and record the inputs/outputs the user actually accepted.
        reg.install(new_skill(), &echo()).unwrap();
        assert_eq!(reg.active_version("upcase").unwrap(), Some(1));
        for (inp, gold) in [
            (b"abc".as_slice(), b"ABC".as_slice()),
            (b"xy".as_slice(), b"XY".as_slice()),
        ] {
            reg.record_run("upcase", 1, inp, gold, 1.0).unwrap();
        }

        // Offer v2 (upcase). It must replay-win and be promoted.
        let decision = improve(&host, &reg, "upcase", new_skill(), &upcase(), true).unwrap();
        match decision {
            Decision::Promoted {
                to,
                incumbent_score,
                candidate_score,
                replays,
                ..
            } => {
                assert_eq!(to, 2);
                assert_eq!(replays, 2);
                assert_eq!(incumbent_score, 0.0);
                assert_eq!(candidate_score, 1.0);
            }
            other => panic!("expected promotion, got {other:?}"),
        }
        assert_eq!(reg.active_version("upcase").unwrap(), Some(2));
    }

    #[test]
    fn rejects_a_worse_program_and_revert_restores() {
        let (host, reg, _d) = setup();
        // Install v1 (upcase, correct), record an accepted run.
        reg.install(new_skill(), &upcase()).unwrap();
        reg.record_run("upcase", 1, b"abc", b"ABC", 1.0).unwrap();
        // Offer v2 (echo, worse). It must be rejected; v1 stays active.
        let decision = improve(&host, &reg, "upcase", new_skill(), &echo(), true).unwrap();
        assert!(matches!(decision, Decision::Rejected { .. }));
        assert_eq!(reg.active_version("upcase").unwrap(), Some(1));

        // And a manual promotion can be reverted.
        reg.set_active("upcase", 2, "user", "skill.activate")
            .unwrap();
        assert_eq!(reg.active_version("upcase").unwrap(), Some(2));
        reg.set_active("upcase", 1, "user", "skill.revert").unwrap();
        assert_eq!(reg.active_version("upcase").unwrap(), Some(1));
        assert!(reg.ledger().verify().unwrap() > 0);
    }
}
