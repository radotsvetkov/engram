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
    /// True during A/B replay: forces network isolation for process skills so scoring a net/egress
    /// skill cannot cause real side effects.
    pub scoring: bool,
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

fn truncate(s: &str, max: usize) -> String {
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
        && !p.scoring;
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
                .taint(p.taint);
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
/// into the ledger. This is the one self-improvement path, shared by the agent tool and the HTTP
/// endpoint so they can never diverge.
#[allow(clippy::too_many_arguments)]
pub async fn improve_skill(
    registry: &Registry,
    id: &str,
    candidate: NewSkill,
    bytes: &[u8],
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
    if runs.is_empty() {
        return Ok(json!({
            "decision": "no_data", "id": id, "active": active,
            "note": "no recorded gold runs to judge against yet — teach it accepted (input, output) examples first"
        }));
    }
    let candidate_version = registry.install(candidate, bytes).map_err(|e| e.to_string())?;
    let incumbent_score = score_skill(registry, id, active, &runs, &metric, p, halt).await;
    let candidate_score = score_skill(registry, id, candidate_version, &runs, &metric, p, halt).await;
    let exact = metric == "exact_match";
    // A fuzzy metric is noisy, so require a real margin and a minimum sample count.
    let margin = if exact { 0.0 } else { 0.05 };
    let promoted =
        candidate_score > incumbent_score + margin && (exact || runs.len() >= 3) && consent;
    if promoted {
        registry
            .set_active(id, candidate_version, actor, "skill.promote")
            .map_err(|e| e.to_string())?;
    }
    let _ = registry.ledger().append(
        "skill.learn",
        actor,
        json!({ "id": id, "promoted": promoted, "from": active, "candidate": candidate_version,
                "incumbent_score": incumbent_score, "candidate_score": candidate_score, "replays": runs.len() }),
    );
    Ok(json!({
        "decision": if promoted { "promoted" } else { "rejected" },
        "id": id, "from": active, "candidate": candidate_version,
        "incumbent_score": incumbent_score, "candidate_score": candidate_score, "replays": runs.len(),
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
    let blocked_by_egress = require_pure && !pure;
    if passes && !blocked_by_egress {
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
    let reason = if blocked_by_egress {
        "skill requests network access — a human must adopt it explicitly"
    } else {
        "did not reproduce its gold examples"
    };
    Ok(json!({
        "decision": "rejected", "id": id, "version": latest,
        "score": score, "replays": runs.len(), "reason": reason
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
            scoring: false,
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
