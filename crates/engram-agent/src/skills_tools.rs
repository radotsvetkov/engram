//! The four tools that let an agent grow and reuse its own skills — the "write a small
//! program, keep it, get better at it" loop, finally exposed to the model (until now `ctx.skills`
//! was inert in every run).
//!
//! - `skill_search` — find an existing skill that fits the task (auto-selection).
//! - `skill_run`    — run the active version of a skill on an input (reuse).
//! - `skill_author` — create a NEW skill: a small polyglot program (default Python), signed and
//!   installed, seeded with the example (input → output) pairs the author asserts.
//! - `skill_improve` — offer a better version; it is replay-scored against the recorded gold and
//!   promoted only if it measurably wins. Every step is signed into the ledger.
//!
//! Authoring/improving is refused on a tainted run and gated by `allow_skill_author`; running a
//! process (code) skill additionally requires the shell gate and a trusted run.

use async_trait::async_trait;
use engram_skills::{Capability, NewSkill, Runtime, SkillHost};
use serde_json::{json, Value};

use crate::skills_runtime::{improve_skill, run_active, SkillRunParams};
use crate::tool::{Tool, ToolCtx};
use crate::tools::arg_str;

/// Build the runtime params for a skill run from the tool context + a fresh WASM host.
fn params<'a>(ctx: &'a ToolCtx, host: &'a SkillHost) -> SkillRunParams<'a> {
    SkillRunParams {
        backend: ctx.policy.shell_backend.as_deref(),
        workdir: &ctx.workdir,
        timeout_secs: ctx.policy.timeout_secs,
        taint: ctx.taint,
        allow_exec: ctx.policy.allow_shell,
        gateway: ctx.gateway.clone(),
        memory: ctx.memory.clone(),
        host,
        scope: ctx.scope.clone(),
        scoring: false,
        // Running an already-adopted, signed, active skill: its bytes are trusted provenance (a human
        // or the verify/adopt gate accepted them). This is not the distillation path, so not tainted.
        source_tainted: false,
    }
}

fn parse_capability(s: &str) -> Option<Capability> {
    match s.trim().to_ascii_lowercase().as_str() {
        "memory_read" | "memoryread" | "recall" => Some(Capability::MemoryRead),
        "memory_write" | "memorywrite" | "remember" => Some(Capability::MemoryWrite),
        "llm" | "model" => Some(Capability::Llm),
        "net" | "network" => Some(Capability::Net),
        _ => None,
    }
}

/// A skill id must be a simple slug, so it maps safely to a directory name.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The interpreter is interpolated into a `sh -c "<interpreter> script < input"` command, so it must
/// not carry shell metacharacters. Allow only what real interpreter commands need (e.g. "python3",
/// "go run", "/usr/bin/node") — letters, digits, space, and `/._-` — which blocks `;|&$\`"'`, etc.
fn valid_interpreter(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '/' | '.' | '_' | '-'))
}

// ---------------------------------------------------------------------------
// skill_search
// ---------------------------------------------------------------------------

pub struct SkillSearchTool;

