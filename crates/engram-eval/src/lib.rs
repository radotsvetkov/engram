//! engram-eval - deterministic, offline regression testing for the agent harness.
//!
//! "Tested by vibes" is the near-universal complaint about agents: you change a prompt,
//! a tool, or the loop, and you cannot tell whether behavior regressed. This harness
//! answers it. An eval **case** records a task plus the exact model completions a run
//! received (a recording of real model output). Replaying it drives the *real* agent and
//! *real* tools through a [`ScriptedProvider`], with no model and no network, fully
//! deterministic, then checks the resulting tool sequence, answer, and stop reason against a
//! baseline. Change the harness, re-run the suite, and a regression shows up as a failing
//! case rather than a hunch. The same ledger and replay substrate that makes Engram
//! auditable also makes it testable.

use std::path::Path;
use std::sync::Arc;

use engram_agent::{Agent, NoBrowser, Policy, ToolCtx};
use engram_core::{Ledger, Taint};
use engram_gateway::{Completion, Gateway, ScriptedProvider, ToolCall};
use engram_memory::{Memory, TrigramHashEmbedder};
use engram_skills::{Registry, SkillSigner};
use serde::Deserialize;

/// One eval case: a task, the model's recorded responses, optional run config, and the
/// expected outcome.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub name: String,
    pub task: String,
    /// The model's scripted responses, replayed in order.
    pub completions: Vec<Completion>,
    #[serde(default)]
    pub max_steps: Option<usize>,
    #[serde(default)]
    pub token_budget: Option<u32>,
    #[serde(default)]
    pub reflect: bool,
    #[serde(default)]
    pub expect: Expect,
}

/// What a case must produce. An empty/absent field is not checked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Expect {
    /// The exact ordered tool-call sequence the run must produce.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub answer_contains: Option<String>,
    /// "final" | "limit" | "budget" | "halted" | "loop".
    #[serde(default)]
    pub stopped: Option<String>,
    #[serde(default)]
    pub min_steps: Option<usize>,
}

/// The observed result of replaying a case.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub tools: Vec<String>,
    pub answer: String,
    pub stopped: String,
    pub steps: usize,
}

/// Replay a case deterministically: the model is scripted, but the real [`Agent`] and the
/// real tools run, against a throwaway brain. Returns the observed outcome.
pub async fn run_case(case: &Case) -> Result<Outcome, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let ledger = Arc::new(Ledger::open(dir.path()).map_err(|e| e.to_string())?);
    let memory = Arc::new(
        Memory::open(
            dir.path().join("b.db"),
            Arc::new(TrigramHashEmbedder::default()),
            ledger.clone(),
        )
        .map_err(|e| e.to_string())?,
    );
    let signer =
        Arc::new(SkillSigner::load_or_create(dir.path().join("k")).map_err(|e| e.to_string())?);
    let skills =
        Arc::new(Registry::open(dir.path(), signer, ledger.clone()).map_err(|e| e.to_string())?);
    let gateway = Arc::new(Gateway::new(
        Box::new(ScriptedProvider::new(case.completions.clone())),
        ledger.clone(),
    ));
    let ctx = ToolCtx {
        memory,
        skills,
        gateway: gateway.clone(),
        ledger,
        taint: Taint::Trusted,
        sensitive: false,
        policy: Policy::default(),
        workdir: dir.path().to_path_buf(),
        model: "eval".into(),
        depth: 0,
        browser: Arc::new(NoBrowser),
        scope: engram_core::ScopeCtx::any(),
        halt: None,
        spend_counter: None,
        token_budget: None,
        on_step: None,
        on_narration: None,
        allowed_tools: None,
    };
    let mut agent = Agent::new(gateway, engram_agent::default_tools(), "eval")
        .max_steps(case.max_steps.unwrap_or(12))
        .reflect(case.reflect);
    if let Some(b) = case.token_budget {
        agent = agent.token_budget(b);
    }
    let run = agent
        .run(&case.task, ctx)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Outcome {
        tools: run.steps.iter().map(|s| s.tool.clone()).collect(),
        answer: run.answer,
        stopped: run.stopped.to_string(),
        steps: run.steps.len(),
    })
}

/// Compare an outcome to a case's expectations; returns human-readable failures (empty = pass).
pub fn check(case: &Case, out: &Outcome) -> Vec<String> {
    let mut fails = Vec::new();
    if !case.expect.tools.is_empty() && out.tools != case.expect.tools {
        fails.push(format!(
            "tool sequence: expected {:?}, got {:?}",
            case.expect.tools, out.tools
        ));
    }
    if let Some(s) = &case.expect.answer_contains {
        if !out.answer.contains(s) {
            fails.push(format!("answer should contain {s:?}, got {:?}", out.answer));
        }
    }
    if let Some(s) = &case.expect.stopped {
        if &out.stopped != s {
            fails.push(format!("stopped: expected {s:?}, got {:?}", out.stopped));
        }
    }
    if let Some(n) = case.expect.min_steps {
        if out.steps < n {
            fails.push(format!("min_steps: expected >= {n}, got {}", out.steps));
        }
    }
    fails
}

/// Load `*.json` eval cases from a directory, sorted by filename.
pub fn load_dir(path: &Path) -> Result<Vec<Case>, String> {
    let rd = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut paths: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    paths.sort();
    let mut cases = Vec::new();
    for p in paths {
        let txt = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let case: Case = serde_json::from_str(&txt).map_err(|e| format!("{}: {e}", p.display()))?;
        cases.push(case);
    }
    Ok(cases)
}

