//! engram — the command-line and terminal-UI client for the Engram personal agent.
//!
//! Run with no arguments to open the full-screen TUI; use a subcommand for
//! scripting. Everything talks to a local `engramd` daemon over its HTTP API,
//! which is auto-started if it isn't already running.

mod api;
mod cli;
mod tui;
mod ui;

use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();
    match cli::run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            // The TUI restores the terminal in its own teardown; for CLI errors a
            // plain stderr line is right.
            eprintln!("{} {e:#}", cli::output::bad("error:"));
            std::process::exit(1);
        }
    }
}
