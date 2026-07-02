//! Executing and improving skills at runtime — the bridge that finally makes `ctx.skills` live.
//!
//! A skill is signed bytes plus a manifest naming its [`Runtime`]. This module runs either kind:
//!
//! - **WASM** skills go through the existing fuel-bounded [`SkillHost`] sandbox.
//! - **Process** skills (the polyglot, LLM-authorable substrate — a small Python/JS/Go/shell
//!   program) are executed through the agent's *existing* shell backend ([`shell_command`]):
//!   local, a network-isolated `docker run`, ssh, or singularity. The sandbox is inherited from
//!   the shell tool's configuration, and the network flag is **derived from the signed capability
//!   set** (no `Net` capability ⇒ `--network none`).
//!
//! Both paths verify the Ed25519 signature against the bytes before running, so nothing unsigned or
//! escalated executes. Process skills additionally:
//!   - require the shell gate (`allow_exec`) — with no backend the skill is **refused, never run
//!     silently on the host**;
//!   - are **refused on a tainted run** (the central dispatch gate only covers *egress* tools, so a
//!     code-executing skill needs this explicit complement — [`Manifest::requires_trust`]);
//!   - execute a *throwaway copy* of the script, never the registry original, so a script cannot
//!     overwrite its own signed bytes and defeat verification on the next load;
//!   - are replayed **network-isolated during A/B scoring** so improving a net/messaging skill can't
//!     re-send messages or hit live endpoints.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use engram_core::Taint;
use engram_gateway::Gateway;
use engram_memory::{Memory, Region};
use engram_skills::{
    artifact_ext, manifest, Capability, NewSkill, Outcome, Registry, RunCtx, Runtime, SignedSkill,
    SkillHost,
};
use serde_json::{json, Value};

/// Everything a skill run needs that isn't the skill itself. Borrowed, so it's cheap to clone for a
/// scoring pass (which only flips `scoring`).
#[derive(Clone)]
pub struct SkillRunParams<'a> {
    /// Shell backend, inherited from the run's policy: `None` = local, `Some(image)` = docker,
    /// `Some("ssh:..")` / `Some("singularity:..")` = those backends.
    pub backend: Option<&'a str>,
    pub workdir: &'a Path,
    pub timeout_secs: u64,
    pub taint: Taint,
    /// The shell gate (`allow_shell`). A process skill is refused when this is false.
    pub allow_exec: bool,
    pub gateway: Arc<Gateway>,
    pub memory: Arc<Memory>,
    pub host: &'a SkillHost,
    /// The calling run's memory rings, so a skill's engram.recall/remember stays scoped to this
    /// project (∪ user-global), never another project's memory.
    pub scope: engram_core::ScopeCtx,
    /// True during A/B replay: forces network isolation for process skills so scoring a net/egress
    /// skill cannot cause real side effects.
    pub scoring: bool,
    /// The code about to be verified/executed originated from an UNTRUSTED-provenance run — e.g. a
    /// skill distilled by `reflect_on_skills` after a run that read injected web/document content.
    /// `p.taint` cannot carry this, because the distillation *reflection* is itself a separate Trusted
    /// model call and the params legitimately run as Trusted; the *provenance of the proposed bytes*
    /// is the thing at risk. When this is set, a Process skill may only run under a network-isolated OS
    /// sandbox (built-in `sandbox` or `docker --network none`); on any non-isolating backend
    /// (local / ssh / singularity) it is REFUSED rather than replay-executed on the host.
    ///
    /// **Fail-closed default: treat as `true`.** Any caller that has NOT established the bytes come
    /// from a trusted source (a signed, human-authored/adopted skill) must leave this `true`.
    pub source_tainted: bool,
}

/// Character-bigram Jaccard similarity in [0,1] — partial credit for a near-correct output, so a
/// fuzzy (non-`exact_match`) skill can measurably improve. Mirrors the daemon's scorer.
fn bigram_similarity(a: &[u8], b: &[u8]) -> f32 {
    if a == b {
        return 1.0;
    }
    let grams = |s: &[u8]| -> std::collections::HashSet<(u8, u8)> {
        s.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ga, gb) = (grams(a), grams(b));
    if ga.is_empty() && gb.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let union = ga.union(&gb).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        ga.intersection(&gb).count() as f32 / union
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Cut on a char boundary, not a raw byte index — skill stderr is arbitrary UTF-8 and slicing
    // mid-codepoint would panic.
    let cut = s
        .char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("{}…", &s[..cut])
}

