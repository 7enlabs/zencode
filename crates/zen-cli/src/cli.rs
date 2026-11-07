use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    #[value(name = "acceptEdits")]
    AcceptEdits,
    Plan,
    #[value(name = "bypassPermissions")]
    BypassPermissions,
    #[value(name = "dontAsk")]
    DontAsk,
}

impl PermissionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::DontAsk => "dontAsk",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" | "manual" | "auto" => PermissionMode::Default,
            "acceptEdits" => PermissionMode::AcceptEdits,
            "plan" => PermissionMode::Plan,
            "bypassPermissions" => PermissionMode::BypassPermissions,
            "dontAsk" => PermissionMode::DontAsk,
            _ => return None,
        })
    }
}

fn parse_tool_bundle(s: &str) -> Result<zen_agent::ToolBundle, String> {
    s.parse::<zen_agent::ToolBundle>()
}

fn parse_permission_mode_flag(s: &str) -> Result<PermissionMode, String> {
    if s == "auto" {
        eprintln!("warning: permission mode 'auto' is not supported; using 'default'");
    }
    PermissionMode::parse(s).ok_or_else(|| {
        format!(
            "invalid permission mode '{}' (expected one of: default, manual, acceptEdits, plan, \
             dontAsk, bypassPermissions)",
            s
        )
    })
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::DontAsk => "dontAsk",
        };
        write!(f, "{}", s)
    }
}

fn parse_autocompact(s: &str) -> Result<String, String> {
    match s {
        "auto" | "off" => Ok(s.to_string()),
        n if n.parse::<u64>().map(|v| v > 0).unwrap_or(false) => Ok(n.to_string()),
        _ => Err(format!(
            "invalid --autocompact '{}': use auto, off or a token count",
            s
        )),
    }
}

fn parse_session_uuid(s: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(s.trim())
        .map(|u| u.hyphenated().to_string())
        .map_err(|_| format!("invalid session id '{}': must be a valid UUID", s))
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "zen",
    version,
    about = "Run an AI coding agent in your terminal",
    after_help = "Examples:
  zen                             Start an interactive session
  zen -p \"fix the failing test\"   Run one prompt and print the result
  zen doctor                      Show configuration and environment diagnostics
  zen --output-format json doctor Print diagnostics as JSON"
)]
pub struct Cli {
    /// Run one prompt non-interactively and print the result
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// Serve the Agent Client Protocol (ACP) over stdio
    #[arg(long = "acp")]
    pub acp: bool,

    /// Format for results and reports
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,

    /// Emit JSON output (alias for --output-format json).
    // The field is named json_output, not json, so the clap arg id does not
    // collide with the json positional (a String) of the mcp add-json
    // subcommand now that this flag is global and reaches every subcommand.
    #[arg(long = "json", global = true)]
    pub json_output: bool,

    /// Silence informational output on stdout; errors still go to stderr.
    #[arg(long = "quiet", global = true)]
    pub quiet: bool,

    /// Emit compact one-line JSON output (the raw output mode).
    #[arg(long = "raw", global = true)]
    pub raw: bool,

    /// Disable colored output (same effect as the NO_COLOR env var).
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    /// Format for prompts read from stdin
    #[arg(long = "input-format", value_enum, default_value_t = InputFormat::Text)]
    pub input_format: InputFormat,

    /// Include partial message chunks in stream-json output
    #[arg(long = "include-partial-messages")]
    pub include_partial_messages: bool,

    /// Print verbose diagnostics
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Continue the most recent session
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// Resume a session by id; without an id, resume the latest
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Auto-compact context: auto, off, or a token count
    #[arg(long = "autocompact", value_parser = parse_autocompact)]
    pub autocompact: Option<String>,

    /// Prune older tool results from context
    #[arg(long = "prune", overrides_with = "no_prune")]
    pub prune: bool,

    /// Do not prune older tool results from context
    #[arg(long = "no-prune", overrides_with = "prune")]
    pub no_prune: bool,

    /// Keep this many recent turns before pruning older tool results
    #[arg(long = "prune-after")]
    pub prune_after: Option<usize>,

    /// Cap tokens kept for a single tool result
    #[arg(long = "max-result-tokens")]
    pub max_result_tokens: Option<u64>,

    /// Use this session id (UUID) for the run
    #[arg(long = "session-id", value_parser = parse_session_uuid)]
    pub session_id: Option<String>,

    /// Fork the resumed session instead of appending to it
    #[arg(long = "fork-session")]
    pub fork_session: bool,

    /// Do not save the session to disk
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,

    /// Ephemeral run: save nothing to disk
    #[arg(long)]
    pub ephemeral: bool,

    /// Model to use for this run
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Model for /btw side questions
    #[arg(long = "btw-model")]
    pub btw_model: Option<String>,

    /// Base URL of the model provider API
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Model provider to route requests through
    #[arg(long = "provider")]
    pub provider: Option<String>,

