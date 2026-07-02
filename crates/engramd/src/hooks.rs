//! Lifecycle hooks — user-configured commands the daemon runs when an event fires (a task finishes,
//! an egress is staged, …). This is Claude-Code-style automation: hooks are OFF by default (an empty
//! list), the commands are the user's own (from `config.json`), each run is bounded by a timeout so a
//! hung hook can't wedge anything, and the event payload is handed to the command as JSON on STDIN so
//! a script can react to specifics ("when a task finishes, post its answer to Slack", "on a staged
//! egress, page me"). Every hook is a plain `sh -c` invocation, so pipelines and redirection work.

use serde_json::Value;

/// One configured hook: run `command` when the fired event matches `event`. An empty `event` or `"*"`
/// matches every event; otherwise it is an exact topic match (e.g. `task.done`, `egress.staged`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HookCfg {
    pub event: String,
    pub command: String,
}

/// Whether a configured hook subscribes to a fired event topic.
pub fn hook_matches(hook_event: &str, fired: &str) -> bool {
    let e = hook_event.trim();
    e.is_empty() || e == "*" || e == fired
}

/// Run every hook subscribed to `event`, passing `payload` as JSON on the command's stdin and the
/// topic in `$ENGRAM_HOOK_EVENT`. Best-effort and bounded: a missing/failing/slow command is logged,
/// never fatal, and never blocks past the per-hook timeout. Returns how many hooks were launched.
pub async fn run_hooks(hooks: &[HookCfg], event: &str, payload: &Value) -> usize {
    let body = payload.to_string();
    let mut launched = 0usize;
    for h in hooks
        .iter()
        .filter(|h| hook_matches(&h.event, event) && !h.command.trim().is_empty())
    {
        launched += 1;
        let mut child = match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&h.command)
            .env("ENGRAM_HOOK_EVENT", event)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(event, command = %h.command, error = %e, "hook failed to spawn");
                continue;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(body.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
            Ok(Ok(status)) if !status.success() => {
                tracing::warn!(event, command = %h.command, code = ?status.code(), "hook exited non-zero")
            }
            Ok(Err(e)) => tracing::warn!(event, command = %h.command, error = %e, "hook wait failed"),
            Err(_) => tracing::warn!(event, command = %h.command, "hook timed out (30s), killed"),
            _ => {}
        }
    }
    launched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_semantics() {
        assert!(hook_matches("", "task.done"));
        assert!(hook_matches("*", "task.done"));
        assert!(hook_matches("task.done", "task.done"));
        assert!(hook_matches("  task.done  ", "task.done"));
        assert!(!hook_matches("task.done", "run.start"));
        assert!(!hook_matches("egress.staged", "task.done"));
    }

    #[tokio::test]
    async fn runs_matching_hook_and_pipes_the_payload() {
        // A hook that writes its stdin (the JSON payload) to a marker file proves the event fired
        // AND the payload was delivered. wait() joins the child, so the file exists on return.
        let dir = std::env::temp_dir().join(format!("engram-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("fired.json");
        let hooks = vec![
            HookCfg {
                event: "task.done".into(),
                command: format!("cat > {}", marker.display()),
            },
            // Non-matching hook must NOT run.
            HookCfg {
                event: "run.start".into(),
                command: format!("echo nope > {}", dir.join("nope").display()),
            },
        ];
        let n = run_hooks(&hooks, "task.done", &serde_json::json!({"id":"t1","status":"done"})).await;
        assert_eq!(n, 1, "exactly one hook subscribes to task.done");
        let body = std::fs::read_to_string(&marker).expect("marker written");
        assert!(body.contains("\"id\":\"t1\""), "payload piped to the hook: {body}");
        assert!(!dir.join("nope").exists(), "non-matching hook must not run");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
