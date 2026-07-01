//! The `engram` command-line surface: a full set of subcommands over the
//! daemon, plus the default no-arg behaviour of launching the TUI.

pub mod daemon;
pub mod handlers;
pub mod output;

use crate::api::{self, Client};
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "engram",
    version,
    about = "engram — command-line & terminal-UI for your Engram personal agent",
    long_about = "engram talks to a local engramd daemon over HTTP.\n\
                  Run with no arguments to open the full-screen TUI, or use a subcommand\n\
                  for scripting. The daemon is auto-started if it isn't already running.",
    propagate_version = true,
    disable_help_subcommand = false
)]
pub struct Cli {
    /// Daemon address (host:port or full URL). Default: $ENGRAM_ADDR or 127.0.0.1:8088.
    #[arg(long, global = true, value_name = "ADDR")]
    pub addr: Option<String>,

    /// Bearer token for an exposed daemon. Default: $ENGRAM_API_TOKEN.
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Emit machine-readable JSON instead of pretty output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Never auto-start the daemon; fail if it isn't already running.
    #[arg(long, global = true)]
    pub no_spawn: bool,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Open the full-screen interactive TUI (default).
    Tui,

    /// Ask a question / chat in one shot (streams tool steps, then the answer).
    #[command(visible_alias = "chat")]
    Ask {
        /// Your message. If omitted, reads from stdin.
        #[arg(trailing_var_arg = true)]
        prompt: Vec<String>,
        /// Print only the final answer (suppress the live tool trace).
        #[arg(long)]
        quiet: bool,
    },

    /// Run the tool-using agent on a task (one-shot, non-interactive).
    Run {
        /// The task to perform.
        #[arg(trailing_var_arg = true)]
        task: Vec<String>,
        /// Maximum agent steps.
        #[arg(long)]
        max_steps: Option<usize>,
    },

    /// One-line health, cost, memory, and ledger summary.
    Status,

    /// Deep diagnostics: provider, tools, ledger integrity, config.
    Doctor,

    /// Tasks: the kanban board behind the scenes.
    Tasks {
        #[command(subcommand)]
        cmd: TasksCmd,
    },