    /// Permission mode for tool calls
    #[arg(long = "permission-mode", value_parser = parse_permission_mode_flag)]
    pub permission_mode: Option<PermissionMode>,

    /// Only allow these tools (space-separated names)
    #[arg(long = "allowedTools", visible_alias = "allowed-tools", num_args = 1..)]
    pub allowed_tools: Vec<String>,

    /// Never allow these tools (space-separated names)
    #[arg(long = "disallowedTools", visible_alias = "disallowed-tools", num_args = 1..)]
    pub disallowed_tools: Vec<String>,

    /// Tool set for the run (bundle name or list)
    #[arg(long = "tools", allow_hyphen_values = true)]
    pub tools: Option<String>,

    /// Override the system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Append text to the system prompt
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,

    /// Add a working directory the agent may access
    #[arg(long = "add-dir", num_args = 1..)]
    pub add_dir: Vec<PathBuf>,

    /// Load MCP servers from this config file or JSON
    #[arg(long = "mcp-config", num_args = 1..)]
    pub mcp_config: Vec<String>,

    /// Only use MCP servers given via --mcp-config
    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,

    /// Load settings from this file or JSON
    #[arg(long = "settings")]
    pub settings: Option<String>,

    /// Settings sources to load (comma-separated)
    #[arg(long = "setting-sources")]
    pub setting_sources: Option<String>,

    /// Agent definitions as JSON
    #[arg(long = "agents")]
    pub agents: Option<String>,

    /// Run as the named agent
    #[arg(long = "agent")]
    pub agent: Option<String>,

    /// Stop after this many agent turns
    #[arg(long = "max-turns")]
    pub max_turns: Option<usize>,

    /// Stop when session cost passes this USD amount
    #[arg(long = "max-budget-usd")]
    pub max_budget_usd: Option<f64>,

    /// Warn when session cost passes this USD amount
    #[arg(long = "budget-warn-at")]
    pub budget_warn_at: Option<f64>,

    /// Verify command to run after turns; failures are retried
    #[arg(long = "until")]
    pub until: Option<String>,

    /// Retries allowed for a failing --until verify command
    #[arg(long = "max-verify-retries", default_value_t = 3)]
    pub max_verify_retries: usize,

    /// Start in plan mode (no edits until the plan is approved)
    #[arg(long = "plan")]
    pub plan: bool,

    /// Answer yes to confirmation prompts
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Bare mode: skip project config, agents and MCP discovery
    #[arg(long = "bare")]
    pub bare: bool,

    /// Ask the model for terse answers
    #[arg(long = "terse")]
    pub terse: bool,

    /// Cap tokens in each model response
    #[arg(long = "max-output-tokens")]
    pub max_output_tokens: Option<u32>,

    /// Token budget for the repository map
    #[arg(long = "repomap-budget", default_value_t = crate::repomap::DEFAULT_MAP_TOKENS)]
    pub map_tokens: usize,

    /// Trust this folder's project config for the session
    #[arg(long = "trust-folder")]
    pub trust_folder: bool,

    /// Skip safety confirmations for risky operations
    #[arg(long = "i-understand-the-risks")]
    pub i_understand_the_risks: bool,

    /// Restrict the run to a named tool bundle
    #[arg(long = "tool-bundle", value_parser = parse_tool_bundle)]
    pub tool_bundle: Option<zen_agent::ToolBundle>,

    /// Output style to apply to responses
    #[arg(long = "style", value_name = "STYLE")]
    pub style: Option<String>,

    /// Validate the final output against this JSON schema file
    #[arg(long = "json-schema", value_name = "FILE")]
    pub json_schema: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub subcommand: Option<Commands>,

    /// Prompt for the agent; omit to start an interactive session
    pub prompt: Vec<String>,
}

impl Cli {
    /// The output format to honor for this run: --json aliases JSON
    /// output, and --raw selects the compact one-line JSON form.
    pub fn effective_output_format(&self) -> OutputFormat {
        if self.json_output || self.raw {
            OutputFormat::Json
        } else {
            self.output_format
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Manage MCP server configuration
    Mcp(McpArgs),

    /// Manage skills: list, install, search, remove
    Skill(SkillArgs),

    /// Diagnose configuration, credentials and environment
    #[command(after_help = "Examples:\n  zen doctor\n  zen --json doctor")]
    Doctor,

    /// List available models
    Models(ModelsArgs),

    /// Show recorded cost for a session
    Cost(CostArgs),

    /// Show what fills a session's context window
    Context(ContextArgs),

    /// Show documentation for one tool
    #[command(name = "tool-help")]
    ToolHelp {
        /// Name of the tool, e.g. Read or Bash
        name: String,
    },

    /// Export a session to a file
    Export(ExportArgs),

    /// Import a session from an export file
    Import(ImportArgs),

    /// List, archive, delete or restore sessions
    Sessions(SessionsArgs),

    /// Inspect the P0-3 multi-seat task registry (observe-only)
    Task(TaskArgs),

    /// Run the built-in evaluation tasks
    Eval(EvalArgs),

    /// Review a diff and report findings
    Review(ReviewArgs),

    /// Show review comments on a GitHub pull request
    #[command(name = "pr-comments")]
    PrComments {
        /// Pull request number or URL (default: current branch)
        pr: Option<String>,
        // JSON output is the global --json flag on Cli (json_output).
    },

    /// Inspect CI check failures and suggest fixes
    #[command(name = "ci-fix")]
    CiFix {
        // JSON output is the global --json flag on Cli (json_output).
    },

    /// Check for and install zen updates
    Update(UpdateArgs),
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen update --check   Only check, do not install\n  zen update           Check and install the update"
)]
pub struct UpdateArgs {
    /// Only check for updates; do not install
    #[arg(long)]
    pub check: bool,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen models           List available models\n  zen models --best    Print only the best working model\n  zen models --refresh Refresh the list from the provider"
)]
pub struct ModelsArgs {
    /// Refresh the model list from the provider
    #[arg(long = "refresh")]
    pub refresh: bool,

