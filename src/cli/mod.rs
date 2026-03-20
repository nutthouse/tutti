use clap::{Parser, Subcommand, ValueEnum};

pub mod agent_ref;
pub mod attach;
pub mod detect;
pub mod diff;
pub mod doctor;
pub mod down;
pub mod handoff;
pub mod health;
pub mod init;
pub mod issue_claim;
pub mod land;
pub mod logs;
pub mod peek;
pub mod permissions;
pub mod remote;
pub mod review;
pub mod run;
pub mod runs;
pub mod send;
pub mod serve;
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

        /// Recreate agent worktree(s) from current workspace HEAD before launch
        #[arg(long)]
        fresh_worktree: bool,

        /// Launch behavior for permission handling
        #[arg(long, value_enum)]
        mode: Option<UpLaunchMode>,

        /// Policy behavior when mode is unattended
        #[arg(long, value_enum)]
        policy: Option<UpLaunchPolicy>,
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

    /// Check workspace readiness (tools, env, auth profile, runtimes)
    Doctor {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Treat warnings as failures (CI-friendly)
        #[arg(long)]
        strict: bool,
    },

    /// Attach to an agent's terminal session
    Attach {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,
    },

    /// Show git changes for an agent worktree
    Diff {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Show staged diff only
        #[arg(long)]
        staged: bool,

        /// Show names only
        #[arg(long, conflicts_with = "stat")]
        name_only: bool,

        /// Show diff stat summary
        #[arg(long)]
        stat: bool,
    },

    /// Explain runtime detector matches for a running agent session
    Detect {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Number of pane lines to inspect
        #[arg(short, long, default_value = "200")]
        lines: u32,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Land an agent branch back into current branch
    Land {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Push agent branch and open a PR instead of local cherry-pick
        #[arg(long)]
        pr: bool,

        /// Skip local branch cleanliness checks before landing
        #[arg(long)]
        force: bool,
    },

    /// Send an agent's diff packet to a reviewer agent
    Review {
        /// Source agent name (or workspace/agent)
        agent: String,

        /// Reviewer agent target (default: reviewer)
        #[arg(long, default_value = "reviewer")]
        reviewer: String,
    },

    /// Send a one-off prompt to a running agent session
    Send {
        /// Agent name (or workspace/agent for cross-workspace)
        agent: String,

        /// Start the agent if it is not already running
        #[arg(long)]
        auto_up: bool,

        /// Wait for activity -> idle completion after sending
        #[arg(long)]
        wait: bool,

        /// Maximum time to wait when --wait is enabled (seconds)
        #[arg(long, default_value = "900")]
        timeout_secs: u64,

        /// Idle stability window required for --wait completion (seconds)
        #[arg(long, default_value = "5")]
        idle_stable_secs: u64,

        /// Print captured response text after send (best-effort pane delta)
        #[arg(long)]
        output: bool,

        /// Number of pane lines to consider when --output is enabled
        #[arg(long, default_value = "200")]
        output_lines: u32,

        /// Prompt text to send
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true)]
        prompt: Vec<String>,
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

    /// Probe and display agent health from .tutti/state/health
    Health {
        /// Agent name (optional). With --all, matches across workspaces.
        agent: Option<String>,

        /// Target a specific workspace by name (default: current directory)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Probe all registered workspaces
        #[arg(long)]
        all: bool,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Run scheduler + health probes + local health HTTP endpoint
    Serve {
        /// Target a specific workspace by name (default: current directory)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Run for all registered workspaces
        #[arg(long)]
        all: bool,

        /// Health HTTP port (default: global dashboard/default port)
        #[arg(long)]
        port: Option<u16>,

        /// Probe/scheduler tick interval in seconds
        #[arg(long, default_value = "15")]
        probe_interval: u64,

        /// Bind to all interfaces (0.0.0.0) with bearer-token auth for remote access
        #[arg(long)]
        remote: bool,

        /// Custom bind address (default: 127.0.0.1, or 0.0.0.0 with --remote)
        #[arg(long)]
        bind: Option<String>,
    },

    /// Fuzzy picker for running agents; attach with Enter
    Switch,

    /// Generate/apply handoff packets for agent session transfer
    Handoff {
        #[command(subcommand)]
        command: HandoffSubcommand,
    },

    /// Run a reusable workflow (prompt + command steps)
    Run {
        /// Workflow name
        #[arg(required_unless_present_any = ["list", "resume"], conflicts_with = "resume")]
        workflow: Option<String>,

        /// Resume a failed run from checkpoint run id
        #[arg(long, conflicts_with_all = ["list", "workflow", "dry_run"])]
        resume: Option<String>,

        /// List configured workflows and exit
        #[arg(long, conflicts_with_all = ["agent", "strict", "dry_run", "workflow", "resume"])]
        list: bool,

        /// Override target agent for agent-scoped steps
        #[arg(long)]
        agent: Option<String>,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Force fail-closed behavior for command steps
        #[arg(long)]
        strict: bool,

        /// Print resolved steps without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Inspect SDLC run history
    Runs {
        #[command(subcommand)]
        command: RunsSubcommand,
    },

    /// Run verification workflow and persist latest summary
    Verify {
        /// Show the latest verification summary and exit
        #[arg(long, conflicts_with_all = ["workflow", "agent", "strict"])]
        last: bool,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,

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

    /// Evaluate/export command permission policy
    Permissions {
        #[command(subcommand)]
        command: PermissionsSubcommand,
    },

    /// List all registered workspaces
    Workspaces {
        #[command(subcommand)]
        command: Option<WorkspacesSubcommand>,
    },

    /// Manage issue claim leases for automation
    IssueClaim {
        #[command(subcommand)]
        command: IssueClaimSubcommand,
    },

    /// Manage remote tutti hosts (SSH tunnels, status)
    Remote {
        #[command(subcommand)]
        command: RemoteSubcommand,
    },
}

#[derive(Subcommand)]
pub enum WorkspacesSubcommand {
    /// Show status overview of all workspaces
    Status,
}

#[derive(Subcommand)]
pub enum RunsSubcommand {
    /// List all tracked SDLC runs
    List,
    /// Show detailed view of a single run
    Show {
        /// Run ID to inspect
        run_id: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UpLaunchMode {
    Safe,
    Auto,
    Unattended,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UpLaunchPolicy {
    Constrained,
    Bypass,
}

#[derive(Subcommand)]
pub enum PermissionsSubcommand {
    /// Check whether a command is allowed by global policy
    Check {
        /// Command to evaluate against allowed prefixes
        #[arg(required = true)]
        command: Vec<String>,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Suggest permission rules required for a workflow's command steps
    Suggest {
        /// Workflow name to inspect
        workflow: String,

        /// Append suggested rules to global config
        #[arg(long)]
        apply: bool,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Export runtime scaffolding from policy
    Export {
        /// Target runtime settings format
        #[arg(long, default_value = "claude")]
        runtime: String,

        /// Write output to a file path instead of stdout
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum IssueClaimSubcommand {
    /// Acquire an exclusive lease on the next unclaimed issue
    Acquire {
        /// Output JSON path for selected issue state
        #[arg(long, default_value = ".tutti/state/auto/selected_issue.json")]
        output: std::path::PathBuf,

        /// GitHub issue label to filter
        #[arg(long, default_value = "agent-ops")]
        label: String,

        /// Optional GitHub milestone title to filter
        #[arg(long)]
        milestone: Option<String>,

        /// Lease time-to-live in seconds (default: 1800 = 30 min)
        #[arg(long)]
        lease_ttl_secs: Option<u64>,
    },
    /// Renew an active lease (call periodically during workflow)
    Heartbeat {
        /// Path to selected issue state JSON
        #[arg(long, default_value = ".tutti/state/auto/selected_issue.json")]
        state: std::path::PathBuf,
    },
    /// Release a claim when workflow completes or fails
    Release {
        /// Path to selected issue state JSON
        #[arg(long, default_value = ".tutti/state/auto/selected_issue.json")]
        state: std::path::PathBuf,

        /// Reason for releasing
        #[arg(long)]
        reason: Option<String>,
    },
    /// Sweep and release all stale (expired) claims
    Sweep,
}

#[derive(Subcommand)]
pub enum HandoffSubcommand {
    /// Generate a handoff packet for an agent
    Generate {
        /// Agent name
        agent: String,

        /// Trigger reason label (default: manual)
        #[arg(long)]
        reason: Option<String>,

        /// Explicit CTX percentage to include in packet
        #[arg(long)]
        ctx: Option<u8>,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Apply a handoff packet by sending it into the agent session
    Apply {
        /// Agent name
        agent: String,

        /// Packet path (default: latest packet for the agent)
        #[arg(long)]
        packet: Option<std::path::PathBuf>,
    },
    /// List handoff packets in this workspace
    List {
        /// Filter to a specific agent
        #[arg(long)]
        agent: Option<String>,

        /// Max packets to return (default: 20)
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum RemoteSubcommand {
    /// Open an SSH tunnel to a remote tutti host
    Attach {
        /// Remote host (e.g. user@host or hostname)
        host: String,

        /// Local/remote port for the tunnel (default: 4040)
        #[arg(short, long, default_value = "4040")]
        port: u16,

        /// Friendly name for this remote (default: host value)
        #[arg(long)]
        name: Option<String>,
    },
    /// Show registered remotes and their reachability
    Status,
}