#[async_trait]
impl Tool for SkillSearchTool {
    fn name(&self) -> &str {
        "skill_search"
    }
    fn description(&self) -> &str {
        "Find an existing skill (a small reusable program the agent built earlier) that fits the \
         task, before solving from scratch. Returns each match's id, what it does, how to call it, \
         and how many gold examples back it. If a good match exists, prefer skill_run over redoing \
         the work; if it's close but imperfect, improve it with skill_improve."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "query": { "type": "string", "description": "what you're trying to do" },
                            "k": { "type": "integer", "description": "max results (default 5)" } },
            "required": ["query"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let query = arg_str(args, "query")?.to_ascii_lowercase();
        let k = args["k"].as_u64().unwrap_or(5).clamp(1, 20) as usize;
        let terms: Vec<&str> = query.split_whitespace().filter(|t| t.len() > 2).collect();
        let ids = ctx.skills.skills().map_err(|e| e.to_string())?;
        let mut ranked: Vec<(f32, Value)> = Vec::new();
        for id in ids {
            let Ok((signed, _)) = ctx.skills.load_active(&id) else {
                continue;
            };
            let m = &signed.manifest;
            let gold = ctx.skills.accepted_runs(&id).map(|r| r.len()).unwrap_or(0);
            let hay = format!(
                "{} {} {} {}",
                m.id,
                m.description,
                m.when_to_use.clone().unwrap_or_default(),
                m.category
            )
            .to_ascii_lowercase();
            let overlap = terms.iter().filter(|t| hay.contains(*t)).count() as f32;
            // Lexical fit dominates; a small bonus for skills with more gold examples behind them.
            let score = overlap + (gold as f32).min(5.0) * 0.1;
            if score <= 0.0 {
                continue;
            }
            ranked.push((
                score,
                json!({
                    "id": m.id,
                    "runtime": if m.runtime == Runtime::Process { "process" } else { "wasm" },
                    "interpreter": m.interpreter,
                    "does": m.description,
                    "when_to_use": m.when_to_use,
                    "capabilities": m.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                    "gold_examples": gold,
                    "call": format!("skill_run with id=\"{}\"", m.id),
                }),
            ));
        }
        if ranked.is_empty() {
            return Ok("(no matching skills yet — solve the task, then consider skill_author to keep a reusable program)".into());
        }
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let out: Vec<Value> = ranked.into_iter().take(k).map(|(_, v)| v).collect();
        Ok(serde_json::to_string_pretty(&json!({ "skills": out })).unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// skill_run
// ---------------------------------------------------------------------------

pub struct SkillRunTool;

#[async_trait]
impl Tool for SkillRunTool {
    fn name(&self) -> &str {
        "skill_run"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Run the active version of an installed skill on an input string and return its output. Use \
         skill_search first to find the right id."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "id": { "type": "string" },
                            "input": { "type": "string", "description": "passed to the skill on stdin" } },
            "required": ["id"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let id = arg_str(args, "id")?;
        // Skills read stdin, often as JSON - so models frequently pass `input` as an OBJECT.
        // Dropping it to "" ran the skill on empty stdin with plausible-but-wrong output;
        // serialize instead, so the skill sees exactly what the model meant.
        let input_owned = match &args["input"] {
            Value::Null => String::new(),
            Value::String(t) => t.clone(),
            v => v.to_string(),
        };
        let input = input_owned.as_str();
        let (signed, _) = ctx
            .skills
            .load_active(id)
            .map_err(|_| format!("no such skill '{id}' (use skill_search to list skills)"))?;
        // Code-executing or egress-capable skills are refused on a tainted run (injection guard) —
        // the central dispatch gate only covers egress *tools*, not skills.
        if signed.manifest.requires_trust() && ctx.taint.is_untrusted() {
            return Err(format!(
                "skill '{id}' refused: this run read untrusted content and the skill executes code / has egress"
            ));
        }
        if ctx.policy.dry_run {
            return Ok(format!("[dry-run] would run skill '{id}'"));
        }
        let host = SkillHost::new();
        let p = params(ctx, &host);
        let outcome = match run_active(&ctx.skills, id, input.as_bytes(), &p).await {
            Ok(o) => o,
            Err(e) => {
                // A failed run is EXPERIENCE. Sign it into the ledger so the reflection loop can
                // see which skills keep failing in real use and target THEM for improvement —
                // swallowing failures left the loop only ever looking at its successes.
                let head: String = e.chars().take(200).collect();
                let _ = ctx.ledger.append(
                    "skill.run",
                    "agent",
                    json!({ "id": id, "ok": false, "error": head }),
                );
                return Err(e);
            }
        };
        let _ = ctx.ledger.append(
            "skill.run",
            "agent",
            json!({ "id": id, "ok": true, "bytes_out": outcome.output.len(), "duration_us": outcome.duration_us }),
        );
        // AUTO-LEARN from real use: record this (input, output) from a PURE (no network / no LLM)
        // skill as a PROVISIONAL example (reward 0.5), NOT accepted gold (≥ 0.75). Deterministic is
        // not the same as CORRECT: if the active version has a bug (wrong rounding, off-by-one), its
        // wrong outputs are deterministic too — recording them as gold would make improve_skill score
        // a FIXED candidate LOWER on those inputs and permanently reject the fix, so the loop
        // converges on its own defects. At 0.5 the example stays in history for audit/analysis but is
        // BELOW the accepted-runs replay threshold, so only human/author-asserted gold judges
        // improvements. Deduped by input and capped so it can't grow without bound; network/LLM skills
        // are skipped entirely (their output varies with the live world).
        if signed.manifest.capabilities.is_empty()
            && !outcome.output.is_empty()
            && !input.is_empty()
        {
            if let Ok(accepted) = ctx.skills.accepted_runs(id) {
                // Don't shadow a real gold example for this input, and cap capture volume by the
                // accepted-set size so provisional records can't grow without bound. (A provisional
                // record is below the accepted threshold, so it never enters the replay/gold set.)
                let already = accepted
                    .iter()
                    .any(|(i, _)| i.as_slice() == input.as_bytes());
                if !already && accepted.len() < 30 {
                    if let Ok(Some(v)) = ctx.skills.active_version(id) {
                        let _ =
                            ctx.skills
                                .record_run(id, v, input.as_bytes(), &outcome.output, 0.5);
                    }
                }
            }
        }
        let text = String::from_utf8_lossy(&outcome.output).to_string();
        Ok(if text.trim().is_empty() {
            format!("(skill '{id}' produced no output)")
        } else {
            text
        })
    }
}

// ---------------------------------------------------------------------------
// skill_source
// ---------------------------------------------------------------------------

pub struct SkillSourceTool;

#[async_trait]
impl Tool for SkillSourceTool {
    fn name(&self) -> &str {
        "skill_source"
    }
    fn description(&self) -> &str {
        "Read the CURRENT program of an installed skill (its active version, else the latest) plus \
         its interpreter and a few of its gold examples. Use this BEFORE skill_improve so the \
         improved source starts from what the skill actually does — the registry is the only place \
         the program lives; it is NOT a file in the workspace."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let id = arg_str(args, "id")?;
        let versions = ctx.skills.versions(id).map_err(|e| e.to_string())?;
        let Some(&latest) = versions.iter().max() else {
            return Err(format!(
                "no such skill '{id}' (use skill_search to list skills)"
            ));
        };
        let shown = ctx
            .skills
            .active_version(id)
            .ok()
            .flatten()
            .unwrap_or(latest);
        let (signed, bytes) = ctx.skills.load(id, shown).map_err(|e| e.to_string())?;
        let m = &signed.manifest;
        if m.runtime != Runtime::Process {
            return Ok(format!(
                "skill '{id}' v{shown} is a WASM transform ({} bytes of wasm) — its source isn't \
                 readable here; improve it from the dashboard.",
                bytes.len()
            ));
        }
        let source = String::from_utf8_lossy(&bytes);
        let gold = ctx.skills.accepted_runs(id).unwrap_or_default();
        let examples = gold
            .iter()
            .take(3)
            .map(|(i, o)| {
                format!(
                    "  input: {}\n  output: {}",
                    crate::skills_runtime::truncate(&String::from_utf8_lossy(i), 300),
                    crate::skills_runtime::truncate(&String::from_utf8_lossy(o), 300)
                )
            })
            .collect::<Vec<_>>()
            .join("\n---\n");
        Ok(format!(
            "skill '{id}' v{shown}{} — interpreter: {} — {}\ncapabilities: {:?}\n\n--- source ---\n{}\n\n--- gold examples ({} recorded{}) ---\n{}",
            if Some(shown) == ctx.skills.active_version(id).ok().flatten() { " (active)" } else { " (proposed, not active)" },
            m.interpreter.as_deref().unwrap_or("python3"),
            m.description,
            m.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            crate::skills_runtime::truncate(&source, 12_000),
            gold.len(),
            if gold.len() > 3 { ", first 3 shown" } else { "" },
            examples,
        ))
    }
}

// ---------------------------------------------------------------------------
// skill_author
// ---------------------------------------------------------------------------

pub struct SkillAuthorTool;

#[async_trait]
impl Tool for SkillAuthorTool {
    fn name(&self) -> &str {
        "skill_author"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Create a NEW reusable skill from a small program you write (default Python; also node, \
         bash, go, etc.). The program reads its input on stdin and writes its result to stdout. \
         Provide a few example {input, output} pairs you KNOW are correct — they become the gold \
         set the skill is later improved against. Use this when you've solved something worth \
         keeping; reuse it next time with skill_run. To change an existing skill, use skill_improve."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id": { "type": "string", "description": "short slug, e.g. hn_top_titles" },
            "description": { "type": "string" },
            "when_to_use": { "type": "string", "description": "a cue for when to reach for this skill" },
            "interpreter": { "type": "string", "description": "python3 (default), node, bash, ruby, ..." },
            "source": { "type": "string", "description": "the program; reads stdin, writes stdout" },
            "capabilities": { "type": "array", "items": { "type": "string" },
                              "description": "subset of: net (network access), llm. Omit for a pure transform." },
            "metric": { "type": "string", "description": "exact_match (default) or a fuzzy metric name" },
            "examples": { "type": "array", "items": { "type": "object",
                          "properties": { "input": {"type":"string"}, "output": {"type":"string"} } } }
        }, "required": ["id", "description", "source"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_skill_author {
            return Err("skill authoring is disabled by policy".into());
        }
        if ctx.taint.is_untrusted() {
            return Err("skill authoring refused: this run read untrusted content".into());
        }
        let id = arg_str(args, "id")?.trim().to_string();
        if !valid_id(&id) {
            return Err("invalid skill id (use letters, digits, '_' or '-', max 64)".into());
        }
        if ctx
            .skills
            .active_version(&id)
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err(format!(
                "skill '{id}' already exists — use skill_improve to offer a better version"
            ));
        }
        let description = arg_str(args, "description")?.to_string();
        let source = arg_str(args, "source")?;
        if source.trim().is_empty() {
            return Err("source is empty".into());
        }
        let interpreter = args["interpreter"]
            .as_str()
            .unwrap_or("python3")
            .trim()
            .to_string();
        if !valid_interpreter(&interpreter) {
            return Err("invalid interpreter (letters, digits, space, and /._- only)".into());
        }
        let when_to_use = args["when_to_use"].as_str().map(|s| s.to_string());
        let metric = args["metric"].as_str().unwrap_or("exact_match").to_string();
        // Capabilities are clamped: a script gets only the egress caps it explicitly, validly asks
        // for (Net/Llm). MemoryRead/Write are WASM-host concepts and not granted to scripts here.
        let mut capabilities = Vec::new();
        if let Some(arr) = args["capabilities"].as_array() {
            for c in arr {
                if let Some(cap) = c.as_str().and_then(parse_capability) {
                    if matches!(cap, Capability::Net | Capability::Llm)
                        && !capabilities.contains(&cap)
                    {
                        capabilities.push(cap);
                    }
                }
            }
        }
        if ctx.policy.dry_run {
            return Ok(format!("[dry-run] would author skill '{id}'"));
        }
        let new = NewSkill {
            id: id.clone(),
            category: "problem_solving".into(),
            description,
            capabilities,
            metric,
            runtime: Runtime::Process,
            interpreter: Some(interpreter),
            when_to_use,
        };
        let version = ctx
            .skills
            .install(new, source.as_bytes())
            .map_err(|e| e.to_string())?;
        // Seed the gold replay set with the examples the author asserts are correct (reward 1.0), so
        // a future skill_improve has something to measure against.
        let mut gold = 0usize;
        if let Some(arr) = args["examples"].as_array() {
            for ex in arr {
                if let (Some(inp), Some(out)) = (ex["input"].as_str(), ex["output"].as_str()) {
                    if ctx
                        .skills
                        .record_run(&id, version, inp.as_bytes(), out.as_bytes(), 1.0)
                        .is_ok()
                    {
                        gold += 1;
                    }
                }
            }
        }
        // Cheap sprawl control: surface near-duplicate ids so the model is nudged to improve rather
        // than mint one-offs next time.
        let dupes: Vec<String> = ctx
            .skills
            .skills()
            .unwrap_or_default()
            .into_iter()
            .filter(|other| other != &id && (other.contains(&id) || id.contains(other.as_str())))
            .collect();
        let dup_note = if dupes.is_empty() {
            String::new()
        } else {
            format!(" Note: similar existing skills: {}.", dupes.join(", "))
        };
        Ok(format!(
            "Authored skill '{id}' v{version} ({gold} gold example(s) recorded). Run it with skill_run id=\"{id}\".{dup_note}"
        ))
    }
}

// ---------------------------------------------------------------------------
// skill_improve
// ---------------------------------------------------------------------------

pub struct SkillImproveTool;

#[async_trait]
impl Tool for SkillImproveTool {
    fn name(&self) -> &str {
        "skill_improve"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Offer a better version of an existing skill (new source; read the current one with \
         skill_source first). It is installed as a new version and replay-scored against the \
         skill's recorded gold examples; it becomes active ONLY if it measurably beats the current \
         version. If the improvement ADDS behavior (new inputs the old version can't handle), you \
         MUST pass 1-3 `examples` proving the new behavior — the candidate is verified to reproduce \
         them AND to keep all old gold passing, while the old version must fail them. This is how a \
         skill gets better over time."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id": { "type": "string" },
            "source": { "type": "string", "description": "the improved program" },
            "interpreter": { "type": "string" },
            "description": { "type": "string" },
            "examples": { "type": "array", "description": "for behavior-extending improvements: (input, output) pairs proving the NEW behavior",
                "items": { "type": "object", "properties": {
                    "input": { "type": "string" }, "output": { "type": "string" } },
                    "required": ["input", "output"] } }
        }, "required": ["id", "source"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_skill_author {
            return Err("skill authoring is disabled by policy".into());
        }
        if ctx.taint.is_untrusted() {
            return Err("skill improvement refused: this run read untrusted content".into());
        }
        let id = arg_str(args, "id")?;
        let source = arg_str(args, "source")?;
        let (active_signed, _) = ctx
            .skills
            .load_active(id)
            .map_err(|_| format!("no such skill '{id}'"))?;
        let m = &active_signed.manifest;
        if m.runtime != Runtime::Process {
            return Err(format!(
                "skill '{id}' is a WASM skill — improve it from the dashboard (WAT), not here"
            ));
        }
        if ctx.policy.dry_run {
            return Ok(format!(
                "[dry-run] would offer a candidate for skill '{id}'"
            ));
        }
        if let Some(interp) = args["interpreter"].as_str() {
            if !valid_interpreter(interp) {
                return Err("invalid interpreter (letters, digits, space, and /._- only)".into());
            }
        }
        let candidate = NewSkill {
            id: id.to_string(),
            category: m.category.clone(),
            description: args["description"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| m.description.clone()),
            capabilities: m.capabilities.clone(),
            metric: m.metric.clone(),
            runtime: Runtime::Process,
            interpreter: args["interpreter"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| m.interpreter.clone()),
            when_to_use: m.when_to_use.clone(),
        };
        // Asserted new (input, output) pairs let a behavior-EXTENDING candidate earn promotion: on
        // exact gold an extension can only tie the incumbent, and ties never promote.
        let new_examples: Vec<(String, String)> = args["examples"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let i = e["input"].as_str()?;
                        let o = e["output"].as_str()?;
                        Some((i.to_string(), o.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let host = SkillHost::new();
        let p = params(ctx, &host);
        let decision = improve_skill(
            &ctx.skills,
            id,
            candidate,
            source.as_bytes(),
            &new_examples,
            true,
            "agent",
            &p,
            None,
        )
        .await?;
        Ok(serde_json::to_string(&decision).unwrap_or_else(|_| "improve done".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{NoBrowser, Policy, ToolCtx};
    use engram_core::{Ledger, Taint};
    use engram_gateway::{Gateway, MockProvider};
    use engram_memory::{Memory, TrigramHashEmbedder};
    use engram_skills::{Registry, SkillSigner};
    use std::sync::Arc;

    fn ctx(
        taint: Taint,
        allow_shell: bool,
        allow_skill_author: bool,
    ) -> (ToolCtx, tempfile::TempDir) {
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
        let skills = Arc::new(Registry::open(dir.path(), signer, ledger.clone()).unwrap());
        let gateway = Arc::new(Gateway::new(Box::new(MockProvider), ledger.clone()));
        let workdir = dir.path().join("work");
        std::fs::create_dir_all(&workdir).unwrap();
        let policy = Policy {
            allow_shell,
            allow_skill_author,
            ..Default::default()
        };
        (
            ToolCtx {
                memory,
                skills,
                gateway,
                ledger,
                taint,
                sensitive: false,
                policy,
                workdir,
                model: "test".into(),
                depth: 0,
                browser: Arc::new(NoBrowser),
                scope: engram_core::ScopeCtx::any(),
                halt: None,
                spend_counter: None,
                token_budget: None,
                on_step: None,
                on_narration: None,
                allowed_tools: None,
                agent_actor: None,
            },
            dir,
        )
    }

    fn author_args() -> Value {
        json!({
            "id": "upper",
            "description": "uppercase the input text",
            "when_to_use": "when you need text uppercased",
            "interpreter": "sh",
            "source": "tr a-z A-Z",
            "examples": [{"input": "abc", "output": "ABC"}]
        })
    }

    #[tokio::test]
    async fn author_then_search_then_run() {
        let (c, _d) = ctx(Taint::Trusted, true, true);
        // Author a polyglot (shell) skill from within a "task".
        let msg = SkillAuthorTool.run(&author_args(), &c).await.unwrap();
        assert!(msg.contains("Authored skill 'upper'"), "got: {msg}");
        // It is now findable by the auto-selection tool.
        let found = SkillSearchTool
            .run(&json!({ "query": "uppercase text" }), &c)
            .await
            .unwrap();
        assert!(found.contains("upper"), "search should surface it: {found}");
        // And runnable, producing the program's output.
        let out = SkillRunTool
            .run(&json!({ "id": "upper", "input": "hello" }), &c)
            .await
            .unwrap();
        assert_eq!(out.trim(), "HELLO");
    }

    #[tokio::test]
    async fn authoring_refused_under_taint() {
        let (c, _d) = ctx(Taint::Untrusted, true, true);
        let err = SkillAuthorTool.run(&author_args(), &c).await.unwrap_err();
        assert!(err.contains("untrusted"), "got: {err}");
    }

    #[tokio::test]
    async fn authoring_refused_when_disabled() {
        let (c, _d) = ctx(Taint::Trusted, true, false);
        let err = SkillAuthorTool.run(&author_args(), &c).await.unwrap_err();
        assert!(err.contains("disabled"), "got: {err}");
    }
}