    /// Print only the best working model
    #[arg(long = "best")]
    pub best: bool,

    /// Approve a model pool switch without prompting (default is to ask)
    #[arg(long = "yes")]
    pub yes: bool,

    /// Restore the model pool from its backup file and exit
    #[arg(long = "restore")]
    pub restore: bool,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen cost                      Cost of the latest session\n  zen cost --session abc123     Cost of one session\n  zen --json cost               Cost report as JSON"
)]
pub struct CostArgs {
    /// Session id (default: the latest session)
    #[arg(long = "session")]
    pub session: Option<String>,

    /// Format for the report
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen context                   Context of the latest session\n  zen context --session abc123  Context of one session"
)]
pub struct ContextArgs {
    /// Session id (default: the latest session)
    #[arg(long = "session")]
    pub session: Option<String>,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen export                                  Export the latest session as markdown\n  zen export --format json --output s.json    Export as JSON to a file\n  zen export --redact                         Redact secrets from the export"
)]
pub struct ExportArgs {
    /// Export format (default: markdown, or json when --json is given)
    #[arg(long, value_enum)]
    pub format: Option<crate::export::ExportFormat>,

    /// Session id (default: the latest session)
    #[arg(long)]
    pub session: Option<String>,

    /// Write to this file instead of stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Redact secrets from the export
    #[arg(long)]
    pub redact: bool,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen import session.md          Import a markdown export\n  zen import ./backup/s.json     Import a JSON export"
)]
pub struct ImportArgs {
    /// Export file to import
    pub file: PathBuf,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen sessions list              List sessions\n  zen sessions list --all        Include archived sessions\n  zen sessions archive abc123    Archive a session\n  zen sessions delete abc123 --yes  Delete without prompting\n  zen sessions restore abc123    Restore an archived session"
)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: SessionSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SessionSubcommand {
    /// List sessions
    List {
        /// Include archived sessions
        #[arg(long)]
        all: bool,
    },

    /// Archive a session
    Archive {
        /// Session id
        id: String,
    },

    /// Delete a session permanently
    Delete {
        /// Session id
        id: String,

        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Restore an archived session
    Restore {
        /// Session id
        id: String,
    },
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen task list                    List tasks in the registry\n  zen task show B-4                Show one task card\n  zen task claim B-4 --by zc-dev   Record a claim (observe-only)\n  zen task heartbeat B-4 --by zc-dev  Record a heartbeat (observe-only)"
)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum TaskSubcommand {
    /// List every task in the registry
    List,

    /// Show one task card
    Show {
        /// Task id, e.g. B-4
        id: String,
    },

    /// Record that a seat holds a task (observe-only: never blocks)
    Claim {
        /// Task id, e.g. B-4
        id: String,

        /// Seat recording the claim
        #[arg(long = "by")]
        by: String,
    },

    /// Record a heartbeat for a task (observe-only: renews the lease, never blocks)
    Heartbeat {
        /// Task id, e.g. B-4
        id: String,

        /// Seat sending the heartbeat
        #[arg(long = "by")]
        by: String,
    },
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen eval --mock            Run the built-in tasks against the scripted solver\n  zen eval --task fix-typo   Run one task\n  zen --json eval --mock     Eval report as JSON"
)]
pub struct EvalArgs {
    /// Model to evaluate (default: the configured model)
    #[arg(long)]
    pub model: Option<String>,

    /// Run only the task with this id
    #[arg(long)]
    pub task: Option<String>,

    // The report-as-JSON switch is the global --json flag on Cli
    // (json_output); eval no longer defines its own copy.
    /// Turn limit per task
    #[arg(long, default_value_t = 25)]
    pub max_turns: usize,

    /// Run every task against a built-in scripted solver instead of a live
    /// model (no API key, no network). Checks the harness, tools and checks.
    #[arg(long)]
    pub mock: bool,

