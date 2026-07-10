//! # Engram agent - the tool-use loop
//!
//! This is what makes Engram *act*, not just answer. [`Agent`] runs the loop: advertise
//! tools to the model, execute the calls it makes, feed observations back, repeat until
//! it answers. [`Tool`] + [`ToolRegistry`] are the action surface; [`default_tools`]
//! assembles the built-ins (memory, files, shell, web).
//!
//! Engram's edge over a plain tool-loop is baked in here: every tool call is ledgered,
//! filesystem access is workdir-confined, the shell is off by default, and the run is
//! tainted the instant a web tool pulls in untrusted content - after which the shell
//! and secret context are revoked for the rest of the run.

pub mod agent;
#[cfg(feature = "browser-cdp")]
pub mod browser_cdp;
pub mod mcp;
pub mod skills_runtime;
pub mod skills_tools;
pub mod tool;
pub mod tools;

pub use agent::{Agent, AgentError, AgentRun, NarrationCallback, StepCallback, StepRecord};
pub use mcp::{
    connect_servers, connect_servers_full, connect_servers_reported, ConnectedServer, McpClient,
    McpPromptTool, McpResourceTool, McpServerSpec, McpTool,
};
pub use skills_runtime::{
    improve_skill, run_active, score_skill, verify_and_adopt, SkillRunParams,
};
pub use tool::{confine, BrowserSession, NoBrowser, Policy, Tool, ToolCtx, ToolRegistry};

use std::sync::{Arc, RwLock};

/// Process-global deny-list of built-in tool names the user has turned off. The daemon has exactly
/// one SecurityCfg, so a single global is accurate — and consulting it inside [`base_tools`] makes
/// the curation hold for delegated SUBAGENTS too (they build their own toolset via [`sub_tools`] and
/// never pass through the daemon's run chokepoint, so a chokepoint-only filter would leak).
static DISABLED_TOOLS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Replace the global tool deny-list. The daemon calls this at the start of every run with the live
/// config's `disabled_tools`, so it tracks config changes without a restart.
pub fn set_global_disabled_tools(names: Vec<String>) {
    if let Ok(mut g) = DISABLED_TOOLS.write() {
        *g = names;
    }
}

fn is_globally_disabled(name: &str) -> bool {
    DISABLED_TOOLS
        .read()
        .map(|g| g.iter().any(|n| n == name))
        .unwrap_or(false)
}