/// Whether a shell backend actually enforces network isolation on the code it runs. Only the built-in
/// OS sandbox (`sandbox`, which passes `allow_net=false` to Seatbelt/bwrap) and Docker (`--network
/// none`) can hold a process off the network. `local` (`None`), `ssh:` and `singularity:` all inherit
/// the host/remote network — they are NOT isolating, so untrusted-provenance code must never run on
/// them. This is the OS-sandbox test the tainted-provenance gate keys off of.
fn backend_is_network_isolated(backend: Option<&str>) -> bool {
    match backend {
        Some("sandbox") => true,
        // Docker: any image string that is not the ssh:/singularity: pseudo-backend.
        Some(img) if !img.starts_with("ssh:") && !img.starts_with("singularity:") => true,
        _ => false, // None (local), ssh:, singularity:
    }
}

/// Build `(program, args)` for a process skill, deriving the docker network flag from `allow_net`.
/// ssh/singularity/local reuse the shared [`shell_command`]; only docker is special-cased so a
/// `Net`-granted skill can reach the network while a pure one stays `--network none`.
fn skill_shell_command(
    backend: Option<&str>,
    workdir: &Path,
    command: &str,
    allow_net: bool,
) -> (String, Vec<String>) {
    match backend {
        // Built-in OS sandbox (Seatbelt/bwrap) — network is granted only when the skill declared Net
        // (and we're trusted + not scoring, per `allow_net`); otherwise the sandbox denies it.
        Some("sandbox") => crate::tools::sandbox_command(workdir, command, allow_net),
        Some(img)
            if !img.starts_with("ssh:") && !img.starts_with("singularity:") =>
        {
            // docker
            let mount = format!("{}:/work", workdir.display());
            let mut args = vec!["run".to_string(), "--rm".to_string()];
            if !allow_net {
                args.push("--network".to_string());
                args.push("none".to_string());
            }
            args.extend([
                "-v".to_string(),
                mount,
                "-w".to_string(),
                "/work".to_string(),
                img.to_string(),
                "sh".to_string(),
                "-c".to_string(),
                command.to_string(),
            ]);
            ("docker".to_string(), args)
        }
        other => crate::tools::shell_command(other, workdir, command),
    }
}