    /// Exit non-zero when passed/total falls below this fraction (0.0 to 1.0).
    #[arg(long, default_value_t = 0.5)]
    pub min_pass_rate: f64,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen skill list             List installed skills\n  zen skill install pdf-tools  Install a skill\n  zen skill search pdf       Search the registry\n  zen skill remove pdf-tools   Remove a skill"
)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SkillSubcommand {
    /// List installed skills
    List,

    /// Install a skill
    Install {
        /// Skill source: registry name, URL or path
        source: String,

        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Registry URL to use
        #[arg(long)]
        registry: Option<String>,
    },

    /// Search skills
    Search {
        /// Search text
        query: String,

        /// Registry URL to use
        #[arg(long)]
        registry: Option<String>,

        /// Only search the curated registry
        #[arg(long)]
        curated: bool,
    },

    /// Show details for one skill
    Info {
        /// Skill name
        name: String,

        /// Registry URL to use
        #[arg(long)]
        registry: Option<String>,
    },

    /// Remove an installed skill
    Remove {
        /// Skill name
        name: String,
    },

    /// Run the tests of a skill
    Test {
        /// Skill name
        name: String,
    },

    /// Scaffold a new skill
    Create {
        /// Skill name
        name: String,
    },
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Examples:\n  zen mcp list               List configured servers\n  zen mcp get my-server      Show one server\n  zen mcp add my-server -- npx my-mcp-server  Add a stdio server\n  zen mcp remove my-server   Remove a server"
)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum McpSubcommand {
    /// List configured MCP servers
    List,

    /// Add an MCP server
    Add {
        /// Transport type: stdio, sse or http
        #[arg(long = "transport", default_value = "stdio")]
        transport: String,

        /// Config scope: local, project or user
        #[arg(short = 's', long = "scope", default_value = "local")]
        scope: String,

        /// Environment variable for the server (KEY=VALUE)
        #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// HTTP header for the server (KEY=VALUE)
        #[arg(long = "header", value_name = "KEY=VALUE")]
        header: Vec<String>,

        /// Server name
        name: String,

        /// Server command and its arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Remove an MCP server
    Remove {
        /// Config scope: local, project or user
        #[arg(short = 's', long = "scope", default_value = "local")]
        scope: String,

        /// Server name
        name: String,
    },

    /// Show one MCP server
    Get {
        /// Config scope: local, project or user
        #[arg(short = 's', long = "scope", default_value = "")]
        scope: String,

        /// Server name
        name: String,
    },

    /// Add an MCP server from a JSON definition
    AddJson {
        /// Config scope: local, project or user
        #[arg(short = 's', long = "scope", default_value = "local")]
        scope: String,

        /// Server name
        name: String,

        /// Server configuration as JSON
        json: String,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ReviewArgs {
    /// Review uncommitted changes
    #[arg(long)]
    pub uncommitted: bool,

    /// Diff against this ref
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,

    /// Review this single commit
    #[arg(long, value_name = "SHA")]
    pub commit: Option<String>,

    /// Lowest severity that makes the run fail
    #[arg(long, default_value = "high")]
    pub fail_on: String,

    /// Format for the review report
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,

    #[arg(long, default_value_t = 100_000)]
    pub max_diff_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn subcommand_help_includes_examples() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        for name in [
            "doctor", "models", "cost", "context", "sessions", "eval", "update", "mcp", "skill",
            "export", "import",
        ] {
            let sub = cmd
                .get_subcommands()
                .find(|c| c.get_name() == name)
                .unwrap_or_else(|| panic!("missing subcommand {name}"));
            let help = sub.clone().render_long_help().to_string();
            assert!(
                help.contains("Examples:"),
                "{name} help has no Examples section:\n{help}"
            );
        }
    }

    fn parse(args: &[&str]) -> Cli {
        let mut base = vec!["zen"];
        base.extend_from_slice(args);
        Cli::parse_from(base)
    }

    #[test]
    fn parse_export_subcommand() {
        let cli = parse(&["export", "--format", "json", "--session", "abc", "--redact"]);
        match cli.subcommand {
            Some(Commands::Export(e)) => {
                assert_eq!(e.format, Some(crate::export::ExportFormat::Json));
                assert_eq!(e.session.as_deref(), Some("abc"));
                assert!(e.redact);
                assert!(e.output.is_none());
            }
            other => panic!("expected Export, got {:?}", other.map(|_| ())),
        }
        let cli = parse(&["export", "-o", "s.md"]);
        match cli.subcommand {
            Some(Commands::Export(e)) => {
                assert_eq!(e.format, None);
                assert_eq!(e.output, Some(PathBuf::from("s.md")));
            }
            _ => panic!("expected Export"),
        }
        let cli = parse(&["import", "sess.json"]);
        match cli.subcommand {
            Some(Commands::Import(i)) => assert_eq!(i.file, PathBuf::from("sess.json")),
            _ => panic!("expected Import"),
        }
    }

    #[test]
    fn parse_print_and_prompt() {
        let cli = parse(&["-p", "hello", "world"]);
        assert!(cli.print);
        assert_eq!(cli.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn parse_quiet_raw_json_flags() {
        let cli = parse(&["--quiet", "doctor"]);
        assert!(cli.quiet);
        let cli = parse(&["--raw", "doctor"]);
        assert!(cli.raw);
        assert_eq!(cli.effective_output_format(), OutputFormat::Json);
        let cli = parse(&["--no-color", "doctor"]);
        assert!(cli.no_color);
        let cli = parse(&["--json", "doctor"]);
        assert!(cli.json_output);
        assert_eq!(cli.effective_output_format(), OutputFormat::Json);
        let cli = parse(&["doctor"]);
        assert_eq!(cli.effective_output_format(), OutputFormat::Text);
    }

    #[test]
    fn parse_global_flags_after_subcommand() {
        // --json/--quiet/--raw are global: they must also parse when placed
        // after the subcommand (zen doctor --json), not only before it.
        let cli = parse(&["doctor", "--json"]);
        assert!(cli.json_output);
        assert_eq!(cli.effective_output_format(), OutputFormat::Json);
        let cli = parse(&["doctor", "--quiet"]);
        assert!(cli.quiet);
        let cli = parse(&["doctor", "--raw"]);
        assert!(cli.raw);
        let cli = parse(&["cost", "--json"]);
        assert!(cli.json_output);
    }

    #[test]
    fn parse_output_format() {
        let cli = parse(&["-p", "--output-format", "json", "hi"]);
        assert_eq!(cli.output_format, OutputFormat::Json);
    }

    #[test]
    fn parse_output_format_stream_json() {
        let cli = parse(&["-p", "--output-format", "stream-json", "hi"]);
        assert_eq!(cli.output_format, OutputFormat::StreamJson);
    }

    #[test]
    fn parse_permission_mode() {
        let cli = parse(&["--permission-mode", "plan"]);
        assert_eq!(cli.permission_mode, Some(PermissionMode::Plan));
    }

    #[test]
    fn c2_11_permission_mode_aliases() {
        assert_eq!(
            parse(&["--permission-mode", "manual"]).permission_mode,
            Some(PermissionMode::Default)
        );
        assert_eq!(
            parse(&["--permission-mode", "auto"]).permission_mode,
            Some(PermissionMode::Default)
        );
        assert_eq!(
            parse(&["--permission-mode", "dontAsk"]).permission_mode,
            Some(PermissionMode::DontAsk)
        );
        let err = Cli::try_parse_from(["zen", "--permission-mode", "yolo"]).unwrap_err();
        assert!(
            err.to_string().contains("invalid permission mode 'yolo'"),
            "{}",
            err
        );
    }

    #[test]
    fn parse_tools_empty() {
        let cli = parse(&["--tools", ""]);
        assert_eq!(cli.tools.as_deref(), Some(""));
    }

    #[test]
    fn parse_tools_default() {
        let cli = parse(&["--tools", "default"]);
        assert_eq!(cli.tools.as_deref(), Some("default"));
    }

    #[test]
    fn parse_tools_list() {
        let cli = parse(&["--tools", "Read,Bash"]);
        assert_eq!(cli.tools.as_deref(), Some("Read,Bash"));
        let cli = parse(&["--tools", "-Bash,-Write", "-p", "hi"]);
        assert_eq!(cli.tools.as_deref(), Some("-Bash,-Write"));
        assert!(cli.print);
        assert_eq!(cli.prompt, vec!["hi".to_string()]);
    }

    #[test]
    fn parse_mcp_subcommand_list() {
        let cli = parse(&["mcp", "list"]);
        assert!(matches!(cli.subcommand, Some(Commands::Mcp(_))));
    }

    #[test]
    fn parse_mcp_subcommand_remove() {
        let cli = parse(&["mcp", "remove", "-s", "user", "foo"]);
        match &cli.subcommand {
            Some(Commands::Mcp(m)) => {
                assert!(matches!(m.command, McpSubcommand::Remove { .. }));
            }
            _ => panic!("expected Mcp Remove"),
        }
    }

    #[test]
    fn parse_mcp_subcommand_add_http() {
        let cli = parse(&[
            "mcp",
            "add",
            "--transport",
            "http",
            "-s",
            "project",
            "myserver",
            "http://localhost:3000",
        ]);
        match &cli.subcommand {
            Some(Commands::Mcp(m)) => {
                assert!(
                    matches!(&m.command, McpSubcommand::Add{transport,..} if transport == "http")
                );
            }
            _ => panic!("expected Mcp Add"),
        }
    }

    #[test]
    fn parse_update_subcommand() {
        let cli = parse(&["update"]);
        match &cli.subcommand {
            Some(Commands::Update(args)) => {
                assert!(!args.check);
            }
            _ => panic!("expected Update"),
        }
        let cli = parse(&["update", "--check"]);
        match &cli.subcommand {
            Some(Commands::Update(args)) => {
                assert!(args.check);
            }
            _ => panic!("expected Update with check"),
        }
    }

    #[test]
    fn parse_doctor_subcommand() {
        let cli = parse(&["doctor"]);
        assert!(matches!(cli.subcommand, Some(Commands::Doctor)));
    }

    #[test]
    fn parse_models_subcommand() {
        let cli = parse(&["models"]);
        match &cli.subcommand {
            Some(Commands::Models(m)) => {
                assert!(!m.refresh);
                assert!(!m.best);
            }
            _ => panic!("expected Models"),
        }
        let cli = parse(&["models", "--refresh", "--best"]);
        match &cli.subcommand {
            Some(Commands::Models(m)) => {
                assert!(m.refresh);
                assert!(m.best);
            }
            _ => panic!("expected Models with flags"),
        }
    }

    #[test]
    fn parse_sessions_subcommands() {
        let cli = parse(&["sessions", "list"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Sessions(s)) if matches!(s.command, SessionSubcommand::List { all: false })
        ));
        let cli = parse(&["sessions", "list", "--all"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Sessions(s)) if matches!(s.command, SessionSubcommand::List { all: true })
        ));
        let cli = parse(&["sessions", "archive", "abc123"]);
        match cli.subcommand {
            Some(Commands::Sessions(s)) => match s.command {
                SessionSubcommand::Archive { id } => assert_eq!(id, "abc123"),
                other => panic!("expected Archive, got {:?}", other),
            },
            _ => panic!("expected Sessions"),
        }
        let cli = parse(&["sessions", "delete", "abc123", "--yes"]);
        match cli.subcommand {
            Some(Commands::Sessions(s)) => match s.command {
                SessionSubcommand::Delete { id, yes } => {
                    assert_eq!(id, "abc123");
                    assert!(yes);
                }
                other => panic!("expected Delete, got {:?}", other),
            },
            _ => panic!("expected Sessions"),
        }
        let cli = parse(&["sessions", "restore", "abc123"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Sessions(s)) if matches!(s.command, SessionSubcommand::Restore { .. })
        ));
    }

    #[test]
    fn parse_task_subcommands() {
        let cli = parse(&["task", "list"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Task(t)) if matches!(t.command, TaskSubcommand::List)
        ));
        let cli = parse(&["task", "show", "B-4"]);
        match cli.subcommand {
            Some(Commands::Task(t)) => match t.command {
                TaskSubcommand::Show { id } => assert_eq!(id, "B-4"),
                other => panic!("expected Show, got {:?}", other),
            },
            _ => panic!("expected Task"),
        }
        let cli = parse(&["task", "claim", "B-4", "--by", "zc-developer"]);
        match cli.subcommand {
            Some(Commands::Task(t)) => match t.command {
                TaskSubcommand::Claim { id, by } => {
                    assert_eq!(id, "B-4");
                    assert_eq!(by, "zc-developer");
                }
                other => panic!("expected Claim, got {:?}", other),
            },
            _ => panic!("expected Task"),
        }
        let cli = parse(&["task", "heartbeat", "B-4", "--by", "zc-developer"]);
        match cli.subcommand {
            Some(Commands::Task(t)) => match t.command {
                TaskSubcommand::Heartbeat { id, by } => {
                    assert_eq!(id, "B-4");
                    assert_eq!(by, "zc-developer");
                }
                other => panic!("expected Heartbeat, got {:?}", other),
            },
            _ => panic!("expected Task"),
        }
    }

    #[test]
    fn parse_skill_subcommands() {
        let cli = parse(&["skill", "list"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::List)
        ));
        let cli = parse(&["skill", "install", "builtin:tdd", "--yes"]);
        match cli.subcommand {
            Some(Commands::Skill(s)) => match s.command {
                SkillSubcommand::Install {
                    source,
                    yes,
                    registry,
                } => {
                    assert_eq!(source, "builtin:tdd");
                    assert!(yes);
                    assert!(registry.is_none());
                }
                other => panic!("expected Install, got {:?}", other),
            },
            _ => panic!("expected Skill"),
        }
        let cli = parse(&["skill", "test", "tdd"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Test{..})
        ));
        let cli = parse(&["skill", "remove", "tdd"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Remove{..})
        ));
        let cli = parse(&["skill", "create", "my-skill"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Create{..})
        ));
        let cli = parse(&["skill", "search", "tdd"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Search{..})
        ));
        let cli = parse(&["skill", "search", "tdd", "--curated"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Search { curated: true, .. })
        ));
        let cli = parse(&["skill", "info", "tdd"]);
        assert!(matches!(
            cli.subcommand,
            Some(Commands::Skill(s)) if matches!(s.command, SkillSubcommand::Info{..})
        ));
        let cli = parse(&["skill", "install", "registry:tdd", "--yes"]);
        match cli.subcommand {
            Some(Commands::Skill(s)) => match s.command {
                SkillSubcommand::Install { source, yes, .. } => {
                    assert_eq!(source, "registry:tdd");
                    assert!(yes);
                }
                other => panic!("expected Install, got {other:?}"),
            },
            _ => panic!("expected Skill"),
        }
    }

    #[test]
    fn parse_flag_bare() {
        let cli = parse(&["--bare"]);
        assert!(cli.bare);
    }

    #[test]
    fn parse_flag_terse() {
        let cli = parse(&["--terse"]);
        assert!(cli.terse);
        let cli = parse(&[]);
        assert!(!cli.terse);
    }

    #[test]
    fn parse_max_output_tokens() {
        let cli = parse(&["--max-output-tokens", "512"]);
        assert_eq!(cli.max_output_tokens, Some(512));
        let cli = parse(&[]);
        assert_eq!(cli.max_output_tokens, None);
    }

    #[test]
    fn parse_no_args() {
        let cli = parse(&[]);
        assert!(!cli.print);
        assert!(cli.prompt.is_empty());
        assert!(cli.subcommand.is_none());
    }

    #[test]
    fn parse_setting_sources() {
        let cli = parse(&["--setting-sources", "user,project"]);
        assert_eq!(cli.setting_sources.as_deref(), Some("user,project"));
    }

    #[test]
    fn parse_resume_with_value() {
        let cli = parse(&["--resume", "abc"]);
        assert_eq!(cli.resume.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_resume_no_value() {
        let cli = parse(&["--resume"]);
        assert_eq!(cli.resume.as_deref(), Some(""));
    }

    #[test]
    fn parse_fork_session() {
        let cli = parse(&["--fork-session"]);
        assert!(cli.fork_session);
    }

    #[test]
    fn parse_prune_flags() {
        let cli = parse(&[]);
        assert!(!cli.prune);
        assert!(!cli.no_prune);
        assert_eq!(cli.prune_after, None);
        assert_eq!(cli.max_result_tokens, None);

        let cli = parse(&["--prune-after", "3", "--max-result-tokens", "4000"]);
        assert_eq!(cli.prune_after, Some(3));
        assert_eq!(cli.max_result_tokens, Some(4000));

        let cli = parse(&["--prune", "--no-prune"]);
        assert!(cli.no_prune);
        assert!(!cli.prune);
        let cli = parse(&["--no-prune", "--prune"]);
        assert!(cli.prune);
        assert!(!cli.no_prune);
    }

    #[test]
    fn parse_ephemeral() {
        let cli = parse(&["--ephemeral"]);
        assert!(cli.ephemeral);

        let cli = parse(&[]);
        assert!(!cli.ephemeral);
    }

    #[test]
    fn parse_session_id() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let cli = parse(&["--session-id", id]);
        assert_eq!(cli.session_id.as_deref(), Some(id));
    }

    #[test]
    fn c9_2_session_id_must_be_uuid() {
        for bad in ["sess-1", "", "123e4567-e89b-12d3-a456", "not a uuid"] {
            let r = Cli::try_parse_from(["zen", "--session-id", bad]);
            assert!(r.is_err(), "{:?} should be rejected", bad);
        }

        let cli = parse(&["--session-id", "123E4567E89B12D3A456426614174000"]);
        assert_eq!(
            cli.session_id.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174000")
        );
    }

    #[test]
    fn parse_max_turns() {
        let cli = parse(&["--max-turns", "10"]);
        assert_eq!(cli.max_turns, Some(10));
    }

    #[test]
    fn parse_max_budget_usd() {
        let cli = parse(&["--max-budget-usd", "2.50"]);
        assert_eq!(cli.max_budget_usd, Some(2.50));
        assert_eq!(cli.budget_warn_at, None);
        let cli = parse(&["--max-budget-usd", "2.50", "--budget-warn-at", "0.5"]);
        assert_eq!(cli.budget_warn_at, Some(0.5));

        let cli = parse(&[] as &[&str]);
        assert_eq!(cli.max_budget_usd, None);
    }

    #[test]
    fn parse_until_and_max_verify_retries() {
        let cli = parse(&["-p", "--until", "cargo test -q", "go"]);
        assert_eq!(cli.until.as_deref(), Some("cargo test -q"));

        assert_eq!(cli.max_verify_retries, 3);
        let cli = parse(&[
            "-p",
            "--until",
            "make check",
            "--max-verify-retries",
            "0",
            "go",
        ]);
        assert_eq!(cli.max_verify_retries, 0);
    }

    #[test]
    fn parse_plan_and_yes() {
        let cli = parse(&["-p", "--plan", "go"]);
        assert!(cli.plan);
        assert!(!cli.yes);
        let cli = parse(&["-p", "--plan", "--yes", "go"]);
        assert!(cli.plan && cli.yes);
        let cli = parse(&["-p", "--plan", "-y", "go"]);
        assert!(cli.yes);
    }

    #[test]
    fn flags_after_prompt_are_parsed() {
        let cli = parse(&[
            "-p",
            "list files",
            "--output-format",
            "json",
            "--model",
            "m:free",
        ]);
        assert!(cli.print);
        assert_eq!(cli.prompt, vec!["list files".to_string()]);
        assert!(matches!(cli.output_format, OutputFormat::Json));
        assert_eq!(cli.model.as_deref(), Some("m:free"));
    }

    #[test]
    fn parse_model_flag() {
        let cli = parse(&["--model", "gpt-4"]);
        assert_eq!(cli.model.as_deref(), Some("gpt-4"));
    }

    #[test]
    fn parse_base_url_flag() {
        let cli = parse(&["--base-url", "https://x.com/v1"]);
        assert_eq!(cli.base_url.as_deref(), Some("https://x.com/v1"));
    }

    #[test]
    fn parse_add_dir() {
        let cli = parse(&["--add-dir", "/a", "/b"]);
        assert_eq!(cli.add_dir.len(), 2);
    }

    #[test]
    fn parse_allowed_disallowed() {
        let cli = parse(&[
            "--allowedTools",
            "Read",
            "Bash(git status)",
            "--disallowedTools",
            "Write",
        ]);
        assert_eq!(cli.allowed_tools, vec!["Read", "Bash(git status)"]);
        assert_eq!(cli.disallowed_tools, vec!["Write"]);
    }

    #[test]
    fn parse_bare_skips_settings_flag() {
        let cli = parse(&["--bare", "-p", "hi"]);
        assert!(cli.bare);
        assert!(cli.print);
    }

    #[test]
    fn parse_agent_flag() {
        let cli = parse(&["-p", "--agent", "researcher", "go"]);
        assert_eq!(cli.agent.as_deref(), Some("researcher"));
    }

    #[test]
    fn parse_mcp_get_with_scope() {
        let cli = parse(&["mcp", "get", "-s", "user", "srv"]);
        match &cli.subcommand {
            Some(Commands::Mcp(m)) => match &m.command {
                McpSubcommand::Get { scope, name } => {
                    assert_eq!(scope, "user");
                    assert_eq!(name, "srv");
                }
                other => panic!("expected Get, got {:?}", other),
            },
            other => panic!("expected Mcp, got {:?}", other),
        }
    }

    #[test]
    fn parse_mcp_add_json() {
        let cli = parse(&["mcp", "add-json", "srv", r#"{"command":"x"}"#]);
        match &cli.subcommand {
            Some(Commands::Mcp(m)) => match &m.command {
                McpSubcommand::AddJson { name, json, .. } => {
                    assert_eq!(name, "srv");
                    assert!(json.contains("command"));
                }
                other => panic!("expected AddJson, got {:?}", other),
            },
            other => panic!("expected Mcp, got {:?}", other),
        }
    }

    #[test]
    fn parse_mcp_add_env_and_header() {
        let cli = parse(&[
            "mcp",
            "add",
            "--transport",
            "http",
            "-e",
            "K=V",
            "--header",
            "Authorization=Bearer x",
            "srv",
            "http://localhost:1",
        ]);
        match &cli.subcommand {
            Some(Commands::Mcp(m)) => match &m.command {
                McpSubcommand::Add {
                    env, header, name, ..
                } => {
                    assert_eq!(env, &vec!["K=V".to_string()]);
                    assert_eq!(header, &vec!["Authorization=Bearer x".to_string()]);
                    assert_eq!(name, "srv");
                }
                other => panic!("expected Add, got {:?}", other),
            },
            other => panic!("expected Mcp, got {:?}", other),
        }
    }

    #[test]
    fn parse_provider_flag() {
        let cli = parse(&["--provider", "ollama"]);
        assert_eq!(cli.provider.as_deref(), Some("ollama"));
        let cli = parse(&[]);
        assert_eq!(cli.provider, None);
    }

    #[test]
    fn parse_eval_subcommand() {
        let cli = parse(&["eval", "--model", "x:free", "--task", "create", "--json"]);
        match &cli.subcommand {
            Some(Commands::Eval(a)) => {
                assert_eq!(a.model.as_deref(), Some("x:free"));
                assert_eq!(a.task.as_deref(), Some("create"));
                assert!(cli.json_output);
                assert_eq!(a.max_turns, 25);
            }
            other => panic!("expected Eval, got {:?}", other),
        }
    }

    #[test]
    fn parse_eval_max_turns() {
        let cli = parse(&["eval", "--max-turns", "10"]);
        match &cli.subcommand {
            Some(Commands::Eval(a)) => assert_eq!(a.max_turns, 10),
            other => panic!("expected Eval, got {:?}", other),
        }
    }

    #[test]
    fn parse_eval_mock_and_threshold() {
        let cli = parse(&["eval", "--mock", "--min-pass-rate", "1.0"]);
        match &cli.subcommand {
            Some(Commands::Eval(a)) => {
                assert!(a.mock);
                assert_eq!(a.min_pass_rate, 1.0);
            }
            other => panic!("expected Eval, got {:?}", other),
        }
        let cli = parse(&["eval"]);
        match &cli.subcommand {
            Some(Commands::Eval(a)) => {
                assert!(!a.mock);
                assert_eq!(a.min_pass_rate, 0.5);
            }
            other => panic!("expected Eval, got {:?}", other),
        }
    }
}
