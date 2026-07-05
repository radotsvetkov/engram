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

    /// Projects: the named worlds (memory scope + persona + working directory) work happens in.
    #[command(visible_alias = "proj")]
    Projects {
        #[command(subcommand)]
        cmd: ProjectsCmd,
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

    /// Agent tools: list them, or enable/disable one.
    Tools {
        #[command(subcommand)]
        cmd: Option<ToolsCmd>,
    },

    /// MCP servers: list / add / remove Model Context Protocol tool servers.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },

    /// Chat sessions: list them, or print a session's transcript.
    #[command(visible_alias = "sess")]
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },

    /// Tail the live spike/event bus.
    Events,

    /// Start the daemon (and stay attached until Ctrl-C).
    Serve {
        /// Return immediately once the daemon is healthy instead of staying attached.
        #[arg(long)]
        detach: bool,
    },

    /// Stop a running daemon (it restarts on the next request or `engram serve`).
    Stop,

    /// Restart the daemon in place (picks up a new binary / env).
    Restart,

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
pub enum ProjectsCmd {
    /// List projects (name, id, working directory).
    List,
    /// Create a new project, optionally bound to a working directory.
    /// Quote a multi-word name: `engram project new "My App" --dir ~/code/my-app`.
    New {
        /// The project name (quote it if it has spaces).
        name: String,
        /// Working directory the project's agent operates in (attach-or-create). Omit for none.
        #[arg(long)]
        dir: Option<String>,
    },
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
        /// Time-travel: "what did I believe on this date" (YYYY-MM-DD, or a raw epoch-ms integer).
        /// Omitted = ordinary current-state recall.
        #[arg(long = "as-of")]
        as_of: Option<String>,
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
    /// Edit an existing consciousness line's text in place (pins it).
    IdentityEdit {
        id: String,
        #[arg(trailing_var_arg = true)]
        text: Vec<String>,
    },
    /// Add a new, permanently-pinned consciousness line.
    IdentityAdd {
        #[arg(trailing_var_arg = true)]
        text: Vec<String>,
    },
    /// Remove a consciousness line by id.
    IdentityRemove { id: String },
    /// Revert consciousness to its previous version.
    IdentityRevert,
    /// List proposed contradictions awaiting a decision, or accept/reject one by id.
    Supersessions {
        #[arg(long)]
        accept: Option<i64>,
        #[arg(long)]
        reject: Option<i64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillsCmd {
    /// List skills (optionally filter by substring/category).
    List {
        #[arg(long)]
        filter: Option<String>,
    },
    /// Show a skill's full detail: manifest, versions, and learning history.
    Show { id: String },
    /// Run a skill with an input string (JSON or plain text).
    Run {
        id: String,
        #[arg(trailing_var_arg = true)]
        input: Vec<String>,
    },
    /// Adopt a proposed skill (replays its gold examples; activates on pass).
    Adopt { id: String },
    /// Enable a skill.
    Enable { id: String },
    /// Disable a skill.
    Disable { id: String },
    /// Author a candidate version from a file and A/B-gate it against the active one — replayed
    /// and promoted only if it measurably wins, signed to the ledger either way.
    Improve {
        id: String,
        /// Path to the new version's source: process-skill source, or WAT for a WASM skill.
        #[arg(long)]
        file: String,
        /// Override the interpreter for a process candidate (defaults to the active version's).
        #[arg(long)]
        interpreter: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Revert a skill to its previous version, or an explicit one.
    Revert {
        id: String,
        #[arg(long)]
        version: Option<u32>,
    },
    /// Set the active version of a skill (one-click promote/rollback).
    Activate { id: String, version: u32 },
    /// Record a runtime example as a gold (input, accepted-output) pair on the active version.
    Teach {
        id: String,
        #[arg(long)]
        input: String,
        #[arg(long)]
        gold: String,
        #[arg(long)]
        reward: Option<f32>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolsCmd {
    /// List agent tools with their enabled/disabled state (default).
    List,
    /// Enable a tool (removes it from security.disabled_tools).
    Enable { name: String },
    /// Disable a tool (adds it to security.disabled_tools).
    Disable { name: String },
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// List configured MCP servers.
    List,
    /// Add a server, or update the one with this name.
    Add {
        /// Unique server name.
        name: String,
        /// The executable, e.g. npx, uvx, /path/to/bin.
        command: String,
        /// Space-separated arguments, e.g. --args "-y @modelcontextprotocol/server-filesystem /tmp".
        #[arg(long, allow_hyphen_values = true)]
        args: Option<String>,
        /// Environment pairs, e.g. --env "TOKEN=abc,REGION=eu".
        #[arg(long)]
        env: Option<String>,
        /// Working directory for the server process.
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Remove a server by name.
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum SessionsCmd {
    /// List chat sessions (optionally scoped to a project id).
    List {
        #[arg(long)]
        project: Option<String>,
    },
    /// Print a session's transcript.
    Show { id: String },
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
        // Lifecycle commands must not auto-spawn a daemon just to stop it.
        Cmd::Stop => handlers::stop(&client, json).await,
        Cmd::Restart => handlers::restart(&client, auto_spawn, json).await,
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