/// Execute a *process* skill: write a throwaway copy of the verified script + the input into the
/// workdir, run `<interpreter> script < input` under the configured backend, return stdout.
async fn run_process_skill(
    signed: &SignedSkill,
    bytes: &[u8],
    input: &[u8],
    p: &SkillRunParams<'_>,
) -> Result<Outcome, String> {
    let m = &signed.manifest;
    if !p.allow_exec {
        return Err(
            "process skills are disabled: enable the shell tool (set ENGRAM_TOOLS_SHELL=1 or \
             configure a backend) to run code-based skills"
                .into(),
        );
    }
    // Code-execution is refused on any tainted run — not just tainted+sensitive. The central
    // dispatch gate only stops egress tools, so a code-executing skill needs this explicit guard.
    if p.taint.is_untrusted() {
        return Err(
            "skill refused: this run read untrusted content (code-execution guard)".into(),
        );
    }
    // Untrusted-PROVENANCE guard (distinct from run taint above). When the *bytes* being executed came
    // from an untrusted source — a skill distilled from a tainted run, whose model-proposed source can
    // embed injected web/document content — replay-executing them to verify/adopt is exactly the
    // dangerous step. Such code may run ONLY under a network-isolated OS sandbox (built-in `sandbox` or
    // `docker --network none`). On a non-isolating backend (local / ssh / singularity) we FAIL CLOSED
    // and refuse, rather than replay-execute attacker-influenced code on the host. This complements the
    // `scoring` flag, which requests isolation but is only physically enforced on those same backends.
    if p.source_tainted && !backend_is_network_isolated(p.backend) {
        return Err(
            "skill refused: untrusted-provenance code (distilled from a tainted run) can only be \
             verified inside a network-isolated OS sandbox — enable the built-in sandbox or a Docker \
             backend (Settings → Tools); it will NOT be run on the local/ssh/singularity host"
                .into(),
        );
    }
    let interpreter = m.interpreter.as_deref().unwrap_or("python3");
    // Last line of defense against shell-command injection: the interpreter is interpolated into a
    // `sh -c` command, so it must carry no shell metacharacters regardless of how the skill was
    // authored (agent tool, HTTP, or a hand-crafted manifest). Authoring validates this too.
    if interpreter.trim().is_empty()
        || interpreter.len() > 64
        || !interpreter
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '/' | '.' | '_' | '-'))
    {
        return Err(format!("skill '{}' has an unsafe interpreter", m.id));
    }
    let ext = artifact_ext(Runtime::Process, Some(interpreter));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let rel_dir = format!(".engram_skill/{}-{}", m.id, nonce);
    let run_dir = p.workdir.join(&rel_dir);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|e| format!("skill workdir: {e}"))?;
    let script_rel = format!("{rel_dir}/skill.{ext}");
    let input_rel = format!("{rel_dir}/input");
    tokio::fs::write(p.workdir.join(&script_rel), bytes)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::write(p.workdir.join(&input_rel), input)
        .await
        .map_err(|e| e.to_string())?;

    // Network is allowed only when the skill declared the Net capability, the run is trusted, AND we
    // are not in a scoring replay (so improving a net skill can't cause real side effects). NOTE: the
    // capability→network mapping is only ENFORCED under the docker backend (--network none). Under a
    // local/ssh backend the process inherits the host's network regardless — consistent with the
    // user's "local = same trust as the shell tool" choice; true isolation requires the docker backend.
    let allow_net = m.capabilities.contains(&Capability::Net)
        && !p.taint.is_untrusted()
        && !p.scoring
        // Untrusted-provenance code never gets the network, even on an isolating backend that reached
        // this point — verifying a distilled net skill must have no live side effects.
        && !p.source_tainted;
    let command = format!("{interpreter} {script_rel} < {input_rel}");
    let (program, args) = skill_shell_command(p.backend, p.workdir, &command, allow_net);

    let started = Instant::now();
    let fut = tokio::process::Command::new(&program)
        .args(&args)
        // Reap the child if the future is dropped (e.g. on timeout), so a stuck skill process /
        // container isn't orphaned — matching the browser/MCP child-process handling.
        .kill_on_drop(true)
        .current_dir(p.workdir)
        .output();
    let result = tokio::time::timeout(Duration::from_secs(p.timeout_secs.max(1)), fut).await;
    // Best-effort cleanup of the throwaway copy regardless of outcome.
    let _ = tokio::fs::remove_dir_all(&run_dir).await;

    let out = result
        .map_err(|_| "skill timed out".to_string())?
        .map_err(|e| format!("failed to spawn '{program}': {e}"))?;
    let duration_us = started.elapsed().as_micros();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!(
            "skill exited {}: {}",
            out.status.code().unwrap_or(-1),
            truncate(stderr.trim(), 2000)
        ));
    }
    let mut output = out.stdout;
    const MAX: usize = 16 * 1024 * 1024;
    output.truncate(MAX);
    Ok(Outcome {
        output,
        fuel_used: 0,
        duration_us,
        host_calls: 0,
        logs: if stderr.trim().is_empty() {
            vec![]
        } else {
            vec![truncate(stderr.trim(), 2000)]
        },
    })
}

/// Run an already-loaded signed skill on `input`, dispatching on its runtime. Verifies the signature
/// against the bytes first on both paths. Takes `registry` only to read its verifying key (so the
/// `ed25519` key type never needs to be named here).
async fn run_loaded(
    registry: &Registry,
    signed: &SignedSkill,
    bytes: &[u8],
    input: &[u8],
    p: &SkillRunParams<'_>,
) -> Result<Outcome, String> {
    let vk = registry.verifying();
    match signed.manifest.runtime {
        Runtime::Wasm => {
            let ctx = RunCtx::pure()
                .memory(p.memory.clone(), Region::ALL.to_vec())
                .gateway(p.gateway.clone())
                .taint(p.taint)
                .scope(p.scope.clone());
            p.host
                .run_signed_async(signed, bytes, vk, input, ctx)
                .await
                .map_err(|e| e.to_string())
        }
        Runtime::Process => {
            manifest::verify(signed, bytes, vk).map_err(|e| e.to_string())?;
            run_process_skill(signed, bytes, input, p).await
        }
    }
}

/// Run the active version of a skill on `input`.
pub async fn run_active(
    registry: &Registry,
    id: &str,
    input: &[u8],
    p: &SkillRunParams<'_>,
) -> Result<Outcome, String> {
    let (signed, bytes) = registry.load_active(id).map_err(|e| e.to_string())?;
    run_loaded(registry, &signed, &bytes, input, p).await
}

