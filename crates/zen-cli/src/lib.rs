pub mod acp;
pub mod agent_run;
pub mod btw;
pub mod budget;
pub mod cli;
pub mod commands;
pub mod context;
pub mod cost;
pub mod curation;
pub mod deep_research;
pub mod doneguard;
pub mod envsnap;
pub mod eval;
pub mod eventlog;
pub mod export;
pub mod git;
pub mod github;
pub mod mentions;
pub mod models;
pub mod ollama;
pub mod output;
pub mod planmode;
pub mod prompt_budget;
pub mod repl;
pub mod repl_input;
pub mod repl_polish;
pub mod repl_progress;
pub mod repomap;
pub mod review;
pub mod safety;
pub mod secret;
pub mod session;
pub mod session_mgmt;
pub mod settings;
pub mod skill_registry;
pub mod skills;
pub mod system_prompt;
pub mod tasks;
pub mod telemetry;
pub mod update;

pub use cli::{
    Cli, Commands, CostArgs, EvalArgs, ExportArgs, ImportArgs, InputFormat, McpArgs, McpSubcommand,
    ModelsArgs, OutputFormat, PermissionMode, ReviewArgs, SessionSubcommand, SessionsArgs,
    UpdateArgs,
};
pub use settings::{ResolvedConfig, resolve_config};
pub use system_prompt::build_system_prompt;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Resolve the user's home directory, honoring explicit overrides.
///
/// dirs::home_dir() on Windows returns the profile known folder and
/// ignores HOME/USERPROFILE, so pointing the app at a different home
/// (tests, portable installs, CI) is silently ignored there and sessions,
/// settings, trust state, and cost ledgers leak into the real profile.
/// Honor HOME first, then USERPROFILE on Windows, before falling back.
fn resolve_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    if cfg!(windows)
        && let Some(h) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty())
    {
        return Some(PathBuf::from(h));
    }
    dirs::home_dir()
}

/// Exit codes from the B2 table (docs/exit-codes.md). Named so call
/// sites read as the table row they implement.
pub mod exit_code {
    pub const OK: i32 = 0;
    pub const GENERAL: i32 = 1;
    pub const AUTH: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const VALIDATION: i32 = 4;
    pub const FORBIDDEN: i32 = 5;
    pub const RATE_LIMIT: i32 = 6;
    pub const CONFLICT: i32 = 7;
}

/// An error that carries its own process exit code from the B2 table.
///
/// Migration queue Q5 (docs/exit-codes.md): new error paths pick their
/// table code at the source with this type instead of relying on the
/// blanket exit 1 in main. `main` reads the code back through
/// [`exit_code_for_error`].
#[derive(Debug)]
pub struct CodedError {
    code: i32,
    message: String,
}

impl CodedError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CodedError {}

/// Build an anyhow error carrying a B2 exit code.
pub fn coded_error(code: i32, message: impl Into<String>) -> anyhow::Error {
    CodedError::new(code, message).into()
}

/// The exit code `main` should use for an error returned from `run`:
/// a [`CodedError`] keeps the code it picked at the source; an
/// auth-shaped failure maps to AUTH (queue Q2: provider 401s and
/// missing credentials surface as anyhow errors from the agent layer);
/// provider status shapes map to their table codes (429 rate limit,
/// 403 forbidden, 404 not found - the agent client formats provider
/// failures as `HTTP <status>: <body>`, and the review path already
/// splits 429 out the same way); everything else stays at the blanket
/// GENERAL fallback.
pub fn exit_code_for_error(err: &anyhow::Error) -> i32 {
    if let Some(coded) = err.downcast_ref::<CodedError>() {
        return coded.code();
    }
    let text = format!("{err:#}").to_lowercase();
    if text.contains("401")
        || text.contains("unauthorized")
        || text.contains("no auth credentials")
        || text.contains("invalid api key")
        || text.contains("authentication failed")
    {
        return exit_code::AUTH;
    }
    if text.contains("429")
        || text.contains("rate limit")
        || text.contains("rate_limit")
        || text.contains("too many requests")
    {
        return exit_code::RATE_LIMIT;
    }
    if text.contains("403") || text.contains("forbidden") {
        return exit_code::FORBIDDEN;
    }
    if text.contains("404") || text.contains("not found") {
        return exit_code::NOT_FOUND;
    }
    if text.contains("409") {
        return exit_code::CONFLICT;
    }
    exit_code::GENERAL
}