    /// Memory: the brain's regions, recall, and the self-model.
    #[command(visible_alias = "mem")]
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },

    /// Skills: the self-improving program library.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },

    /// Schedule: recurring jobs that wake the daemon.
    Schedule {
        #[command(subcommand)]
        cmd: ScheduleCmd,
    },

    /// Autonomy: the egress policy report and pending approvals.
    Autonomy {
        #[command(subcommand)]
        cmd: AutonomyCmd,
    },

    /// Ledger: the signed, append-only audit chain.
    Ledger {
        #[command(subcommand)]
        cmd: LedgerCmd,
    },

    /// Configuration of the running daemon.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },

    /// Named agents: list / create / edit / delete / set autonomy policy.
    Agents {
        #[command(subcommand)]
        cmd: Option<AgentsCmd>,
    },

    /// Available agent tools.
    Tools,

    /// Tail the live spike/event bus.
    Events,

    /// Start the daemon (and stay attached until Ctrl-C).
    Serve {
        /// Return immediately once the daemon is healthy instead of staying attached.
        #[arg(long)]
        detach: bool,
    },

    /// Generate shell completion script (bash, zsh, fish, powershell, elvish).
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand, Debug)]
pub enum TasksCmd {
    /// List tasks grouped by column.
    List,
    /// Show a task's detail and signed receipt.
    Show { id: String },
    /// Create a new task.
    New {
        #[arg(trailing_var_arg = true)]
        title: Vec<String>,
        #[arg(long)]
        detail: Option<String>,
        /// Create and immediately run it (streaming).
        #[arg(long)]
        run: bool,
    },
    /// Run an existing task by id (streaming).
    Run { id: String },
    /// Print the signed receipt JSON for a finished task.
    Receipt { id: String },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCmd {
    /// Region/tier counts.
    Stats,
    /// Most recently touched memories.
    Recent {
        #[arg(long)]
        region: Option<String>,
        #[arg(long, default_value_t = 20)]
        n: usize,
    },
    /// Hybrid (keyword + semantic) recall.
    Recall {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 8)]
        k: usize,
    },
    /// Write a memory.
    Remember {
        #[arg(trailing_var_arg = true)]
        text: Vec<String>,
        #[arg(long, default_value = "semantic")]
        region: String,
        #[arg(long)]
        importance: Option<f32>,
    },
    /// Forget a memory by id.
    Forget { id: i64 },
    /// The distilled self-model (consciousness).
    Identity {
        /// Re-distill before showing.
        #[arg(long)]
        distill: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillsCmd {
    /// List skills (optionally filter by substring/category).
    List {
        #[arg(long)]
        filter: Option<String>,
    },
    /// Run a skill with an input string (JSON or plain text).
    Run {
        id: String,
        #[arg(trailing_var_arg = true)]
        input: Vec<String>,
    },
    /// Enable a skill.
    Enable { id: String },
    /// Disable a skill.
    Disable { id: String },
}

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// List named agents.
    List,
    /// Create an agent.
    Create {
        name: String,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        emoji: Option<String>,
    },
    /// Update an agent's fields.
    Edit {
        id: String,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        emoji: Option<String>,
    },
    /// Delete an agent.
    Delete { id: String },
    /// Set an agent's signed autonomy policy (allowlist + budget + expiry).
    Policy {
        id: String,
        /// Comma-separated allowed egress destinations.
        #[arg(long)]
        egress: Option<String>,
        /// Comma-separated allowed actions (send, post, …).
        #[arg(long)]
        actions: Option<String>,
        #[arg(long, default_value_t = 0)]
        max_actions: u64,
        #[arg(long)]
        max_spend_cents: Option<u64>,
        #[arg(long)]
        expires_days: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScheduleCmd {
    /// List scheduled jobs with their next fire time.
    List,
    /// Add a recurring job (natural-language cadence).
    Add {
        name: String,
        when: String,
        /// The task title to run on each fire.
        #[arg(long)]
        title: Option<String>,
    },
    /// Preview when a cadence expression would next fire (no model call).
    Preview {
        #[arg(trailing_var_arg = true)]
        when: Vec<String>,
    },
    /// Fire a job now.
    Run { id: String },
    /// Delete a scheduled job.
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
pub enum AutonomyCmd {
    /// The autonomy/egress totals and per-scope breakdown.
    Report,
    /// Staged egress actions awaiting approval.
    Pending,
    /// Approve a staged egress (allowlist the destination).
    Approve { scope: String, dest: String },
    /// Deny a staged egress.
    Deny { scope: String, dest: String },
}

#[derive(Subcommand, Debug)]
pub enum LedgerCmd {
    /// Tail the last N ledger entries.
    Tail {
        #[arg(long, default_value_t = 20)]
        n: usize,
    },
    /// Verify the signed chain.
    Verify,
    /// Print the ledger public key.
    Pubkey,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show the full (redacted) config.
    Show,
    /// Set a dotted config key to a JSON value, e.g. `provider.model "claude-..."`.
    Set { key: String, value: String },
    /// Test the configured model provider with a tiny completion.
    Test,
}

/// Build a client from global flags + environment.
pub fn client_from(cli: &Cli) -> Client {
    let base = api::resolve_base(cli.addr.as_deref());
    let token = api::resolve_token(cli.token.as_deref());
    Client::new(base, token)
}

/// Dispatch a parsed CLI to its handler. Returns a process exit code.
pub async fn run(cli: Cli) -> Result<i32> {
    let client = client_from(&cli);
    let json = cli.json;
    let auto_spawn = !cli.no_spawn;

    // Commands that need the daemon ensure it first (Completions/Serve handle themselves).
    let cmd = cli.cmd.unwrap_or(Cmd::Tui);

    match cmd {
        Cmd::Completions { shell } => {
            handlers::completions(shell);
            Ok(0)
        }
        Cmd::Serve { detach } => handlers::serve(&client, detach).await,
        Cmd::Tui => {
            daemon::ensure(&client, auto_spawn, false).await?;
            crate::tui::run(client).await?;
            Ok(0)
        }
        other => {
            daemon::ensure(&client, auto_spawn, json).await?;
            handlers::dispatch(&client, other, json).await
        }
    }
}