/// Replay a version against its recorded `(input, gold)` runs and return the mean score on `metric`
/// (`exact_match` = all-or-nothing; otherwise bigram partial credit). Bounded to the most recent 50
/// runs and cooperative with the halt switch, so an improve can't become a cost/availability DoS.
pub async fn score_skill(
    registry: &Registry,
    id: &str,
    version: u32,
    runs: &[(Vec<u8>, Vec<u8>)],
    metric: &str,
    p: &SkillRunParams<'_>,
    halt: Option<&AtomicBool>,
) -> f32 {
    if runs.is_empty() {
        return 0.0;
    }
    let Ok((signed, bytes)) = registry.load(id, version) else {
        return 0.0;
    };
    let exact = metric == "exact_match";
    const MAX_REPLAYS: usize = 50;
    // Scoring always runs network-isolated so replaying a net/egress skill has no real side effects.
    let sp = SkillRunParams {
        scoring: true,
        ..p.clone()
    };
    let mut total = 0.0f32;
    let mut n = 0usize;
    for (input, gold) in runs.iter().rev().take(MAX_REPLAYS) {
        if halt.map(|h| h.load(Ordering::Relaxed)).unwrap_or(false) {
            break;
        }
        if let Ok(o) = run_loaded(registry, &signed, &bytes, input, &sp).await {
            total += if exact {
                if &o.output == gold {
                    1.0
                } else {
                    0.0
                }
            } else {
                bigram_similarity(&o.output, gold)
            };
        }
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        total / n as f32
    }
}

/// Install a candidate version, replay both it and the incumbent against the recorded gold runs, and
/// promote the candidate iff it measurably wins (and consent is granted). Every outcome is signed
/// into the ledger. This is the one self-improvement path, shared by the agent tool, the reflection
/// pass, and the HTTP endpoint so they can never diverge.
///
/// `new_examples` is how a candidate that EXTENDS behavior earns promotion: on exact-match gold a
/// pure extension can only ever TIE the incumbent (both reproduce the old gold), and a tie never
/// promotes. Asserted new (input, output) pairs are replayed against BOTH versions — the candidate
/// must reproduce them ALL, keep the incumbent's score on the old gold, and the incumbent must FAIL
/// at least part of the new behavior (otherwise nothing is being added). Only on promotion do the
/// new examples become recorded gold; a rejected candidate leaves the gold set untouched.
#[allow(clippy::too_many_arguments)]
pub async fn improve_skill(
    registry: &Registry,
    id: &str,
    candidate: NewSkill,
    bytes: &[u8],
    new_examples: &[(String, String)],
    consent: bool,
    actor: &str,
    p: &SkillRunParams<'_>,
    halt: Option<&AtomicBool>,
) -> Result<Value, String> {
    let (active_signed, _) = registry.load_active(id).map_err(|e| e.to_string())?;
    let metric = active_signed.manifest.metric.clone();
    let active = registry
        .active_version(id)
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    // Check for gold BEFORE installing — otherwise a candidate version (artifact + signed manifest +
    // skill.install ledger entry) accumulates on every improve attempt that can't be scored.
    let runs = registry.accepted_runs(id).map_err(|e| e.to_string())?;
    if runs.is_empty() && new_examples.is_empty() {
        return Ok(json!({
            "decision": "no_data", "id": id, "active": active,
            "note": "no recorded gold runs to judge against yet — teach it accepted (input, output) examples first, or pass new examples with the improvement"
        }));
    }
    let fresh: Vec<(Vec<u8>, Vec<u8>)> = new_examples
        .iter()
        .map(|(i, o)| (i.as_bytes().to_vec(), o.as_bytes().to_vec()))
        .collect();
    let candidate_version = registry.install(candidate, bytes).map_err(|e| e.to_string())?;
    let incumbent_score = if runs.is_empty() {
        1.0 // no old gold to regress on
    } else {
        score_skill(registry, id, active, &runs, &metric, p, halt).await
    };
    let candidate_score = if runs.is_empty() {
        1.0
    } else {
        score_skill(registry, id, candidate_version, &runs, &metric, p, halt).await
    };
    let exact = metric == "exact_match";
    // A fuzzy metric is noisy, so require a real margin and a minimum sample count.
    let margin = if exact { 0.0 } else { 0.05 };
    let promoted = if fresh.is_empty() {
        candidate_score > incumbent_score + margin && (exact || runs.len() >= 3) && consent
    } else {
        // Extension path: no regression on the old gold, the candidate reproduces every asserted
        // new example, and the incumbent demonstrably does NOT — a measurable win, not a tie.
        let cand_new = score_skill(registry, id, candidate_version, &fresh, &metric, p, halt).await;
        let incumbent_new = score_skill(registry, id, active, &fresh, &metric, p, halt).await;
        candidate_score >= incumbent_score
            && cand_new >= if exact { 1.0 } else { 0.8 }
            && incumbent_new + margin < cand_new
            && consent
    };
    if promoted {
        registry
            .set_active(id, candidate_version, actor, "skill.promote")
            .map_err(|e| e.to_string())?;
        // The asserted examples were just verified against the now-active version — they are gold.
        for (i, o) in &fresh {
            let _ = registry.record_run(id, candidate_version, i, o, 1.0);
        }
    }
    let _ = registry.ledger().append(
        "skill.learn",
        actor,
        json!({ "id": id, "promoted": promoted, "from": active, "candidate": candidate_version,
                "incumbent_score": incumbent_score, "candidate_score": candidate_score,
                "replays": runs.len(), "new_examples": fresh.len() }),
    );
    Ok(json!({
        "decision": if promoted { "promoted" } else { "rejected" },
        "id": id, "from": active, "candidate": candidate_version,
        "incumbent_score": incumbent_score, "candidate_score": candidate_score,
        "replays": runs.len(), "new_examples": fresh.len(),
    }))
}