/// Map an HTTP status to its B2 table code (target table,
/// docs/exit-codes.md): 401 auth, 403 forbidden, 404 not found,
/// 409 conflict, 429 rate limit; anything else, including 5xx,
/// is general.
pub fn exit_code_for_http_status(status: u16) -> i32 {
    match status {
        401 => exit_code::AUTH,
        403 => exit_code::FORBIDDEN,
        404 => exit_code::NOT_FOUND,
        409 => exit_code::CONFLICT,
        429 => exit_code::RATE_LIMIT,
        _ => exit_code::GENERAL,
    }
}

/// Find the first `HTTP <status>` shape in free text, if any. The
/// agent client formats provider failures as `HTTP <status>: <body>`
/// (zen-agent client), and that text can carry a request id or extra
/// context around it, so match the shape anywhere instead of assuming
/// it is a prefix. Only a three-digit status counts, so a bare
/// number or a longer run of digits never matches.
pub fn http_status_in_text(text: &str) -> Option<u16> {
    let lower = text.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(idx) = rest.find("http ") {
        let after = &rest[idx + "http ".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() == 3
            && let Ok(status) = digits.parse::<u16>()
        {
            return Some(status);
        }
        rest = after;
    }
    None
}

/// Exit code for a finished headless (`-p`) turn (issue #165): the
/// agent reports provider failures as a `result/error_during_execution`
/// event whose text is `HTTP <status>: ...` instead of propagating an
/// error, so `exit_code_for_error` never sees them. Map the status
/// from the result text to the B2 table; an error without an HTTP
/// status keeps the blanket general code, and a successful turn
/// stays 0.
pub fn exit_code_for_headless_result(is_error: bool, result_text: &str) -> i32 {
    if !is_error {
        return exit_code::OK;
    }
    http_status_in_text(result_text)
        .map(exit_code_for_http_status)
        .unwrap_or(exit_code::GENERAL)
}

pub async fn run(cli: Cli) -> Result<i32> {
    let cwd = std::env::current_dir()?;
    let home = resolve_home();
    run_with_home(cli, &cwd, home.as_deref()).await
}

pub async fn run_with_home(cli: Cli, cwd: &Path, home: Option<&Path>) -> Result<i32> {
    if cli.no_color {
        // --no-color is the flag form of NO_COLOR: the color checks in
        // the REPL and the diff paths already honor the env var.
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }
    if cli.acp {
        return acp::run_acp_server(&cli).await;
    }

    if let Some(sub) = cli.subcommand.clone() {
        return run_subcommand(&cli, cwd, home, &sub).await;
    }

    if cli.print {
        return run_headless(&cli, cwd, home).await;
    }

    // A lone word in prompt position is far more often a mistyped command
    // than a real prompt: fail fast and teach the fix instead of feeding
    // the word to the REPL as a first turn (design audit, section 4.7).
    if cli.prompt.len() == 1 && !cli.continue_session && cli.resume.is_none() {
        let word = cli.prompt[0].as_str();
        let command_like = word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if command_like {
            eprintln!("error: unrecognized command '{}'", word);
            eprintln!();
            eprintln!(
                "  tip: to send '{}' as a prompt instead, run: zen -p \"{}\"",
                word, word
            );
            eprintln!();
            eprintln!("Usage: zen [OPTIONS] [PROMPT]... [COMMAND]");
            eprintln!();
            eprintln!("Run `zen --help` for the list of commands.");
            return Ok(exit_code::VALIDATION);
        }
    }

    // A bare REPL on a non-TTY stdin is deliberately not blocked here:
    // piping scripted lines into the REPL until EOF is a supported,
    // tested mode (crates/zen-cli/tests/builtins.rs). The audit hang came
    // from the unknown-command case above, which now fails fast first.

    ask_folder_trust(&cli, cwd, home);
    let cwd_buf = cwd.to_path_buf();
    repl::run_repl(&cli, &cwd_buf, home).await?;
    Ok(0)
}

async fn run_subcommand(cli: &Cli, cwd: &Path, home: Option<&Path>, sub: &Commands) -> Result<i32> {
    match sub {
        Commands::Doctor => {
            let cfg = settings::resolve_config_with_home(cli, cwd, home)?;
            let store = home.map(zen_agent::RateGate::default_store_path);
            let quota = zen_agent::rategate::quota_report_for(store.as_deref());
            match cli.effective_output_format() {
                OutputFormat::Json | OutputFormat::StreamJson => {
                    if cli.quiet {
                        return Ok(0);
                    }
                    let report = serde_json::json!({
                        "type": "doctor",
                        "version": env!("CARGO_PKG_VERSION"),
                        "revision": env!("ZEN_GIT_SHA"),
                        "model": cfg.model,
                        "base_url": cfg.base_url,
                        "api_key": if cfg.api_key().is_some() {
                            "configured (hidden)"
                        } else {
                            "not set"
                        },
                        "ollama": cfg.ollama,
                        "permission_mode": cfg.permission_mode.to_string(),
                        "folder_trusted": cfg.folder_trusted,
                        "trust_skipped": cfg.trust_skipped,
                        "quota": quota.lines().collect::<Vec<_>>(),
                        "settings_sources": cfg.loaded_sources,
                        "warnings": cfg.warnings,
                        "safety_checklist": safety::doctor_safety_checklist(
                            env!("CARGO_PKG_VERSION")
                        ),
                    });
                    println!("{}", serde_json::to_string(&report)?);
                }
                OutputFormat::Text => {
                    if cli.quiet {
                        return Ok(0);
                    }
                    println!("zen doctor");
                    println!("  model: {}", cfg.model);
                    println!("  base_url: {}", cfg.base_url);
                    if cfg.ollama {
                        println!("  provider: ollama (local)");
                    }
                    println!(
                        "  api_key: {}",
                        if cfg.api_key().is_some() {
                            "configured (hidden)"
                        } else {
                            "not set"
                        }
                    );
                    println!("  permission_mode: {}", cfg.permission_mode);
                    println!("  folder_trusted: {}", cfg.folder_trusted);
                    for s in &cfg.trust_skipped {
                        println!("    skipped: {}", s);
                    }
                    for line in quota.lines() {
                        println!("  {line}");
                    }
                    if cfg.loaded_sources.is_empty() {
                        println!("  settings_sources: (none)");
                    } else {
                        println!("  settings_sources:");
                        for s in &cfg.loaded_sources {
                            println!("    - {}", s);
                        }
                    }
                    if !cfg.warnings.is_empty() {
                        println!("  warnings:");
                        for w in &cfg.warnings {
                            println!("    - {}", w);
                        }
                    }
                    print!(
                        "{}",
                        safety::doctor_safety_checklist(env!("CARGO_PKG_VERSION"))
                    );
                }
            }
            Ok(0)
        }
        Commands::Mcp(mcp) => {
            commands::mcp_command(cli, cwd, home, mcp).await?;
            Ok(0)
        }
        Commands::Models(args) => {
            models::models_command(cli, cwd, home, args).await?;
            Ok(0)
        }
        Commands::Cost(args) => {
            cost_subcommand(cli, cwd, home, args)?;
            Ok(0)
        }
        Commands::Context(args) => {
            context::run_context(
                args.session.as_deref(),
                &cli.effective_output_format(),
                cli.quiet,
                cli.raw,
                cwd,
                home,
            )?;
            Ok(0)
        }
        Commands::ToolHelp { name } => {
            let text = commands::tool_help(name);
            // tool_help renders an unknown name as "Unknown tool '<name>'.",
            // which is an error: it belongs on stderr with a non-zero exit,
            // not on stdout with exit 0 where scripts would read success.
            if text.starts_with("Unknown tool") {
                eprintln!("{}", text);
                eprintln!("next: check the spelling, or pick one of the tools listed above");
                return Ok(3);
            }
            if !cli.quiet {
                if cli.effective_output_format() == OutputFormat::Json {
                    let v = serde_json::json!({"tool": name, "help": text});
                    if cli.raw {
                        println!("{}", serde_json::to_string(&v)?);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v)?);
                    }
                } else {
                    println!("{}", text);
                }
            }
            Ok(0)
        }
        Commands::Export(e) => {
            // B9/export ruling: an explicit --format always wins; the
            // global JSON switch (--json/--raw/--output-format json)
            // selects the JSON export format only when --format is
            // absent, and markdown stays the default otherwise.
            let format =
                e.format
                    .unwrap_or(if cli.effective_output_format() == OutputFormat::Json {
                        export::ExportFormat::Json
                    } else {
                        export::ExportFormat::Markdown
                    });
            export::run_export(
                cwd,
                home,
                format,
                e.session.as_deref(),
                e.output.as_deref(),
                e.redact,
                cli.quiet,
            )?;
            Ok(0)
        }
        Commands::Import(i) => {
            if !i.file.exists() {
                eprintln!(
                    "error[not_found]: cannot import '{}': file does not exist",
                    i.file.display()
                );
                eprintln!("  why: no file exists at that path");
                eprintln!(
                    "  next: check the path for typos, or export a session first with `zen export --format json`"
                );
                return Ok(3);
            }
            export::run_import(cwd, home, &i.file)?;
            Ok(0)
        }
        Commands::Skill(skill) => {
            skills::skill_command(cli, cwd, home, &skill.command).await?;
            Ok(0)
        }
        Commands::Review(args) => {
            return review::run_review(cli, cwd, home, args).await;
        }
        Commands::Sessions(s) => {
            commands::sessions_command(cli, cwd, home, s).await?;
            Ok(0)
        }
        Commands::Task(args) => tasks::run_task(
            args,
            &cli.effective_output_format(),
            cli.quiet,
            cli.raw,
            home,
        ),
        Commands::Secret(args) => secret::run_secret(
            args,
            &cli.effective_output_format(),
            cli.quiet,
            cli.raw,
            home,
        ),
        Commands::Eval(args) => {
            if !(0.0..=1.0).contains(&args.min_pass_rate) {
                return Err(coded_error(
                    exit_code::VALIDATION,
                    "--min-pass-rate must be between 0.0 and 1.0",
                ));
            }
            let cfg = settings::resolve_config_with_home(cli, cwd, home)?;
            let model = args.model.clone().unwrap_or_else(|| cfg.model.clone());
            let opts = eval::EvalOptions {
                model,
                task_filter: args.task.clone(),
                max_turns: args.max_turns,
                base_url: cfg.base_url.clone(),
                api_key: cfg.api_key().map(|s| s.to_string()),
                mock: args.mock,
            };
            let report = eval::run_eval(&opts).await?;
            if !cli.quiet {
                match cli.effective_output_format() {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        if cli.raw {
                            println!("{}", serde_json::to_string(&report)?);
                        } else {
                            println!("{}", report.to_json()?);
                        }
                    }
                    OutputFormat::Text => {
                        println!("{}", report.to_markdown());
                    }
                }
            }

            Ok(
                if report.passed_count as f64 + 1e-9
                    >= args.min_pass_rate * report.task_count as f64
                {
                    0
                } else {
                    1
                },
            )
        }
        Commands::PrComments { pr } => {
            let fetched = github::fetch_pr_comments(cwd, pr.as_deref())?;
            if !cli.quiet {
                if cli.effective_output_format() == OutputFormat::Json {
                    if cli.raw {
                        println!("{}", serde_json::to_string(&fetched)?);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&fetched)?);
                    }
                } else {
                    println!("{}", github::format_comments_text(&fetched));
                }
            }
            Ok(0)
        }
        Commands::Update(args) => update::run_update(
            args.check,
            home,
            cli.quiet,
            cli.effective_output_format() == OutputFormat::Json,
            cli.raw,
        ),
        Commands::CiFix {} => {
            let status = github::fetch_ci_status(cwd)?;
            if !cli.quiet {
                if cli.effective_output_format() == OutputFormat::Json {
                    if cli.raw {
                        println!("{}", serde_json::to_string(&status)?);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&status)?);
                    }
                } else {
                    println!("{}", github::format_ci_text(&status));
                }
            }

            Ok(if status.failing.is_empty() { 0 } else { 1 })
        }
    }
}

