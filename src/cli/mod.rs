use clap::{Parser, Subcommand};

pub mod attach;
pub mod down;
pub mod init;
pub mod logs;
pub mod peek;
pub mod run;
pub mod snapshot;
pub mod status;
pub mod switch;
pub mod up;
pub mod usage;
pub mod verify;
pub mod watch;
pub mod workspaces;

#[derive(Parser)]
#[command(name = "tt", about = "tutti — your agents, all together", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new tutti.toml in the current directory
    Init,

    /// Launch agent sessions
    Up {
        /// Launch only this agent (default: all)
        agent: Option<String>,

        /// Target a specific workspace by name (default: current directory)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Launch all agents in all registered workspaces
        #[arg(long)]
        all: bool,
    },

    /// Stop agent sessions
    Down {
        /// Stop only this agent (default: all)
        agent: Option<String>,

        /// Target a specific workspace by name (default: current directory)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Stop all agents in all registered workspaces
        #[arg(long)]
        all: bool,

        /// Also remove git worktrees
        #[arg(long)]
        clean: bool,
    },

    /// Show status of all agents
    Status {
        /// Show all registered workspaces
        #[arg(long)]
        all: bool,
    },

    /// Show status of all agents (alias for status)
    Voices {
        /// Show all registered workspaces
        #[arg(long)]
        all: bool,
    },

    /// Live-updating status dashboard
    Watch {
        /// Refresh interval in seconds (default: 2)
        #[arg(short, long, default_value = "2")]
        interval: u64,

        /// Auto-restart crashed agents marked `persistent = true`
        #[arg(long)]
        restart_persistent: bool,
    },

    /// Attach to an agent's terminal session
    Attach {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,
    },

    /// Read-only view of an agent's terminal
    Peek {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Number of lines to capture (default: 50)
        #[arg(short, long, default_value = "50")]
        lines: u32,
    },

    /// Tail captured logs for an agent
    Logs {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Number of lines to show initially (default: 50)
        #[arg(short, long, default_value = "50")]
        lines: u32,

        /// Follow log output
        #[arg(short = 'f', long)]
        follow: bool,
    },

    /// Fuzzy picker for running agents; attach with Enter
    Switch,

    /// Run a reusable workflow (prompt + command steps)
    Run {
        /// Workflow name
        workflow: String,

        /// Override target agent for agent-scoped steps
        #[arg(long)]
        agent: Option<String>,

        /// Force fail-closed behavior for command steps
        #[arg(long)]
        strict: bool,

        /// Print resolved steps without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Run verification workflow and persist latest summary
    Verify {
        /// Workflow name (default: verify)
        #[arg(long)]
        workflow: Option<String>,

        /// Override target agent for agent-scoped steps
        #[arg(long)]
        agent: Option<String>,

        /// Fail on any step error
        #[arg(long)]
        strict: bool,
    },

    /// Show API-profile capacity and token usage
    Usage {
        /// Filter to a specific profile
        #[arg(short = 'p', long)]
        profile: Option<String>,

        /// Break down usage by workspace
        #[arg(long)]
        by_workspace: bool,
    },

    /// List all registered workspaces
    Workspaces {
        #[command(subcommand)]
        command: Option<WorkspacesSubcommand>,
    },
}

#[derive(Subcommand)]
pub enum WorkspacesSubcommand {
    /// Show status overview of all workspaces
    Status,
}
