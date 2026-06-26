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
pub mod tool;
pub mod tools;

pub use agent::{Agent, AgentError, AgentRun, StepCallback, StepRecord};
pub use mcp::{connect_servers, McpClient, McpTool};
pub use tool::{confine, BrowserSession, NoBrowser, Policy, Tool, ToolCtx, ToolRegistry};

use std::sync::Arc;

/// The common toolset (memory, files, shell, web) shared by agents and subagents.
fn base_tools() -> ToolRegistry {
    let reg = ToolRegistry::new()
        .with(Arc::new(tools::UpdatePlanTool))
        .with(Arc::new(tools::MemoryRecallTool))
        .with(Arc::new(tools::MemoryRememberTool))
        .with(Arc::new(tools::ReadFileTool))
        .with(Arc::new(tools::WriteFileTool))
        .with(Arc::new(tools::ListDirTool))
        .with(Arc::new(tools::ShellTool))
        .with(Arc::new(tools::BrowserReadTool))
        .with(Arc::new(tools::BrowserScreenshotTool))
        .with(Arc::new(tools::BrowserOpenTool))
        .with(Arc::new(tools::BrowserClickTool))
        .with(Arc::new(tools::BrowserTypeTool))
        .with(Arc::new(tools::BrowserExtractTool))
        .with(Arc::new(tools::VisionAnalyzeTool))
        .with(Arc::new(tools::ImageGenerateTool))
        .with(Arc::new(tools::TextToSpeechTool))
        .with(Arc::new(tools::TranscribeTool));
    #[cfg(feature = "web")]
    let reg = reg
        .with(Arc::new(tools::WebFetchTool))
        .with(Arc::new(tools::WebSearchTool))
        .with(Arc::new(tools::SendMessageTool));
    reg
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
pub fn browser_session() -> Arc<dyn BrowserSession> {
    #[cfg(feature = "browser-cdp")]
    {
        if let Some(chrome) = tools::find_chrome() {
            let port = std::env::var("ENGRAM_CDP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(9222);
            return Arc::new(browser_cdp::CdpBrowser::new(chrome, port));
        }
    }
    Arc::new(NoBrowser)
}