fn cost_subcommand(
    cli: &Cli,
    cwd: &Path,
    home: Option<&std::path::Path>,
    args: &CostArgs,
) -> anyhow::Result<()> {
    // --quiet suppresses stdout only: it must never skip the work,
    // swallow an error, or change the exit code (pilot finding D1:
    // cost --json --quiet used to exit 0 with no session at all).
    let quiet = cli.quiet;
    let Some(home) = home else {
        anyhow::bail!("cannot determine home directory");
    };
    let format = match cli.effective_output_format() {
        OutputFormat::Text => args.output_format,
        f => f,
    };

    let session_id = match &args.session {
        Some(s) => {
            if let Some(p) = session::find_session_path(home, s) {
                p.file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| s.clone())
            } else {
                s.clone()
            }
        }
        None => cost::latest_session_id(home)
            .ok_or_else(|| anyhow::anyhow!("no sessions found under ~/.zen/projects"))?,
    };
    let Some(ledger) = cost::load(home, &session_id) else {
        if !quiet {
            match format {
                OutputFormat::Json | OutputFormat::StreamJson => {
                    let empty = cost::summarize(&session_id, &[]);
                    println!("{}", serde_json::to_string(&cost::to_json(&empty))?);
                }
                OutputFormat::Text => {
                    println!("No cost data recorded for session {}.", session_id);
                    println!("Cost tracking starts with this version; run a session first.");
                }
            }
        }
        return Ok(());
    };
    let summary = cost::summarize(&ledger.session_id, &ledger.entries);

    let cfg = settings::resolve_config_with_home(cli, cwd, Some(home)).ok();
    let budget = crate::budget::resolve_budget(
        cli.max_budget_usd,
        cli.budget_warn_at,
        cfg.as_ref().and_then(|c| c.max_budget_usd),
        cfg.as_ref().and_then(|c| c.budget_warn_at),
    );
    if !quiet {
        match format {
            OutputFormat::Text => {
                print!("{}", cost::render_text(&summary));
                if let Some(policy) = &budget {
                    println!(
                        "{}",
                        crate::budget::budget_line(policy, summary.total_cost_usd)
                    );
                }
            }
            OutputFormat::Json | OutputFormat::StreamJson => {
                let mut v = cost::to_json(&summary);
                if let Some(policy) = &budget {
                    v["budget_usd"] = serde_json::json!(policy.max_budget_usd);
                    v["budget_warn_at"] = serde_json::json!(policy.warn_at);
                }
                println!("{}", serde_json::to_string(&v)?)
            }
        }
    }
    Ok(())
}