/// The common toolset (memory, files, shell, web) shared by agents and subagents.
fn base_tools() -> ToolRegistry {
    let reg = ToolRegistry::new()
        .with(Arc::new(tools::UpdatePlanTool))
        // Ask the user a focused question instead of guessing on an ambiguous request.
        .with(Arc::new(tools::ClarifyTool))
        // Request user authorization for a risky/irreversible/egress action (model asks, never grants).
        .with(Arc::new(tools::RequestApprovalTool))
        .with(Arc::new(tools::MemoryRecallTool))
        .with(Arc::new(tools::MemoryRememberTool))
        .with(Arc::new(tools::MemoryRecallPageTool))
        // Verifiable receipts from the signed audit ledger — "prove what you did" (read-only).
        .with(Arc::new(tools::ProofOfActionTool))
        .with(Arc::new(tools::ReadFileTool))
        .with(Arc::new(tools::WriteFileTool))
        .with(Arc::new(tools::EditFileTool))
        // Atomic multi-hunk edits to one file (companion to edit_file; all-or-nothing).
        .with(Arc::new(tools::MultiEditTool))
        .with(Arc::new(tools::AppendFileTool))
        .with(Arc::new(tools::ListDirTool))
        .with(Arc::new(tools::GlobTool))
        .with(Arc::new(tools::GrepTool))
        .with(Arc::new(tools::MakeDirTool))
        .with(Arc::new(tools::MoveFileTool))
        .with(Arc::new(tools::CopyFileTool))
        .with(Arc::new(tools::DeleteFileTool))
        .with(Arc::new(tools::ShellTool))
        // Background processes: start a long-running command, stream its output, kill it.
        .with(Arc::new(tools::ShellStartTool))
        .with(Arc::new(tools::ShellOutputTool))
        .with(Arc::new(tools::ShellKillTool))
        .with(Arc::new(tools::BrowserReadTool))
        .with(Arc::new(tools::BrowserScreenshotTool))
        .with(Arc::new(tools::BrowserOpenTool))
        .with(Arc::new(tools::BrowserClickTool))
        .with(Arc::new(tools::BrowserTypeTool))
        .with(Arc::new(tools::BrowserExtractTool))
        .with(Arc::new(tools::BrowserWaitTool))
        .with(Arc::new(tools::BrowserScrollTool))
        .with(Arc::new(tools::VisionAnalyzeTool))
        .with(Arc::new(tools::ImageGenerateTool))
        .with(Arc::new(tools::TextToSpeechTool))
        .with(Arc::new(tools::TranscribeTool))
        // Self-improving skills: find, run, read, author, and improve small reusable programs.
        .with(Arc::new(skills_tools::SkillSearchTool))
        .with(Arc::new(skills_tools::SkillRunTool))
        .with(Arc::new(skills_tools::SkillSourceTool))
        .with(Arc::new(skills_tools::SkillAuthorTool))
        .with(Arc::new(skills_tools::SkillImproveTool));
    #[cfg(feature = "web")]
    let reg = reg
        .with(Arc::new(tools::WebFetchTool))
        .with(Arc::new(tools::WebSearchTool))
        .with(Arc::new(tools::SendMessageTool));
    // Apply the user's global tool deny-list here so it covers BOTH top-level agents and delegated
    // subagents (the chokepoint additionally filters per-agent allowed_tools + MCP tools).
    reg.retaining(|n| !is_globally_disabled(n))
}

/// The full toolset for a top-level agent - base tools plus subagent delegation.
pub fn default_tools() -> ToolRegistry {
    base_tools().with(Arc::new(tools::DelegateTool))
}

/// The toolset a delegated subagent receives - base tools, no further delegation.
pub fn sub_tools() -> ToolRegistry {
    base_tools()
}

/// Build the interactive browser session: a real CDP-backed Chrome when built with
/// `browser-cdp` and Chrome is present, otherwise a no-op that errors with guidance.
/// Build the browser session at boot. `chrome_override` (a binary path) and `port_override`
/// (the CDP port) come from the daemon's settings; each falls back to auto-detect / the
/// ENGRAM_CHROME / ENGRAM_CDP_PORT env vars when `None`/empty. The session is fixed for the
/// process lifetime, so changing these in the UI is a "restart to apply" setting.
pub fn browser_session(
    chrome_override: Option<String>,
    port_override: Option<u16>,
) -> Arc<dyn BrowserSession> {
    #[cfg(feature = "browser-cdp")]
    {
        let chrome = chrome_override
            .filter(|p| !p.is_empty() && std::path::Path::new(p).exists())
            .or_else(tools::find_chrome);
        if let Some(chrome) = chrome {
            let port = port_override
                .filter(|p| *p != 0)
                .or_else(|| {
                    std::env::var("ENGRAM_CDP_PORT")
                        .ok()
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(9222);
            return Arc::new(browser_cdp::CdpBrowser::new(chrome, port));
        }
    }
    #[cfg(not(feature = "browser-cdp"))]
    {
        let _ = (chrome_override, port_override);
    }
    Arc::new(NoBrowser)
}

/// Whether interactive browser automation is actually available (built with `browser-cdp` AND a
/// Chrome/Chromium binary is on the system). The daemon surfaces this so the UI can show an honest
/// "interactive browsing: on/off" badge instead of advertising a tool that will only error.
pub fn browser_available() -> bool {
    #[cfg(feature = "browser-cdp")]
    {
        tools::find_chrome().is_some()
    }
    #[cfg(not(feature = "browser-cdp"))]
    {
        false
    }
}