/// Earn activation for a *proposed* (inactive) skill: replay its LATEST version against its own
/// recorded gold and flip it active iff it reproduces the gold (and, when `require_pure` is set, only
/// if it asks for no egress capabilities). This is the promotion gate that lets autonomously
/// distilled skills become usable WITHOUT trusting them on faith — and it sidesteps the circular
/// `load_active` precondition (it works on a skill that has no active version yet). Replays run
/// network-isolated. Every adoption is signed into the ledger as `skill.learn`.
///
/// `require_pure` is true for the autonomous path (a net-capable skill needs a human to consent to
/// the egress) and false for an explicit human "Adopt" click. Returns a decision object; never
/// activates a skill that fails to reproduce its gold or has no gold to judge against.
pub async fn verify_and_adopt(
    registry: &Registry,
    id: &str,
    actor: &str,
    require_pure: bool,
    p: &SkillRunParams<'_>,
    halt: Option<&AtomicBool>,
) -> Result<Value, String> {
    let versions = registry.versions(id).map_err(|e| e.to_string())?;
    let Some(&latest) = versions.iter().max() else {
        return Err(format!("no such skill '{id}'"));
    };
    if registry.active_version(id).map_err(|e| e.to_string())? == Some(latest) {
        return Ok(json!({ "decision": "already_active", "id": id, "version": latest }));
    }
    let (signed, _) = registry.load(id, latest).map_err(|e| e.to_string())?;
    let metric = signed.manifest.metric.clone();
    let pure = signed.manifest.capabilities.is_empty();
    // A Process (script) skill can only be VERIFIED by running it, which needs the shell tool. Say so
    // plainly instead of scoring 0 and reporting "didn't reproduce its examples" — that reads as if the
    // skill is wrong, when in fact it never ran.
    if signed.manifest.runtime == Runtime::Process && !p.allow_exec {
        return Ok(json!({
            "decision": "needs_shell", "id": id, "version": latest,
            "note": "enable the shell tool (Settings → Tools; the built-in sandbox or Docker) so this script skill can be run and verified before adopting"
        }));
    }
    // A NETWORK/LLM skill can't be replay-verified: its output depends on the live world, and scoring
    // runs network-isolated. So the gold-replay gate doesn't apply — instead it's a trust decision.
    // Autonomous path: never auto-activate it; leave it staged for a human. Human "Adopt": the click IS
    // the approval, so activate it on trust (clearly marked as not replay-verified).
    if !pure {
        if require_pure {
            return Ok(json!({
                "decision": "needs_approval", "id": id, "version": latest,
                "note": "network skill — can't be auto-verified offline; approve it in the Skills tab to activate"
            }));
        }
        registry
            .set_active(id, latest, actor, "skill.adopt")
            .map_err(|e| e.to_string())?;
        let _ = registry.ledger().append(
            "skill.learn",
            actor,
            json!({ "id": id, "adopted": true, "promoted": true, "candidate": latest,
                    "approved_unverified": true }),
        );
        return Ok(json!({
            "decision": "approved", "id": id, "version": latest,
            "note": "network skill activated on your approval (can't be replay-verified offline)"
        }));
    }
    let runs = registry.accepted_runs(id).map_err(|e| e.to_string())?;
    if runs.is_empty() {
        return Ok(json!({
            "decision": "no_data", "id": id, "version": latest,
            "note": "no gold examples to verify against — left proposed; teach it accepted (input, output) pairs to enable adoption"
        }));
    }
    let score = score_skill(registry, id, latest, &runs, &metric, p, halt).await;
    let exact = metric == "exact_match";
    // Reproduce ALL gold on an exact skill; clear the high-water mark on a fuzzy one.
    let passes = if exact { score >= 1.0 } else { score >= 0.8 };
    if passes {
        registry
            .set_active(id, latest, actor, "skill.adopt")
            .map_err(|e| e.to_string())?;
        let _ = registry.ledger().append(
            "skill.learn",
            actor,
            json!({ "id": id, "adopted": true, "promoted": true, "candidate": latest,
                    "candidate_score": score, "replays": runs.len() }),
        );
        return Ok(json!({
            "decision": "adopted", "id": id, "version": latest,
            "score": score, "replays": runs.len()
        }));
    }
    Ok(json!({
        "decision": "rejected", "id": id, "version": latest,
        "score": score, "replays": runs.len(), "reason": "did not reproduce its gold examples"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::Ledger;
    use engram_gateway::MockProvider;
    use engram_memory::{Memory, TrigramHashEmbedder};
    use engram_skills::SkillSigner;

    struct Fixture {
        _dir: tempfile::TempDir,
        registry: Registry,
        gateway: Arc<Gateway>,
        memory: Arc<Memory>,
        host: SkillHost,
        workdir: std::path::PathBuf,
    }

    fn setup() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Arc::new(
            Memory::open(
                dir.path().join("b.db"),
                Arc::new(TrigramHashEmbedder::default()),
                ledger.clone(),
            )
            .unwrap(),
        );
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("k")).unwrap());
        let registry = Registry::open(dir.path(), signer, ledger.clone()).unwrap();
        let gateway = Arc::new(Gateway::new(Box::new(MockProvider), ledger.clone()));
        let workdir = dir.path().join("work");
        std::fs::create_dir_all(&workdir).unwrap();
        Fixture {
            _dir: dir,
            registry,
            gateway,
            memory,
            host: SkillHost::new(),
            workdir,
        }
    }

    fn upper_skill() -> NewSkill {
        // A shell script that uppercases stdin — hermetic (sh + tr exist everywhere), no Python needed.
        NewSkill {
            id: "upper".into(),
            category: "transform".into(),
            description: "uppercase stdin".into(),
            capabilities: vec![],
            metric: "exact_match".into(),
            runtime: Runtime::Process,
            interpreter: Some("sh".into()),
            when_to_use: None,
        }
    }

    fn params<'a>(f: &'a Fixture, taint: Taint, allow_exec: bool) -> SkillRunParams<'a> {
        SkillRunParams {
            backend: None,
            workdir: &f.workdir,
            timeout_secs: 10,
            taint,
            allow_exec,
            gateway: f.gateway.clone(),
            memory: f.memory.clone(),
            host: &f.host,
            scope: engram_core::ScopeCtx::any(),
            scoring: false,
            source_tainted: false,
        }
    }

    #[tokio::test]
    async fn process_skill_runs_and_returns_stdout() {
        let f = setup();
        f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        let out = run_active(&f.registry, "upper", b"hello", &params(&f, Taint::Trusted, true))
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.output).trim(), "HELLO");
    }

    #[tokio::test]
    async fn process_skill_refused_under_taint() {
        let f = setup();
        f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        let err = run_active(
            &f.registry,
            "upper",
            b"hello",
            &params(&f, Taint::Untrusted, true),
        )
        .await
        .unwrap_err();
        assert!(err.contains("untrusted"), "got: {err}");
    }

    #[tokio::test]
    async fn process_skill_refused_without_exec_gate() {
        let f = setup();
        f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        let err = run_active(
            &f.registry,
            "upper",
            b"hello",
            &params(&f, Taint::Trusted, false),
        )
        .await
        .unwrap_err();
        assert!(err.contains("disabled"), "got: {err}");
    }

    #[tokio::test]
    async fn tampered_process_skill_fails_verification() {
        let f = setup();
        let v = f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        // Overwrite the on-disk artifact (skills/upper/v{v}.sh) with different bytes — the manifest's
        // module_hash no longer matches, so verification must reject it on the next load.
        let path = f
            ._dir
            .path()
            .join("skills/upper")
            .join(format!("v{v}.sh"));
        assert!(path.exists(), "artifact should be stored as v{v}.sh");
        std::fs::write(&path, b"echo HACKED").unwrap();
        let err = run_active(&f.registry, "upper", b"hello", &params(&f, Taint::Trusted, true))
            .await
            .unwrap_err();
        assert!(
            err.to_lowercase().contains("hash") || err.to_lowercase().contains("signature"),
            "tampered skill should fail verification, got: {err}"
        );
    }

    #[test]
    fn truncate_never_splits_a_multibyte_char() {
        // A skill's stderr is arbitrary UTF-8; truncating at a byte index inside a multibyte char
        // would panic. Build a string whose `max` boundary lands mid-codepoint.
        let s = format!("{}é{}", "a".repeat(1999), "z".repeat(50)); // 'é' starts at byte 1999
        let out = truncate(&s, 2000); // 2000 is inside the 2-byte 'é'
        assert!(out.ends_with('…'));
        assert!(out.len() <= s.len());
        // And the trivial cases.
        assert_eq!(truncate("hi", 10), "hi");
        assert!(truncate(&"😀".repeat(10), 5).ends_with('…'));
    }

    #[tokio::test]
    async fn improve_promotes_a_better_program() {
        let f = setup();
        // v1: identity (wrong for an uppercasing task).
        f.registry
            .install(upper_skill(), b"cat")
            .unwrap();
        for (inp, gold) in [("abc", "ABC"), ("xy", "XY"), ("hi", "HI")] {
            f.registry
                .record_run("upper", 1, inp.as_bytes(), gold.as_bytes(), 1.0)
                .unwrap();
        }
        // Offer v2: the correct uppercasing program. It must replay-win and be promoted.
        let decision = improve_skill(
            &f.registry,
            "upper",
            upper_skill(),
            b"tr a-z A-Z",
            &[],
            true,
            "test",
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(decision["decision"], "promoted", "decision was {decision}");
        assert_eq!(f.registry.active_version("upper").unwrap(), Some(2));
    }

    #[tokio::test]
    async fn improve_with_new_examples_promotes_an_extension_and_rejects_a_tie() {
        let f = setup();
        // v1 uppercases correctly — on old gold alone, any correct candidate can only TIE.
        f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        for (inp, gold) in [("abc", "ABC"), ("xy", "XY")] {
            f.registry
                .record_run("upper", 1, inp.as_bytes(), gold.as_bytes(), 1.0)
                .unwrap();
        }
        // A no-op "improvement" with examples the INCUMBENT already satisfies must be rejected —
        // nothing is being added.
        let tie = improve_skill(
            &f.registry,
            "upper",
            upper_skill(),
            b"tr a-z A-Z",
            &[("zz".to_string(), "ZZ".to_string())],
            true,
            "test",
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(tie["decision"], "rejected", "decision was {tie}");
        // An EXTENSION: also swap spaces for underscores — old gold still passes (no spaces in it),
        // the new example proves behavior the incumbent fails. Must be promoted, and the new
        // example must become recorded gold.
        let golds_before = f.registry.accepted_runs("upper").unwrap().len();
        let ext = improve_skill(
            &f.registry,
            "upper",
            upper_skill(),
            b"tr 'a-z ' 'A-Z_'",
            &[("a b".to_string(), "A_B".to_string())],
            true,
            "test",
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(ext["decision"], "promoted", "decision was {ext}");
        assert!(f.registry.accepted_runs("upper").unwrap().len() > golds_before);
    }

    #[tokio::test]
    async fn verify_and_adopt_activates_a_skill_that_reproduces_its_gold() {
        let f = setup();
        // A proposed (inactive) pure skill that correctly uppercases stdin.
        let v = f
            .registry
            .install_inactive(upper_skill(), b"tr a-z A-Z")
            .unwrap();
        assert_eq!(f.registry.active_version("upper").unwrap(), None);
        for (inp, gold) in [("abc", "ABC"), ("xy", "XY")] {
            f.registry
                .record_run("upper", v, inp.as_bytes(), gold.as_bytes(), 1.0)
                .unwrap();
        }
        let d = verify_and_adopt(
            &f.registry,
            "upper",
            "test",
            true,
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "adopted", "decision was {d}");
        assert_eq!(f.registry.active_version("upper").unwrap(), Some(v));
    }

    #[tokio::test]
    async fn verify_and_adopt_rejects_a_skill_that_fails_its_gold() {
        let f = setup();
        // `cat` echoes the input unchanged — it will NOT reproduce the uppercased gold.
        f.registry
            .install_inactive(upper_skill(), b"cat")
            .unwrap();
        f.registry
            .record_run("upper", 1, b"abc", b"ABC", 1.0)
            .unwrap();
        let d = verify_and_adopt(
            &f.registry,
            "upper",
            "test",
            true,
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "rejected", "decision was {d}");
        assert_eq!(f.registry.active_version("upper").unwrap(), None);
    }

    fn net_skill() -> NewSkill {
        NewSkill {
            id: "netskill".into(),
            category: "research".into(),
            description: "fetches the web".into(),
            capabilities: vec![engram_skills::Capability::Net],
            metric: "exact_match".into(),
            runtime: Runtime::Process,
            interpreter: Some("sh".into()),
            when_to_use: None,
        }
    }

    #[tokio::test]
    async fn verify_and_adopt_stages_net_skill_for_approval_on_auto_path() {
        let f = setup();
        f.registry.install_inactive(net_skill(), b"echo hi").unwrap();
        // Autonomous path (require_pure=true) must NEVER auto-activate a network skill.
        let d = verify_and_adopt(
            &f.registry,
            "netskill",
            "distiller",
            true,
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "needs_approval", "decision was {d}");
        assert_eq!(f.registry.active_version("netskill").unwrap(), None);
    }

    #[tokio::test]
    async fn verify_and_adopt_activates_net_skill_on_human_approval() {
        let f = setup();
        f.registry.install_inactive(net_skill(), b"echo hi").unwrap();
        // Human "Adopt" (require_pure=false): the click IS the approval → activate on trust.
        let d = verify_and_adopt(
            &f.registry,
            "netskill",
            "user",
            false,
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "approved", "decision was {d}");
        assert_eq!(f.registry.active_version("netskill").unwrap(), Some(1));
    }

    #[test]
    fn sandbox_command_denies_network_by_default() {
        let (prog, args) =
            crate::tools::sandbox_command(std::path::Path::new("/tmp/x"), "echo hi", false);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(prog, "sandbox-exec");
            assert!(args.iter().any(|a| a.contains("(deny network*)")));
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(prog, "bwrap");
            assert!(args.iter().any(|a| a == "--unshare-net"));
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = (prog, args);
    }

    #[tokio::test]
    async fn verify_and_adopt_reports_needs_shell_when_exec_disabled() {
        let f = setup();
        let v = f
            .registry
            .install_inactive(upper_skill(), b"tr a-z A-Z")
            .unwrap();
        f.registry
            .record_run("upper", v, b"abc", b"ABC", 1.0)
            .unwrap();
        // allow_exec = false → a script skill can't be run, so we must SAY it needs the shell, not
        // silently score 0 and call it "did not reproduce".
        let d = verify_and_adopt(
            &f.registry,
            "upper",
            "test",
            true,
            &params(&f, Taint::Trusted, false),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "needs_shell", "decision was {d}");
        assert_eq!(f.registry.active_version("upper").unwrap(), None);
    }

    #[test]
    fn only_sandbox_and_docker_are_network_isolated() {
        assert!(backend_is_network_isolated(Some("sandbox")));
        assert!(backend_is_network_isolated(Some("alpine"))); // docker image
        assert!(backend_is_network_isolated(Some("ubuntu:24.04")));
        assert!(!backend_is_network_isolated(None)); // local host
        assert!(!backend_is_network_isolated(Some("ssh:deploy@host")));
        assert!(!backend_is_network_isolated(Some("singularity:img.sif")));
    }

    #[tokio::test]
    async fn tainted_provenance_skill_refused_on_local_backend() {
        // A skill distilled from a tainted run (source_tainted=true) must NOT replay-execute on the
        // non-isolating local backend, even with a Trusted run taint and the shell gate open.
        let f = setup();
        f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
        let mut p = params(&f, Taint::Trusted, true);
        assert_eq!(p.backend, None, "this test needs the local backend");
        p.source_tainted = true;
        let err = run_active(&f.registry, "upper", b"hello", &p)
            .await
            .unwrap_err();
        assert!(
            err.contains("untrusted-provenance") || err.contains("network-isolated"),
            "tainted-provenance code must be refused on the local host, got: {err}"
        );
    }

    #[tokio::test]
    async fn tainted_provenance_skill_runs_under_builtin_sandbox() {
        // On macOS/Linux the built-in `sandbox` backend IS network-isolated, so verifying an
        // untrusted-provenance skill is allowed there (it just can't reach the network).
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let f = setup();
            f.registry.install(upper_skill(), b"tr a-z A-Z").unwrap();
            let mut p = params(&f, Taint::Trusted, true);
            p.backend = Some("sandbox");
            p.source_tainted = true;
            let out = run_active(&f.registry, "upper", b"hello", &p).await.unwrap();
            assert_eq!(String::from_utf8_lossy(&out.output).trim(), "HELLO");
        }
    }

    #[tokio::test]
    async fn verify_and_adopt_left_proposed_without_gold() {
        let f = setup();
        f.registry
            .install_inactive(upper_skill(), b"tr a-z A-Z")
            .unwrap();
        // No recorded gold → cannot verify → must not activate.
        let d = verify_and_adopt(
            &f.registry,
            "upper",
            "test",
            true,
            &params(&f, Taint::Trusted, true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(d["decision"], "no_data", "decision was {d}");
        assert_eq!(f.registry.active_version("upper").unwrap(), None);
    }
}