fn ask_folder_trust(cli: &Cli, cwd: &Path, home: Option<&Path>) {
    use std::io::{BufRead, IsTerminal, Write};
    if cli.bare || settings::folder_trusted(cli, cwd, home) {
        return;
    }
    let gated = zen_ext::gated_project_config(cwd);
    if gated.is_empty() {
        return;
    }
    if std::io::stdin().is_terminal() {
        eprintln!(
            "This folder has project config that can run commands: {}",
            gated.join("; ")
        );
        eprint!("Trust this folder? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            match zen_ext::trust_folder(cwd, home) {
                Ok(()) => return,
                Err(e) => eprintln!("warning: could not save folder trust: {}", e),
            }
        }
    }
    eprintln!("{}", untrusted_notice(&gated));
}

fn untrusted_notice(skipped: &[String]) -> String {
    format!(
        "zen: folder not trusted; skipped project {} (use --trust-folder or ZEN_TRUST_FOLDER=1 to load)",
        skipped.join("; ")
    )
}

async fn run_headless(cli: &Cli, cwd: &Path, home: Option<&Path>) -> Result<i32> {
    use std::io::IsTerminal;

    let mut cfg = settings::resolve_config_with_home(cli, cwd, home)?;
    // Same provider routing as the REPL (PR #71 review F2): a
    // catalog model must reach its own provider with the id that
    // provider accepts, or the first headless prompt fails with
    // the same HTTP 400. The plan flow re-resolves its own
    // config and routes that one itself (PR #71 review F10).
    settings::route_catalog_model(&mut cfg)?;
    crate::telemetry::maybe_ping(home, cfg.telemetry.as_deref());
    if cfg.ollama {
        crate::ollama::ensure_ollama_ready(&cfg.base_url, &cfg.model).await?;
    }
    if !cfg.trust_skipped.is_empty() {
        eprintln!("{}", untrusted_notice(&cfg.trust_skipped));
    }
    if cli.ephemeral {
        eprintln!("EPHEMERAL MODE - nothing will be saved");
    }
    let system_prompt = build_system_prompt(cli, cwd, home)?;

    let mut prompt_parts: Vec<String> = Vec::new();
    if cli.input_format == InputFormat::Text && !std::io::stdin().is_terminal() {
        let s = read_stdin()?;
        if !s.is_empty() {
            prompt_parts.push(s);
        }
    }
    let arg_prompt = cli.prompt.join(" ").trim().to_string();
    if !arg_prompt.is_empty() {
        prompt_parts.push(arg_prompt);
    }
    let full_prompt = prompt_parts.join("\n\n");
    if full_prompt.is_empty() {
        anyhow::bail!("No prompt provided. Pass a prompt argument or pipe stdin with -p.");
    }

    let (session_id, transcript_path, history_from) = resolve_session(cli, cwd, home)?;

    if cli.plan {
        return agent_run::run_plan_flow(
            cli,
            cwd,
            home,
            &cfg,
            &system_prompt,
            &full_prompt,
            &session_id,
            transcript_path.as_deref(),
            history_from.as_deref(),
        )
        .await;
    }

    let code = agent_run::run_agent_headless(
        cli,
        cwd,
        home,
        &cfg,
        &system_prompt,
        &full_prompt,
        &session_id,
        transcript_path.as_deref(),
        history_from.as_deref(),
    )
    .await?;
    Ok(code)
}

