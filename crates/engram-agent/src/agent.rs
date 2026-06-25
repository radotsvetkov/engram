//! The agent loop — where the model stops talking and starts *doing*.
//!
//! Given a task, the agent advertises its tools to the model, lets the model call
//! them, runs each call, feeds the observation back, and repeats until the model
//! answers with no further tool call (or a step budget is hit). Every step is
//! ledgered, and the run's taint is raised the moment a tool reads untrusted content —
//! after which the shell and secret context are off the table for the rest of the run.

use std::sync::Arc;

use engram_core::Taint;
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use serde::Serialize;
use serde_json::json;

use crate::tool::{ToolCtx, ToolRegistry};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("gateway: {0}")]
    Gateway(#[from] engram_gateway::GatewayError),
}

#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    pub tool: String,
    pub args: serde_json::Value,
    pub observation: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentRun {
    pub answer: String,
    pub steps: Vec<StepRecord>,
    /// "final" if the model answered, "limit" if the step budget ran out.
    pub stopped: &'static str,
}

pub struct Agent {
    gateway: Arc<Gateway>,
    tools: ToolRegistry,
    model: String,
    max_steps: usize,
    /// Personality / standing instructions (from SOUL.md), prepended to the prompt.
    persona: Option<String>,
}

impl Agent {
    pub fn new(gateway: Arc<Gateway>, tools: ToolRegistry, model: impl Into<String>) -> Self {
        Self { gateway, tools, model: model.into(), max_steps: 8, persona: None }
    }
    pub fn max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }
    /// Set the agent's persona / standing instructions.
    pub fn persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = Some(persona.into());
        self
    }

    pub async fn run(&self, task: &str, mut ctx: ToolCtx) -> Result<AgentRun, AgentError> {
        let tool_defs = self.tools.defs();
        let mut system = String::new();
        if let Some(p) = &self.persona {
            system.push_str(p);
            system.push_str("\n\n");
        }
        system.push_str(&format!(
            "You are Engram, an autonomous agent that completes the user's task by calling tools. \
             Work step by step: call tools to gather information and take actions, observe the \
             results, and continue. When the task is done, reply in plain text with NO tool call. \
             Tools available: {}.",
            self.tools.names().join(", ")
        ));
        let mut messages = vec![Message::system(system), Message::user(task)];
        let mut steps = Vec::new();
        let _ = ctx.ledger.append("agent.start", "agent", json!({ "task": task }));

        for _ in 0..self.max_steps {
            let req = CompletionRequest::new(&self.model, messages.clone())
                .tools(tool_defs.clone())
                .max_tokens(1024);
            let completion =
                self.gateway.complete(Call::new(req).actor("agent").tainted(ctx.taint)).await?;

            if completion.tool_calls.is_empty() {
                let _ = ctx.ledger.append("agent.finish", "agent", json!({ "steps": steps.len() }));
                return Ok(AgentRun { answer: completion.text, steps, stopped: "final" });
            }

            messages.push(Message::assistant_tool_calls(
                completion.text.clone(),
                completion.tool_calls.clone(),
            ));
            for call in &completion.tool_calls {
                let (observation, ok) = match self.tools.get(&call.name) {
                    Some(tool) => match tool.run(&call.arguments, &ctx).await {
                        Ok(o) => {
                            if tool.taints() {
                                ctx.taint = Taint::Untrusted;
                            }
                            (o, true)
                        }
                        Err(e) => (format!("error: {e}"), false),
                    },
                    None => (format!("error: unknown tool '{}'", call.name), false),
                };
                let truncated = truncate(&observation, ctx.policy.max_obs_len);
                let _ = ctx
                    .ledger
                    .append("agent.tool", "agent", json!({ "tool": call.name, "ok": ok }));
                messages.push(Message::tool_result(call.id.clone(), truncated.clone()));
                steps.push(StepRecord {
                    tool: call.name.clone(),
                    args: call.arguments.clone(),
                    observation: truncated,
                    ok,
                });
            }
        }
        let _ = ctx
            .ledger
            .append("agent.finish", "agent", json!({ "steps": steps.len(), "limit": true }));
        Ok(AgentRun {
            answer: "(reached step limit without a final answer)".into(),
            steps,
            stopped: "limit",
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated {} bytes]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Policy, ToolCtx};
    use engram_core::Ledger;
    use engram_gateway::{Completion, ScriptedProvider, ToolCall};
    use engram_memory::{Memory, TrigramHashEmbedder};
    use engram_skills::{Registry, SkillSigner};

    fn call(id: &str, name: &str, args: serde_json::Value) -> Completion {
        Completion {
            text: String::new(),
            model: "test".into(),
            tokens_in: 0,
            tokens_out: 0,
            tool_calls: vec![ToolCall { id: id.into(), name: name.into(), arguments: args }],
        }
    }
    fn final_answer(text: &str) -> Completion {
        Completion { text: text.into(), model: "test".into(), tokens_in: 0, tokens_out: 1, tool_calls: vec![] }
    }

    #[tokio::test]
    async fn agent_executes_tools_then_answers() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Arc::new(
            Memory::open(dir.path().join("b.db"), Arc::new(TrigramHashEmbedder::default()), ledger.clone()).unwrap(),
        );
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("k")).unwrap());
        let skills = Arc::new(Registry::open(dir.path(), signer, ledger.clone()).unwrap());

        // Scripted model: remember a fact, recall it, then answer.
        let provider = ScriptedProvider::new(vec![
            call("1", "memory_remember", json!({ "text": "the sky is blue", "region": "semantic" })),
            call("2", "memory_recall", json!({ "query": "what colour is the sky" })),
            final_answer("The sky is blue."),
        ]);
        let gateway = Arc::new(Gateway::new(Box::new(provider), ledger.clone()));

        let ctx = ToolCtx {
            memory,
            skills,
            gateway: gateway.clone(),
            ledger: ledger.clone(),
            taint: Taint::Trusted,
            policy: Policy::default(),
            workdir: dir.path().to_path_buf(),
            model: "test".into(),
            depth: 0,
        };
        let agent = Agent::new(gateway, crate::default_tools(), "test");
        let run = agent.run("Tell me the colour of the sky.", ctx).await.unwrap();

        assert_eq!(run.stopped, "final");
        assert_eq!(run.answer, "The sky is blue.");
        assert_eq!(run.steps.len(), 2);
        assert_eq!(run.steps[0].tool, "memory_remember");
        assert!(run.steps[1].observation.contains("blue"), "recall should see the fact");
        assert!(ledger.verify().unwrap() > 0);
    }

    #[tokio::test]
    async fn shell_is_refused_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Arc::new(
            Memory::open(dir.path().join("b.db"), Arc::new(TrigramHashEmbedder::default()), ledger.clone()).unwrap(),
        );
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("k")).unwrap());
        let skills = Arc::new(Registry::open(dir.path(), signer, ledger.clone()).unwrap());
        let provider = ScriptedProvider::new(vec![
            call("1", "shell", json!({ "command": "echo hi" })),
            final_answer("could not run shell"),
        ]);
        let gateway = Arc::new(Gateway::new(Box::new(provider), ledger.clone()));
        let ctx = ToolCtx {
            memory,
            skills,
            gateway: gateway.clone(),
            ledger,
            taint: Taint::Trusted,
            policy: Policy::default(), // allow_shell = false
            workdir: dir.path().to_path_buf(),
            model: "test".into(),
            depth: 0,
        };
        let run = Agent::new(gateway, crate::default_tools(), "test").run("run echo", ctx).await.unwrap();
        assert!(!run.steps[0].ok);
        assert!(run.steps[0].observation.contains("disabled"));
    }

    #[tokio::test]
    async fn delegate_runs_a_subagent() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Arc::new(
            Memory::open(dir.path().join("b.db"), Arc::new(TrigramHashEmbedder::default()), ledger.clone()).unwrap(),
        );
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("k")).unwrap());
        let skills = Arc::new(Registry::open(dir.path(), signer, ledger.clone()).unwrap());
        // Parent delegates; the subagent (sharing the scripted model) answers; parent answers.
        let provider = ScriptedProvider::new(vec![
            call("1", "delegate_task", json!({ "task": "compute the subresult" })),
            final_answer("subresult: 42"),
            final_answer("done — got subresult: 42"),
        ]);
        let gateway = Arc::new(Gateway::new(Box::new(provider), ledger.clone()));
        let ctx = ToolCtx {
            memory,
            skills,
            gateway: gateway.clone(),
            ledger,
            taint: Taint::Trusted,
            policy: Policy::default(),
            workdir: dir.path().to_path_buf(),
            model: "test".into(),
            depth: 0,
        };
        let run = Agent::new(gateway, crate::default_tools(), "test").run("do the thing", ctx).await.unwrap();
        assert_eq!(run.steps.len(), 1);
        assert_eq!(run.steps[0].tool, "delegate_task");
        assert!(run.steps[0].observation.contains("subresult: 42"), "subagent result should bubble up");
        assert!(run.answer.contains("done"));
    }
}