// --- helpers for building cases in code (the built-in suite) ---

fn call(id: &str, name: &str, args: serde_json::Value) -> Completion {
    Completion {
        text: String::new(),
        model: "eval".into(),
        tokens_in: 10,
        tokens_out: 0,
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args,
        }],
    }
}

fn answer(text: &str) -> Completion {
    Completion {
        text: text.into(),
        model: "eval".into(),
        tokens_in: 5,
        tokens_out: 5,
        tool_calls: vec![],
    }
}

/// The built-in regression suite - exercises core harness behaviors using only
/// deterministic, offline-safe tools (memory + planning), so it runs anywhere with no
/// model and no network.
pub fn builtin_cases() -> Vec<Case> {
    use serde_json::json;
    vec![
        Case {
            name: "remembers-then-recalls-then-answers".into(),
            task: "Remember that I love Rust, then recall it and tell me.".into(),
            completions: vec![
                call(
                    "1",
                    "memory_remember",
                    json!({ "text": "User loves Rust", "region": "identity" }),
                ),
                call(
                    "2",
                    "memory_recall",
                    json!({ "query": "what does the user love" }),
                ),
                answer("You love Rust."),
            ],
            max_steps: None,
            token_budget: None,
            reflect: false,
            expect: Expect {
                tools: vec!["memory_remember".into(), "memory_recall".into()],
                answer_contains: Some("Rust".into()),
                stopped: Some("final".into()),
                min_steps: Some(2),
            },
        },
        Case {
            name: "maintains-an-explicit-plan".into(),
            task: "Plan a two-step task.".into(),
            completions: vec![
                call(
                    "1",
                    "update_plan",
                    json!({ "steps": [{ "title": "research", "status": "doing" }, { "title": "write", "status": "todo" }] }),
                ),
                answer("Plan recorded."),
            ],
            max_steps: None,
            token_budget: None,
            reflect: false,
            expect: Expect {
                tools: vec!["update_plan".into()],
                answer_contains: None,
                stopped: Some("final".into()),
                min_steps: None,
            },
        },
        Case {
            name: "stops-on-token-budget".into(),
            task: "Do an expensive thing.".into(),
            completions: vec![
                Completion {
                    text: String::new(),
                    model: "eval".into(),
                    tokens_in: 600,
                    tokens_out: 0,
                    tool_calls: vec![ToolCall {
                        id: "1".into(),
                        name: "memory_recall".into(),
                        arguments: json!({ "query": "a" }),
                    }],
                },
                Completion {
                    text: String::new(),
                    model: "eval".into(),
                    tokens_in: 600,
                    tokens_out: 0,
                    tool_calls: vec![ToolCall {
                        id: "2".into(),
                        name: "memory_recall".into(),
                        arguments: json!({ "query": "b" }),
                    }],
                },
                answer("done"),
            ],
            max_steps: Some(10),
            token_budget: Some(1000),
            reflect: false,
            expect: Expect {
                tools: vec![],
                answer_contains: None,
                stopped: Some("budget".into()),
                min_steps: None,
            },
        },
        Case {
            name: "stops-on-a-repeating-loop".into(),
            task: "Get stuck.".into(),
            completions: vec![
                call("1", "memory_recall", json!({ "query": "stuck" })),
                call("1", "memory_recall", json!({ "query": "stuck" })),
                call("1", "memory_recall", json!({ "query": "stuck" })),
                answer("done"),
            ],
            max_steps: Some(10),
            token_budget: None,
            reflect: false,
            expect: Expect {
                tools: vec![],
                answer_contains: None,
                stopped: Some("loop".into()),
                min_steps: None,
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn builtin_suite_all_pass() {
        for case in builtin_cases() {
            let out = run_case(&case)
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", case.name));
            let fails = check(&case, &out);
            assert!(
                fails.is_empty(),
                "{} failed: {:?} (outcome: {:?})",
                case.name,
                fails,
                out
            );
        }
    }

    #[tokio::test]
    async fn a_regression_is_detected() {
        // Flip an expectation: the suite must catch the mismatch rather than pass blindly.
        let mut case = builtin_cases().into_iter().next().unwrap();
        case.expect.stopped = Some("limit".into()); // wrong on purpose
        let out = run_case(&case).await.unwrap();
        assert!(
            !check(&case, &out).is_empty(),
            "a wrong expectation must fail"
        );
    }

    #[test]
    fn cases_round_trip_through_json() {
        let case = &builtin_cases()[0];
        let json = serde_json::to_string(&CaseWire::from(case)).unwrap();
        let back: Case = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, case.name);
        assert_eq!(back.completions.len(), case.completions.len());
    }

    // Minimal serializable mirror so the test can round-trip a Case through JSON (Case is
    // deserialize-only in the library; the loader reads cases authored/recorded as JSON).
    #[derive(serde::Serialize)]
    struct CaseWire<'a> {
        name: &'a str,
        task: &'a str,
        completions: &'a [Completion],
    }
    impl<'a> From<&'a Case> for CaseWire<'a> {
        fn from(c: &'a Case) -> Self {
            CaseWire {
                name: &c.name,
                task: &c.task,
                completions: &c.completions,
            }
        }
    }
}