/// Resolve -c/-r to an existing transcript for the REPL (None = start fresh).
pub(crate) fn resolve_session_for_repl(
    cli: &Cli,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<Option<std::path::PathBuf>> {
    if cli.resume.as_deref().is_none_or(str::is_empty) && !cli.continue_session {
        return Ok(None);
    }
    // Like /resume, -r also accepts a session name (title).
    if let (Some(r), Some(h)) = (cli.resume.as_deref().filter(|r| !r.is_empty()), home)
        && session::find_session_path(h, r).is_none()
        && let Some((_, p)) = session::list_sessions_for_cwd(h, cwd)
            .into_iter()
            .find(|(_, p)| session::session_title(p).as_deref() == Some(r))
    {
        return Ok(Some(p));
    }
    let (_, _, from) = resolve_session(cli, cwd, home)?;
    Ok(from)
}

fn resolve_session(
    cli: &Cli,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<(
    String,
    Option<std::path::PathBuf>,
    Option<std::path::PathBuf>,
)> {
    let home_path = home.map(|p| p.to_path_buf());

    let persist = !cli.no_session_persistence && !cli.ephemeral;

    let resumed: Option<std::path::PathBuf> = if let Some(h) = &home_path {
        if let Some(r) = &cli.resume {
            if r.is_empty() {
                None
            } else {
                Some(session::find_session_path(h, r).ok_or_else(|| {
                    anyhow::anyhow!("No conversation found with session ID or path: {}", r)
                })?)
            }
        } else if cli.continue_session {
            session::latest_session_for_cwd(h, &session::encode_cwd(cwd))
        } else {
            None
        }
    } else {
        if cli.resume.as_deref().is_some_and(|r| !r.is_empty()) {
            anyhow::bail!("--resume needs a home directory to locate the session");
        }
        if cli.continue_session {
            anyhow::bail!("--continue needs a home directory to locate the session");
        }
        None
    };

    let session_id = if let Some(existing) = &resumed {
        if cli.fork_session {
            session::new_session_id()
        } else {
            existing
                .file_stem()
                .and_then(|s| s.to_str())
                .map(String::from)
                .unwrap_or_else(session::new_session_id)
        }
    } else {
        cli.session_id
            .clone()
            .unwrap_or_else(session::new_session_id)
    };

    if !persist {
        return Ok((session_id, None, resumed));
    }
    if let (Some(existing), false) = (&resumed, cli.fork_session) {
        return Ok((session_id, Some(existing.clone()), resumed.clone()));
    }
    let h = home_path.unwrap_or_else(std::env::temp_dir);
    let proj = session::ensure_session_dir(&h, cwd, &session_id)?;
    let path = session::session_path(&proj, &session_id);
    Ok((session_id, Some(path), resumed))
}

fn read_stdin() -> Result<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut handle = std::io::stdin().take(10 * 1024 * 1024 + 1);
    handle.read_to_end(&mut buf)?;
    if buf.len() > 10 * 1024 * 1024 {
        anyhow::bail!("stdin input exceeds 10 MB limit");
    }
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn exit_code_maps_provider_status_shapes() {
        // Provider failures surface from the agent layer as anyhow errors
        // whose text is "HTTP <status>: <body>" (zen-agent client).
        let err = anyhow::anyhow!("HTTP 429: rate_limit_exceeded");
        assert_eq!(exit_code_for_error(&err), exit_code::RATE_LIMIT);
        let err = anyhow::anyhow!("HTTP 403: insufficient permissions");
        assert_eq!(exit_code_for_error(&err), exit_code::FORBIDDEN);
        let err = anyhow::anyhow!("HTTP 404: model not found");
        assert_eq!(exit_code_for_error(&err), exit_code::NOT_FOUND);
        let err = anyhow::anyhow!("HTTP 401: bad key");
        assert_eq!(exit_code_for_error(&err), exit_code::AUTH);
        let err = anyhow::anyhow!("HTTP 409: already exists");
        assert_eq!(exit_code_for_error(&err), exit_code::CONFLICT);
        let err = anyhow::anyhow!("something else broke");
        assert_eq!(exit_code_for_error(&err), exit_code::GENERAL);
    }

    #[test]
    fn exit_code_for_http_status_maps_target_table() {
        // Target table (docs/exit-codes.md): 401 auth, 403 forbidden,
        // 404 not found, 409 conflict, 429 rate limit, 5xx general.
        for (status, want) in [
            (401, exit_code::AUTH),
            (403, exit_code::FORBIDDEN),
            (404, exit_code::NOT_FOUND),
            (409, exit_code::CONFLICT),
            (429, exit_code::RATE_LIMIT),
            (400, exit_code::GENERAL),
            (500, exit_code::GENERAL),
            (502, exit_code::GENERAL),
            (503, exit_code::GENERAL),
        ] {
            assert_eq!(exit_code_for_http_status(status), want, "for {status}");
        }
    }

    #[test]
    fn http_status_in_text_finds_first_http_shape() {
        assert_eq!(
            http_status_in_text("HTTP 401: bad key"),
            Some(401),
            "plain prefix"
        );
        assert_eq!(
            http_status_in_text("request failed: HTTP 500: boom [request-id: abc]"),
            Some(500),
            "wrapped shape"
        );
        assert_eq!(
            http_status_in_text("http 429: slow down"),
            Some(429),
            "lowercase shape"
        );
        assert_eq!(
            http_status_in_text("HTTP 401: first, HTTP 429: second"),
            Some(401),
            "first shape wins"
        );
        assert_eq!(http_status_in_text("connection refused"), None);
        assert_eq!(http_status_in_text("error 404 without http"), None);
        assert_eq!(http_status_in_text("HTTP 40: too short"), None);
        assert_eq!(http_status_in_text("HTTP 4010: too long"), None);
    }

    #[test]
    fn exit_code_for_headless_result_maps_error_text() {
        // Issue #165: the -p result event carries the provider
        // failure as text, so the code comes from the HTTP status
        // in that text instead of the blanket 1.
        for (text, want) in [
            ("HTTP 401: bad key", exit_code::AUTH),
            ("HTTP 403: insufficient permissions", exit_code::FORBIDDEN),
            ("HTTP 404: model not found", exit_code::NOT_FOUND),
            ("HTTP 409: already exists", exit_code::CONFLICT),
            ("HTTP 429: rate_limit_exceeded", exit_code::RATE_LIMIT),
            ("HTTP 500: upstream overloaded", exit_code::GENERAL),
            ("HTTP 503: unavailable", exit_code::GENERAL),
            ("connection refused", exit_code::GENERAL),
        ] {
            assert_eq!(
                exit_code_for_headless_result(true, text),
                want,
                "for {text:?}"
            );
        }
        assert_eq!(
            exit_code_for_headless_result(false, "HTTP 401: bad key"),
            exit_code::OK,
            "a successful turn stays 0 even when its text mentions a status"
        );
        assert_eq!(
            exit_code_for_headless_result(false, "hello from stub"),
            exit_code::OK
        );
    }

    #[test]
    fn exit_code_maps_each_rate_limit_and_not_found_signal_alone() {
        // Each `||` arm must fire on its own signal: a 429-shaped error
        // that only says "429", only "rate limit", only "rate_limit" or
        // only "too many requests" is still a rate limit, and a 404-shaped
        // error that only says "404" or only "not found" is still not-found.
        for (text, want) in [
            ("HTTP 429: slow down", exit_code::RATE_LIMIT),
            ("Rate limit exceeded, retry later", exit_code::RATE_LIMIT),
            ("rate_limit_exceeded", exit_code::RATE_LIMIT),
            ("too many requests, slow down", exit_code::RATE_LIMIT),
            ("HTTP 404: gone", exit_code::NOT_FOUND),
            ("model not found", exit_code::NOT_FOUND),
        ] {
            let err = anyhow::anyhow!(text);
            assert_eq!(exit_code_for_error(&err), want, "for {text:?}");
        }
    }

    #[test]
    fn exit_code_coded_error_wins_over_text_shapes() {
        // A CodedError keeps its source-picked code even when its message
        // happens to contain a status-shaped substring.
        let err = coded_error(exit_code::VALIDATION, "HTTP 429 shaped but coded");
        assert_eq!(exit_code_for_error(&err), exit_code::VALIDATION);
    }

    #[test]
    fn resolve_session_generates_id_and_path() {
        let dir = std::env::temp_dir().join(format!("zen_lib_sess_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cli = Cli::parse_from([
            "zen",
            "-p",
            "--session-id",
            "00000000-0000-4000-8000-0000000000aa",
            "hi",
        ]);
        let (id, path, _) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert_eq!(id, "00000000-0000-4000-8000-0000000000aa");
        assert!(
            path.unwrap()
                .ends_with("00000000-0000-4000-8000-0000000000aa.jsonl")
        );
    }

    #[test]
    fn resolve_session_no_persistence_returns_none_path() {
        let dir = std::env::temp_dir().join(format!("zen_lib_np_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cli = Cli::parse_from(["zen", "-p", "--no-session-persistence", "hi"]);
        let (_, path, _) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn resolve_session_ephemeral_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("zen_lib_eph_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cli = Cli::parse_from(["zen", "-p", "--ephemeral", "hi"]);
        let (id, path, _) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert!(!id.is_empty());
        assert!(
            path.is_none(),
            "ephemeral must not produce a transcript path"
        );

        assert!(
            !dir.join(".zen").exists(),
            "ephemeral must not create ~/.zen entries"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_session_ephemeral_allows_readonly_resume() {
        let dir = std::env::temp_dir().join(format!("zen_lib_eph_rs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let proj = session::ensure_session_dir(&dir, &dir, "orig").unwrap();
        let p = session::session_path(&proj, "orig");
        session::append_typed(&p, "user", "orig", "cwd", "u", None, None, None).unwrap();

        let cli = Cli::parse_from(["zen", "-p", "--ephemeral", "--resume", "orig", "hi"]);
        let (id, path, src) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert_eq!(id, "orig");
        assert_eq!(src, Some(p), "ephemeral resume replays history read-only");
        assert!(
            path.is_none(),
            "ephemeral resume must not open a transcript for writing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn repl_sess_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zen_repl_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_session_for_repl_fresh_returns_none() {
        let dir = repl_sess_dir("fresh");
        let cli = Cli::parse_from(["zen"]);
        assert_eq!(
            resolve_session_for_repl(&cli, &dir, Some(&dir)).unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_session_for_repl_resume_by_title() {
        let dir = repl_sess_dir("title");
        let proj = session::ensure_session_dir(&dir, &dir, "orig").unwrap();
        let p = session::session_path(&proj, "orig");
        session::append_typed(&p, "user", "orig", "cwd", "u", None, None, None).unwrap();
        session::append_title(&p, "orig", "my-title").unwrap();
        let cli = Cli::parse_from(["zen", "-r", "my-title"]);
        assert_eq!(
            resolve_session_for_repl(&cli, &dir, Some(&dir)).unwrap(),
            Some(p)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_session_for_repl_resume_by_id_and_unknown_errors() {
        let dir = repl_sess_dir("id");
        let proj = session::ensure_session_dir(&dir, &dir, "orig").unwrap();
        let p = session::session_path(&proj, "orig");
        session::append_typed(&p, "user", "orig", "cwd", "u", None, None, None).unwrap();
        let cli = Cli::parse_from(["zen", "-r", "orig"]);
        assert_eq!(
            resolve_session_for_repl(&cli, &dir, Some(&dir)).unwrap(),
            Some(p)
        );
        let bad = Cli::parse_from(["zen", "-r", "no-such-session"]);
        assert!(resolve_session_for_repl(&bad, &dir, Some(&dir)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_session_fork_gives_new_id() {
        let dir = std::env::temp_dir().join(format!("zen_lib_fork_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let proj = session::ensure_session_dir(&dir, &dir, "orig").unwrap();
        let p = session::session_path(&proj, "orig");
        session::append_typed(&p, "user", "orig", "cwd", "u", None, None, None).unwrap();

        let cli = Cli::parse_from(["zen", "-p", "--resume", "orig", "--fork-session", "hi"]);
        let (id, path, src) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert_ne!(id, "orig");
        assert!(path.unwrap().ends_with(format!("{}.jsonl", id)));
        assert_eq!(src.unwrap(), p, "fork replays the original transcript");
    }

    #[test]
    fn resolve_session_continue_picks_latest() {
        let dir = std::env::temp_dir().join(format!("zen_lib_cont_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let proj = session::ensure_session_dir(&dir, &dir, "first").unwrap();
        let p1 = session::session_path(&proj, "first");
        session::append_typed(&p1, "user", "first", "cwd", "u", None, None, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));
        let proj2 = session::ensure_session_dir(&dir, &dir, "second").unwrap();
        let p2 = session::session_path(&proj2, "second");
        session::append_typed(&p2, "user", "second", "cwd", "u", None, None, None).unwrap();

        let cli = Cli::parse_from(["zen", "-p", "--continue", "hi"]);
        let (id, path, _) = resolve_session(&cli, &dir, Some(&dir)).unwrap();
        assert_eq!(id, "second");
        assert_eq!(path.unwrap(), p2);
    }
}
