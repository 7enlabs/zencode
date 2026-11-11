use crate::cli::Cli;
use crate::commands;
use crate::repl_polish;
use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zen_core::{EventSink, RunResult, UsageStats};

enum LineAction {
    Continue,
    Exit,
}

fn status_line(theme: repl_polish::Theme, cfg_model: &str, cwd: &Path) -> String {
    let (branch, _) = crate::git::git_info(cwd);
    let cwd_str = cwd.display().to_string();

    let cwd_short = if let Some(home) = dirs::home_dir() {
        let h = home.display().to_string();
        if cwd_str.starts_with(&h) {
            cwd_str.replacen(&h, "~", 1)
        } else {
            cwd_str
        }
    } else {
        cwd_str
    };
    // v2: provider in dim, short model name in accent — the eye lands on
    // the model, the provider is still there for disambiguation.
    let (provider, short) = split_model_id(cfg_model);
    let model = if provider.is_empty() {
        theme.paint(repl_polish::ColorRole::Accent, cfg_model)
    } else {
        format!(
            "{}{}",
            theme.paint(repl_polish::ColorRole::Dim, &format!("{provider}/")),
            theme.paint(repl_polish::ColorRole::Accent, short)
        )
    };
    // Middle-truncate the cwd when the whole line would exceed ~80 cols.
    let cwd_short = truncate_middle(&cwd_short, 40);
    let rest = match branch {
        Some(b) => format!(" · {} · {}", cwd_short, b.trim()),
        None => format!(" · {}", cwd_short),
    };
    format!(
        "{}{}",
        model,
        theme.paint(repl_polish::ColorRole::Dim, &rest)
    )
}

/// Split a model id into (provider, short name). "google/gemini-4-argon:free"
/// -> ("google", "gemini-4-argon:free"); an id without "/" has no provider.
fn split_model_id(id: &str) -> (&str, &str) {
    match id.split_once('/') {
        Some((provider, short)) => (provider, short),
        None => ("", id),
    }
}

/// Middle-truncate a path to at most `max` visible chars, keeping the
/// first segment and the last two, joined by "…". Short paths pass through.
/// The result is always at most `max` chars.
fn truncate_middle(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    if max < 4 {
        return "…".to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 4 {
        // Too few segments to truncate meaningfully; cut the tail.
        let mut s: String = path.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        return s;
    }
    let head = parts[..2].join("/");
    let tail = parts[parts.len() - 2..].join("/");
    let mut out = format!("{head}/…/{tail}");
    if out.chars().count() > max {
        // Still too long: drop the head, keep the tail, cut if needed.
        out = format!("…/{tail}");
    }
    if out.chars().count() > max {
        let mut s: String = out.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        out = s;
    }
    out
}

/// One line of the welcome box: segments of (text, optional colour role).
/// The visible width is the sum of the text char counts (ANSI codes from
/// `paint` do not count). A `None` role means plain text.
type BoxLine = Vec<(String, Option<repl_polish::ColorRole>)>;

fn box_line_width(line: &BoxLine) -> usize {
    line.iter().map(|(t, _)| t.chars().count()).sum()
}

fn render_box_line(theme: repl_polish::Theme, line: &BoxLine, width: usize) -> String {
    let mut out = String::new();
    for (text, role) in line {
        match role {
            Some(r) => out.push_str(&theme.paint(*r, text)),
            None => out.push_str(text),
        }
    }
    let pad = width.saturating_sub(box_line_width(line));
    out.push_str(&" ".repeat(pad));
    out
}

/// Bordered welcome box v2 (design doc `docs/design/repl-visual-redesign-v2.md`).
/// Rounded corners, width capped at 60 inner columns, brand line `✓ zen`,
/// theme colours, greeting only for returning users. Pure formatting
/// (unit-testable). `sessions` is the list from `list_sessions_for_cwd`
/// (empty on first run); `branch` is the git branch or `None`.
fn welcome_box(
    theme: repl_polish::Theme,
    version: &str,
    model: &str,
    cwd: &str,
    branch: Option<&str>,
    sessions: &[(String, std::path::PathBuf)],
    ephemeral: bool,
) -> String {
    use repl_polish::ColorRole::{Accent, Dim, Error};
    let mut content: Vec<BoxLine> = Vec::new();
    // Brand line: ✓ zen 0.1.0 (check is the essence of the Z-Check logo).
    content.push(vec![
        ("✓ ".to_string(), Some(Accent)),
        ("zen".to_string(), Some(Accent)),
        (format!(" {version}"), Some(Dim)),
    ]);
    content.push(vec![]);
    // Greeting only when there is a previous session for this cwd.
    if !sessions.is_empty() {
        content.push(vec![("Welcome back!".to_string(), None)]);
    }
    let (provider, short) = split_model_id(model);
    let mut model_line: BoxLine = vec![("model  ".to_string(), Some(Dim))];
    model_line.push((short.to_string(), Some(Accent)));
    if !provider.is_empty() {
        model_line.push((format!("  ·  {provider}"), Some(Dim)));
    }
    content.push(model_line);
    content.push(vec![
        ("cwd    ".to_string(), Some(Dim)),
        (cwd.to_string(), None),
    ]);
    if let Some(b) = branch {
        content.push(vec![
            ("branch ".to_string(), Some(Dim)),
            (b.trim().to_string(), None),
        ]);
    }
    if ephemeral {
        content.push(vec![(
            "EPHEMERAL MODE - nothing will be saved".to_string(),
            Some(Error),
        )]);
    }
    // Recent activity, one dim line when a previous session exists.
    if let Some((_, path)) = sessions.first() {
        if let Ok(mtime) = path.metadata().and_then(|m| m.modified()) {
            let local: chrono::DateTime<chrono::Local> = mtime.into();
            content.push(vec![]);
            content.push(vec![(
                format!("Last session {}", local.format("%H:%M")),
                Some(Dim),
            )]);
        }
    }
    content.push(vec![]);
    content.push(vec![(
        "/model to switch · /help for commands".to_string(),
        Some(Dim),
    )]);
    // Width: longest visible line, capped at 60, min 40. The cwd is
    // middle-truncated to fit instead of setting the width.
    let raw_width = content.iter().map(box_line_width).max().unwrap_or(0);
    let width = raw_width.clamp(40, 60);
    // Truncate the cwd line if it still overflows the cap.
    for line in content.iter_mut() {
        let mut over = box_line_width(line).saturating_sub(width);
        while over > 0 {
            // Only the cwd line can overflow (it is the longest value);
            // truncate its plain (unlabelled) segment.
            let mut truncated = false;
            for (text, role) in line.iter_mut() {
                if role.is_none() {
                    let keep = text.chars().count().saturating_sub(over);
                    *text = truncate_middle(text, keep);
                    truncated = true;
                }
            }
            if !truncated {
                break;
            }
            over = box_line_width(line).saturating_sub(width);
        }
    }
    let mut out = String::new();
    out.push_str(&format!("╭{}╮\n", "─".repeat(width + 2)));
    for line in &content {
        out.push_str(&format!("│ {} │\n", render_box_line(theme, line, width)));
    }
    out.push_str(&format!("╰{}╯", "─".repeat(width + 2)));
    out
}

pub async fn run_repl(cli: &Cli, cwd: &Path, home: Option<&Path>) -> Result<()> {
    let mut cfg = crate::settings::resolve_config_with_home(cli, cwd, home)?;
    crate::telemetry::maybe_ping(home, cfg.telemetry.as_deref());
    // The key resolved for the configured route, kept verbatim: a
    // /model round trip through another provider restores this key
    // instead of leaking that provider's key (PR #71 review F1).
    let home_api_key = cfg.api_key.clone();
    // A catalog id is a picker label, not a wire id: route the model
    // to its serving provider (gateway id, base URL, key) so the very
    // first prompt does not fail with the same HTTP 400.
    crate::settings::route_catalog_model(&mut cfg)?;
    if cfg.ollama {
        crate::ollama::ensure_ollama_ready(&cfg.base_url, &cfg.model).await?;
    }
    let mut session_id = cli
        .session_id
        .clone()
        .unwrap_or_else(crate::session::new_session_id);

    let transcript_path: Option<std::path::PathBuf> = if cli.ephemeral {
        None
    } else {
        let proj_dir = crate::session::ensure_session_dir(
            home.unwrap_or(&std::env::temp_dir()),
            cwd,
            &session_id,
        )?;
        Some(crate::session::session_path(&proj_dir, &session_id))
    };

    let mut writer = match &transcript_path {
        Some(p) => crate::session::TranscriptWriter::new(p.clone(), &session_id, cwd),
        None => crate::session::TranscriptWriter::ephemeral(&session_id, cwd),
    };

    // v2 welcome box: theme, branch and previous sessions for the
    // greeting / Recent line. The long hint line is first-run-only
    // (returning users already know !cmd / @file / #note).
    // Startup fast path: only the branch is shown, so run just
    // rev-parse. git_info also runs `git status --porcelain` (a second
    // process spawn, slow on Windows) and this caller discards it.
    let branch = crate::git::current_branch(cwd);
    let sessions = home
        .map(|h| crate::session::list_sessions_for_cwd(h, cwd))
        .unwrap_or_default();
    let first_run = sessions.is_empty();
    println!(
        "{}",
        welcome_box(
            repl_polish::Theme::default(),
            env!("CARGO_PKG_VERSION"),
            &cfg.model,
            &cwd.display().to_string(),
            branch.as_deref(),
            &sessions,
            cli.ephemeral
        )
    );
    if first_run {
        println!("Type /help for commands, !cmd for shell, @file to attach files, #note to remember, Ctrl-D to exit. While typing a /command, Tab lists the matches and Esc cancels.");
    }

    let assembly = crate::agent_run::assemble(
        cli,
        cwd,
        home,
        &cfg,
        &session_id,
        transcript_path.as_deref(),
    )
    .await?;

    let progress_sink = Arc::new(crate::repl_progress::ReplProgressSink::new());
    let progress: Arc<dyn EventSink> = progress_sink.clone();
    let llm = crate::eventlog::wrap(
        cfg.gated_llm_client(progress.clone(), home),
        assembly.recorder.as_ref(),
    );

    let mut config = build_repl_config(cli, &cfg, cwd);
    let _ = &mut config;

    let base_system_prompt = config.system_prompt.clone();
    if config.permission_mode == zen_core::PermissionMode::Plan {
        config.system_prompt =
            zen_agent::plan::system_prompt_for_mode(&base_system_prompt, config.permission_mode);
    }

    let checkpoints = Arc::new(zen_tools::checkpoint::Checkpoints::new(
        match &transcript_path {
            Some(q) => crate::session::session_scratch_dir(q),
            None => zen_core::ToolContext::new(cwd.to_path_buf(), session_id.clone()).scratch_dir(),
        }
        .join("checkpoints"),
    ));
    let mut agent = zen_agent::Agent::new(
        llm,
        assembly.registry.clone(),
        assembly.permissions.clone(),
        Some(Arc::new(crate::agent_run::ReplPrompter)),
        assembly.hooks.clone(),
        progress.clone(),
        {
            let mut ctx = crate::agent_run::tool_context_public(cwd, &cfg, cli, &session_id);

            ctx.session_dir = transcript_path
                .as_ref()
                .map(|q| crate::session::session_scratch_dir(q));

            ctx.checkpoints = Some(checkpoints.clone());
            ctx
        },
        config,
    );
    agent.journal = assembly
        .recorder
        .clone()
        .map(|r| r as Arc<dyn zen_core::ToolJournal>);

    if agent.config.permission_mode == zen_core::PermissionMode::Plan {
        println!("Plan mode is ON (read-only). Research, then present a plan. /plan toggles it.");
    }

    repl_session_start(&mut agent, &session_id, cwd, &cfg, "startup").await;

    let mut interrupts = ReplInterrupts::install();
    let mut repl = ReplState {
        session_id: &mut session_id,
        writer: &mut writer,
        mcp_status: assembly.mcp_status.clone(),
        interrupts: interrupts.as_mut(),
        recorder: assembly.recorder.clone(),
        home_api_key,
        // Auto state comes from config resolution, not from model
        // equality: pinning the provider's default model is a pin,
        // not auto (PR #72 review follow-up).
        auto_model: cfg.model_auto,
        goal: None,
        personality: repl_polish::Personality::default(),
        theme: repl_polish::Theme::default(),
        vim_mode: false,
        base_system_prompt,
        pre_plan_mode: zen_core::PermissionMode::Default,
        pending_plan: None,
        scope: crate::doneguard::ScopeGuard::default(),
        scope_seeded: false,
        checkpoints,
        progress: progress_sink.clone(),
    };

    // Honour -c/-r and a positional prompt: resume first, then run the
    // prompt as the first turn and stay in the session.
    if !cli.ephemeral || cli.resume.is_some() || cli.continue_session {
        match crate::resolve_session_for_repl(cli, cwd, home) {
            Ok(Some(path)) if !cli.fork_session => {
                if let Err(e) = resume_into(path, cwd, &mut agent, &mut repl) {
                    eprintln!("error: {e:#}");
                }
            }
            Ok(Some(path)) => {
                // --fork-session: replay history into a fresh session id
                let keep_id = repl.session_id.clone();
                match crate::eventlog::resume_history(&path) {
                    Ok((_, history)) => {
                        agent.set_messages(history);
                        println!("forked from {} as {}", path.display(), keep_id);
                    }
                    Err(e) => eprintln!("error: {e:#}"),
                }
            }
            Ok(None) => {}
            Err(e) => return Err(e),
        }
    }
    let first_prompt = cli.prompt.join(" ").trim().to_string();
    if !first_prompt.is_empty() {
        match handle_line(
            &first_prompt,
            cli,
            cwd,
            home,
            &mut cfg,
            &mut repl,
            &mut agent,
        )
        .await
        {
            Ok(LineAction::Exit) => {
                crate::agent_run::run_session_end(
                    &agent.hooks,
                    &repl.session_id.clone(),
                    cwd,
                    &cfg,
                    "prompt_input_exit",
                )
                .await;
                println!("bye");
                return Ok(());
            }
            Ok(LineAction::Continue) => {}
            Err(e) => eprintln!("error: {e:#}"),
        }
    }

    let mut reader = crate::repl_input::LineReader::new(home);
    let mut last_interrupt: Option<std::time::Instant> = None;
    loop {
        println!("{}", status_line(repl.theme, &agent.config.model, cwd));
        let prompt = {
            let mut p = repl_polish::prompt_text(repl.theme, &repl.goal);

            let badge =
                crate::planmode::prompt_for(agent.config.permission_mode).trim_end_matches("> ");
            let at = p.rfind('\n').map_or(0, |i| i + 1);
            p.insert_str(at, badge);
            p
        };
        reader.set_vi(repl.vim_mode);
        reader.set_prompt_style(repl.theme);

        let line = match reader.read(&prompt) {
            crate::repl_input::ReadOutcome::Line(l) => {
                last_interrupt = None;
                l
            }
            crate::repl_input::ReadOutcome::Eof => break,
            crate::repl_input::ReadOutcome::Interrupted => {
                if last_interrupt
                    .map(|t| t.elapsed() < std::time::Duration::from_secs(2))
                    .unwrap_or(false)
                {
                    eprintln!();
                    std::process::exit(130);
                }
                last_interrupt = Some(std::time::Instant::now());
                eprintln!("(press Ctrl-C again to exit, or Ctrl-D)");
                continue;
            }
        };
        let joined = read_multiline(line, || match reader.read("... ") {
            crate::repl_input::ReadOutcome::Line(next) => Some(next),
            _ => None,
        });
        reader.remember(joined.trim_end_matches(['\n', '\r']));
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            continue;
        }

        match handle_line(trimmed, cli, cwd, home, &mut cfg, &mut repl, &mut agent).await {
            Ok(LineAction::Exit) => break,
            Ok(LineAction::Continue) => {}
            Err(e) => eprintln!("error: {e:#}"),
        }
    }

    let session_id = repl.session_id.clone();
    crate::agent_run::run_session_end(&agent.hooks, &session_id, cwd, &cfg, "prompt_input_exit")
        .await;
    println!("bye");
    Ok(())
}

async fn repl_session_start(
    agent: &mut zen_agent::Agent,
    session_id: &str,
    cwd: &Path,
    cfg: &crate::settings::ResolvedConfig,
    source: &str,
) {
    let hooks = agent.hooks.clone();
    if let Some(ctx) =
        crate::agent_run::run_session_start(&hooks, session_id, cwd, cfg, source).await
    {
        agent.messages.push(zen_core::ChatMessage::user(format!(
            "[SessionStart hook context]\n{}",
            ctx
        )));
    }
}

struct ReplState<'a> {
    session_id: &'a mut String,
    writer: &'a mut crate::session::TranscriptWriter,
    mcp_status: Vec<(String, String)>,
    interrupts: Option<&'a mut ReplInterrupts>,

    recorder: Option<Arc<crate::eventlog::Recorder>>,

    // API key resolved at startup for the configured route; a /model
    // round trip restores it (see apply_model_switch, review F1).
    home_api_key: Option<String>,

    // Auto mode (Leader tin 7962): on by default. A pinned /model
    // switch turns it off; /model auto and /provider turn it back on.
    auto_model: bool,

    goal: Option<String>,

    personality: repl_polish::Personality,

    theme: repl_polish::Theme,

    vim_mode: bool,

    base_system_prompt: String,

    pre_plan_mode: zen_core::PermissionMode,

    pending_plan: Option<String>,

    scope: crate::doneguard::ScopeGuard,

    scope_seeded: bool,

    checkpoints: Arc<zen_tools::checkpoint::Checkpoints>,

    progress: Arc<crate::repl_progress::ReplProgressSink>,
}

struct ReplInterrupts {
    rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    in_turn: Arc<AtomicBool>,
}

impl ReplInterrupts {
    /// Unix-only interrupt handling; on other platforms this returns None
    /// and turns run uninterrupted. While a turn is running, Ctrl-C
    /// cancels the turn; outside a turn, two presses within 2 seconds exit
    /// with code 130, so a single stray press never kills the session.
    fn install() -> Option<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sig = signal(SignalKind::interrupt()).ok()?;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let in_turn = Arc::new(AtomicBool::new(false));
            let flag = in_turn.clone();
            tokio::spawn(async move {
                let mut last: Option<std::time::Instant> = None;
                while sig.recv().await.is_some() {
                    if flag.load(Ordering::SeqCst) {
                        let _ = tx.send(());
                        last = None;
                        continue;
                    }
                    if last
                        .map(|t| t.elapsed() < std::time::Duration::from_secs(2))
                        .unwrap_or(false)
                    {
                        eprintln!();
                        std::process::exit(130);
                    }
                    last = Some(std::time::Instant::now());
                    eprintln!("\n(press Ctrl-C again to exit, or Ctrl-D)");
                }
            });
            Some(Self { rx, in_turn })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

async fn run_turn_interruptible(
    agent: &mut zen_agent::Agent,
    prompt: &str,
    interrupts: Option<&mut ReplInterrupts>,
) -> Option<RunResult> {
    // S9 pattern 3: a friendly thinking status on stderr the moment a turn
    // starts; stderr keeps the stdout answer channel clean.
    eprintln!("Thinking...");
    if let Some(c) = &agent.ctx.checkpoints {
        c.begin_turn(prompt);
    }
    let Some(intr) = interrupts else {
        return Some(agent.run_turn(prompt).await);
    };
    while intr.rx.try_recv().is_ok() {}
    let before = agent.messages().to_vec();
    let pre = crate::agent_run::descendants_of(std::process::id());
    intr.in_turn.store(true, Ordering::SeqCst);
    let out = {
        let turn = agent.run_turn(prompt);
        tokio::pin!(turn);
        tokio::select! {
            r = &mut turn => Some(r),
            _ = intr.rx.recv() => {

                crate::agent_run::kill_new_descendants(&pre);
                None
            }
        }
    };
    intr.in_turn.store(false, Ordering::SeqCst);
    if out.is_none() {
        agent.set_messages(before);
        eprintln!("Interrupted.");
    }
    out
}

/// Load a saved transcript into the running REPL (shared by /resume and -c/-r).
fn resume_into(
    path: std::path::PathBuf,
    cwd: &Path,
    agent: &mut zen_agent::Agent,
    repl: &mut ReplState<'_>,
) -> Result<()> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let (committed, history) = crate::eventlog::resume_history(&path)?;
    agent.ctx.session_dir = Some(crate::session::session_scratch_dir(&path));
    if let Some(r) = &repl.recorder {
        r.set_path(path.clone());
    }
    *repl.writer = crate::session::TranscriptWriter::new(path, &id, cwd);
    if history != committed {
        if let Err(e) = repl.writer.persist_turn(&committed, &history) {
            eprintln!("warning: could not write transcript: {}", e);
        }
    }
    agent.set_messages(history);
    *repl.session_id = id.clone();
    agent.ctx.session_id = id.clone();
    println!(
        "resumed session {} ({} messages)",
        id,
        agent.messages().len()
    );
    Ok(())
}

/// Join continuation lines into one turn: a trailing `\\` continues on the
/// next line, and a line starting with `"""` opens a block that runs until a
/// line ending with `"""` (or end of input).
fn read_multiline(first: String, mut next: impl FnMut() -> Option<String>) -> String {
    let head = first.trim_end_matches(['\n', '\r']).to_string();
    if let Some(rest) = head.trim_start().strip_prefix("\"\"\"") {
        let mut out = String::new();
        let mut cur = rest.to_string();
        loop {
            if let Some(body) = cur.trim_end().strip_suffix("\"\"\"") {
                out.push_str(body);
                return out;
            }
            out.push_str(&cur);
            out.push('\n');
            match next() {
                Some(l) => cur = l.trim_end_matches(['\n', '\r']).to_string(),
                None => return out,
            }
        }
    }
    let mut out = head;
    while out.trim_end().ends_with('\\') {
        let t = out.trim_end().len() - 1;
        out.truncate(t);
        out.push('\n');
        match next() {
            Some(l) => out.push_str(l.trim_end_matches(['\n', '\r'])),
            None => break,
        }
    }
    out
}

/// One-line close of a REPL turn. `input_tokens` already includes the
/// prompt-cache reads (they are a subset of the prompt tokens the
/// provider reports), so the total counts them; name the cached part
/// instead of silently folding it in.
fn format_turn_summary(duration_ms: u64, usage: &UsageStats) -> String {
    let total = usage.input_tokens + usage.output_tokens;
    if usage.cache_read_input_tokens > 0 {
        format!(
            "Done in {:.1}s - {} tokens used ({} from cache)",
            duration_ms as f64 / 1000.0,
            total,
            usage.cache_read_input_tokens
        )
    } else {
        format!(
            "Done in {:.1}s - {} tokens used",
            duration_ms as f64 / 1000.0,
            total
        )
    }
}

fn print_turn(result: Option<RunResult>) {
    if let Some(r) = result {
        if r.is_error {
            eprintln!("error: {}", r.result);
        } else if !r.result.is_empty() {
            // Claude-Code-style marker: assistant text is prefixed with ●.
            println!("● {}", r.result);
        }
        // S9 pattern 3: close the turn with the real elapsed time and
        // token total from the run result, on stderr like the rest of
        // the live progress so stdout readers are unaffected.
        eprintln!("{}", format_turn_summary(r.duration_ms, &r.usage));
    }
}

fn init_prompt_with_import_offer(cwd: &std::path::Path) -> String {
    let candidates = zen_ext::detect_import_candidates(cwd);
    if candidates.is_empty() {
        return commands::init_prompt();
    }
    println!("Found existing agent memory files:");
    for c in &candidates {
        println!("  {}", c.display());
    }
    print!("Import their content into the new ZEN.md? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    let yes = std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if yes {
        commands::init_prompt_with_imports(&candidates)
    } else {
        commands::init_prompt()
    }
}

fn prompt_yes_no(question: &str) -> bool {
    use std::io::Write;
    print!("{} [y/N] ", question);
    let _ = std::io::stdout().flush();
    let mut line = String::new();

    match std::io::stdin().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

fn maybe_auto_commit(cwd: &Path, cfg: &crate::settings::ResolvedConfig) {
    if !cfg.auto_commit {
        return;
    }
    let files = crate::git::changed_files(cwd);
    if files.is_empty() {
        return;
    }
    println!("Changed files:");
    for (status, path) in &files {
        println!("  {} {}", status, path);
    }
    let message = crate::git::suggest_commit_message(&files);
    println!("Suggested message: {}", message);
    if prompt_yes_no("Commit these changes?") {
        match crate::git::stage_and_commit(cwd, &message) {
            Ok(summary) => println!("{}", summary),
            Err(e) => eprintln!("commit failed: {}", e),
        }
    } else {
        println!("commit skipped (use /commit to commit later)");
    }
}

async fn handle_line(
    line: &str,
    cli: &Cli,
    cwd: &Path,
    home: Option<&Path>,
    cfg: &mut crate::settings::ResolvedConfig,
    repl: &mut ReplState<'_>,
    agent: &mut zen_agent::Agent,
) -> Result<LineAction> {
    let clearing = line == "/clear" || line.starts_with("/clear ");
    let before = if clearing {
        vec![]
    } else {
        agent.messages().to_vec()
    };
    if let Some(rest) = line.strip_prefix('!') {
        run_shell_passthrough(rest, cwd, cfg, agent);
        persist(repl.writer, &before, agent);
        return Ok(LineAction::Continue);
    }
    if let Some(note) = line.strip_prefix('#') {
        match zen_ext::append_memory_note(cwd, note) {
            Ok(p) => println!("remembered in {}", p.display()),
            Err(e) => eprintln!("could not append note: {}", e),
        }
        return Ok(LineAction::Continue);
    }
    if line.starts_with('/') {
        let action = handle_slash(line, cli, cwd, home, cfg, repl, agent).await;
        persist(repl.writer, &before, agent);
        return action;
    }

    if repl.pending_plan.is_some() && crate::planmode::is_approve_word(line) {
        let plan = repl.pending_plan.take().unwrap_or_default();
        if let Err(e) = repl.writer.record_plan_approved(&plan) {
            eprintln!("warning: could not record approved plan: {}", e);
        }
        crate::planmode::set_permission_mode(
            &mut agent.config,
            zen_core::PermissionMode::AcceptEdits,
            &repl.base_system_prompt,
        );
        println!("plan approved and recorded - permission mode is now acceptEdits.");
        return Ok(LineAction::Continue);
    }

    if repl.pending_plan.is_some() {
        repl.pending_plan = None;
    }

    if !repl.scope_seeded {
        repl.scope.seed_from_prompt(line, cwd);
        repl.scope_seeded = true;
        agent.config.done_scope = repl.scope.declared.clone();
    }
    let scope_active = !repl.scope.declared.is_empty();
    let turn_start_files = if scope_active {
        crate::doneguard::changed_files(cwd)
    } else {
        Vec::new()
    };
    let usage_before = agent.usage.clone();
    let prompt = match crate::mentions::expand_mentions(line, cwd, cfg.mention_token_budget) {
        Ok(exp) => {
            for f in &exp.files {
                println!("@{} ({} tokens)", f.path, f.tokens);
            }
            for w in &exp.warnings {
                eprintln!("warning: {}", w);
            }
            exp.prompt
        }
        Err(e) => {
            eprintln!("error: {}", e);
            return Ok(LineAction::Continue);
        }
    };
    let result = run_turn_interruptible(agent, &prompt, repl.interrupts.as_deref_mut()).await;
    persist(repl.writer, &before, agent);
    record_turn_cost(home, repl.session_id.as_str(), cfg, agent, &usage_before);

    if agent.config.permission_mode == zen_core::PermissionMode::Plan {
        if let Some(r) = &result {
            if zen_agent::plan::looks_like_plan_exit(&r.result) {
                repl.pending_plan = Some(crate::planmode::strip_plan_exit_markers(&r.result));
                println!(
                    "Plan ready. Reply `approve` to accept it (exits plan mode → acceptEdits), \
                     or keep chatting to refine."
                );
            }
        }
    }
    if scope_active {
        let after = crate::doneguard::changed_files(cwd);
        let fresh: Vec<String> = after
            .into_iter()
            .filter(|f| !turn_start_files.contains(f))
            .collect();
        let violations = repl.scope.violations(&fresh);
        if !violations.is_empty() {
            eprintln!(
                "[scope guard] {} file(s) changed outside the declared scope ({}): {}.                  Run /scope to adjust the scope.",
                violations.len(),
                repl.scope.declared.join(", "),
                violations.join(", ")
            );
        }
    }
    let finished = result.is_some();
    print_turn(result);

    if finished {
        maybe_auto_commit(cwd, cfg);
    }
    Ok(LineAction::Continue)
}

async fn offer_fix_turn(
    prompt: &str,
    what: &str,
    cfg: &crate::settings::ResolvedConfig,
    repl: &mut ReplState<'_>,
    agent: &mut zen_agent::Agent,
) {
    match crate::github::fix_offer_for_mode(cfg.permission_mode) {
        crate::github::FixOffer::Never => {
            println!("(plan mode is read-only: not offering to apply fixes)");
        }
        crate::github::FixOffer::Auto => {
            print_turn(run_turn_interruptible(agent, prompt, repl.interrupts.as_deref_mut()).await);
        }
        crate::github::FixOffer::Ask => {
            eprint!("Address {} with the agent? [y/N] ", what);
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_ok()
                && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            {
                print_turn(
                    run_turn_interruptible(agent, prompt, repl.interrupts.as_deref_mut()).await,
                );
            }
        }
    }
}

/// One row of the `/model` picker: the id plus the facts a person uses
/// to choose (provider, context window, probe latency). `latency_ms` is
/// `None` for rows from the static catalog (never probed).
struct ModelRow {
    id: String,
    provider: String,
    short: String,
    context_window: Option<u32>,
    latency_ms: Option<u64>,
}

impl ModelRow {
    fn from_probe(p: &crate::models::ModelProbe) -> Self {
        let (provider, short) = split_model_id(&p.model);
        ModelRow {
            id: p.model.clone(),
            provider: provider.to_string(),
            short: short.to_string(),
            context_window: p.context_window,
            latency_ms: Some(p.latency_ms),
        }
    }

    fn from_catalog(m: &crate::models::CatalogModel) -> Self {
        let (provider, short) = split_model_id(m.id);
        ModelRow {
            id: m.id.to_string(),
            provider: provider.to_string(),
            short: short.to_string(),
            context_window: Some(m.context_window),
            latency_ms: None,
        }
    }
}

/// Humanise a context window: 1_000_000 -> "1M", 262_144 -> "262K".
fn humanise_context(ctx: Option<u32>) -> String {
    match ctx {
        None => "—".to_string(),
        Some(n) if n >= 1_000_000 => format!("{}M", n / 1_000_000),
        Some(n) if n >= 1_000 => format!("{}K", n / 1_000),
        Some(n) => n.to_string(),
    }
}

/// Model rows the user can pick from in the REPL: the probed pool cache
/// when it matches the current provider, otherwise the static free catalog.
/// The probe data (context window, latency) is threaded through instead of
/// being discarded — the picker shows it.
fn available_model_rows(home: Option<&Path>, base_url: &str) -> (Vec<ModelRow>, Option<String>) {
    if let Some(cache) = crate::models::load_cache(home) {
        if cache.base_url == base_url {
            let rows: Vec<ModelRow> = cache
                .results
                .iter()
                .filter(|p| p.success)
                .map(ModelRow::from_probe)
                .collect();
            if !rows.is_empty() {
                // `zen models` warns when the probe cache is stale;
                // the picker - the path users actually switch with -
                // used it silently, offering models that may have
                // left the free tier or been renamed (bug hunt A B5).
                let stale = crate::models::cache_is_stale(&cache).then(|| {
                    let age = crate::models::cache_age_secs(&cache).unwrap_or(-1);
                    format!(
                        "warning: models cache is {} old (>24h); run `zen models --refresh` to re-probe",
                        crate::models::human_age(age)
                    )
                });
                return (rows, stale);
            }
        }
    }
    // The catalog fallback shows only what the current provider
    // really serves (catalog by real provider): on the Experiential
    // base the OpenRouter entries must not appear as pickable
    // models. A custom base has no known provider, so the whole
    // catalog stays rather than excluding models that might serve it.
    let rows = match crate::models::provider_for_base_url(base_url) {
        Some(provider) => crate::models::models_for_provider(provider)
            .into_iter()
            .map(ModelRow::from_catalog)
            .collect(),
        None => crate::models::FREE_MODEL_CATALOG
            .iter()
            .map(ModelRow::from_catalog)
            .collect(),
    };
    (rows, None)
}

/// Numbered model table for `/model`: short name, provider, context
/// window and probe latency, with the current model marked by a leading
/// `▸`. Pure formatting so it is unit-testable without a provider.
fn format_model_list(current: &str, rows: &[ModelRow], auto: bool) -> String {
    let mut out = String::new();
    let (_, current_short) = split_model_id(current);
    let auto_mark = if auto { " (auto)" } else { "" };
    out.push_str(&format!("Models — current: {current_short}{auto_mark}\n"));
    // Drop columns on narrow output: latency first, then context.
    let name_width = rows
        .iter()
        .map(|r| r.short.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(10, 32);
    let row_width = rows
        .iter()
        .map(|r| r.short.chars().count() + r.provider.chars().count())
        .max()
        .unwrap_or(0);
    let show_latency = row_width < 56;
    let show_ctx = row_width < 64;
    for (i, row) in rows.iter().enumerate() {
        // The current model is stored as the gateway id the
        // provider accepts, so a catalog row also matches when its
        // gateway id is the current one.
        let marker = if row.id == current || crate::models::gateway_model_id(&row.id) == current {
            "▸ "
        } else {
            "  "
        };
        let latency = match row.latency_ms {
            Some(ms) => format!("{ms}ms"),
            None => "—".to_string(),
        };
        let short = if row.short.chars().count() > name_width {
            let mut s: String = row.short.chars().take(name_width - 1).collect();
            s.push('…');
            s
        } else {
            row.short.clone()
        };
        let mut line = format!(
            "{}{}. {:<name_width$} {:<10}",
            marker,
            i + 1,
            short,
            row.provider,
            name_width = name_width
        );
        if show_ctx {
            line.push_str(&format!(" {:>7} ctx", humanise_context(row.context_window)));
        }
        if show_latency {
            line.push_str(&format!("  {latency:>6}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("Enter a number, name, or unique part — /model 2, /model nemotron\n");
    out
}

/// Resolve a `/model` argument to a model id: a 1-based number picks from
/// the list, otherwise an exact id wins, else a unique substring match.
/// Id matching is case-insensitive: catalog ids are user-facing
/// (`/model DEEPSEEK-V4.1-FLASH`), so casing must not change the meaning.
/// The canonical catalog id is returned, never the user's casing, so the
/// id still matches the catalog downstream.
fn pick_model(arg: &str, rows: &[ModelRow]) -> Result<String, String> {
    if let Ok(n) = arg.parse::<usize>() {
        return match rows.get(n.wrapping_sub(1)) {
            Some(row) => Ok(row.id.clone()),
            None => Err(format!("no model #{} (1-{})", n, rows.len())),
        };
    }
    if let Some(row) = rows.iter().find(|r| r.id.eq_ignore_ascii_case(arg)) {
        return Ok(row.id.clone());
    }
    let needle = arg.to_ascii_lowercase();
    let matches: Vec<&ModelRow> = rows
        .iter()
        .filter(|r| r.id.to_ascii_lowercase().contains(&needle))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => Err(format!("no model matches '{arg}'")),
        _ => Err(format!(
            "'{arg}' matches {} models; be more specific",
            matches.len()
        )),
    }
}

/// Switch the REPL to a picked model, routing the request to the
/// provider that actually serves it. A catalog entry carries both its
/// provider and the gateway id the provider accepts; picking one must
/// change the model string, the base URL, the API key and the live
/// backend together - changing only the string is the HTTP 400
/// "Invalid Model Name" bug (the old code sent OpenRouter-style ids to
/// the Experiential gateway, and experiential ids with the wrong shape).
/// Returns the line to print, or an error to print; on error the switch
/// is refused and the previous model and backend stay untouched.
fn apply_model_switch(
    cfg: &mut crate::settings::ResolvedConfig,
    agent: &mut zen_agent::Agent,
    recorder: Option<&Arc<crate::eventlog::Recorder>>,
    home: Option<&Path>,
    home_api_key: &Option<String>,
    id: &str,
) -> Result<String, String> {
    let Some(entry) = crate::models::catalog_entry(id) else {
        // Not a catalog entry (e.g. a probed pool id, already a gateway
        // id for the current provider): only the string changes.
        agent.config.model = id.to_string();
        cfg.model = id.to_string();
        return Ok(format!("model set to {id}"));
    };
    let Some(base_url) = crate::models::provider_base_url(entry.provider) else {
        agent.config.model = entry.gateway_id.to_string();
        cfg.model = entry.gateway_id.to_string();
        return Ok(format!(
            "model set to {} (sending {})",
            id, entry.gateway_id
        ));
    };
    let old_base_url = cfg.base_url.clone();
    let old_key = cfg.api_key.clone();
    // Route model + base URL + key together (shared with the slash
    // `model:` pin and the headless pin path). A refused route (the
    // provider key is unset) leaves everything untouched.
    if let Err(e) = crate::settings::route_to_catalog_entry(cfg, entry, home_api_key) {
        return Err(format!(
            "cannot switch to {id}: {e}; keeping {}",
            agent.config.model
        ));
    }
    if cfg.base_url != old_base_url || cfg.api_key != old_key {
        agent.llm = crate::eventlog::wrap(cfg.gated_llm_client(agent.sink.clone(), home), recorder);
    }
    agent.config.model = entry.gateway_id.to_string();
    // Keep cfg.model in step with what is actually sent, so anything
    // that re-reads it later sees the effective model (review F7).
    cfg.model = entry.gateway_id.to_string();
    Ok(format!(
        "model set to {} (sending {} to {})",
        id, entry.gateway_id, base_url
    ))
}

async fn handle_slash(
    line: &str,
    cli: &Cli,
    cwd: &Path,
    home: Option<&Path>,
    cfg: &mut crate::settings::ResolvedConfig,
    repl: &mut ReplState<'_>,
    agent: &mut zen_agent::Agent,
) -> Result<LineAction> {
    let session_id = repl.session_id.clone();
    let session_id = session_id.as_str();
    let mut it = line.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("").to_string();
    let arg = it.next().unwrap_or("").trim().to_string();

    match cmd.as_str() {
        "/exit" | "/quit" => return Ok(LineAction::Exit),
        "/help" => {
            println!("Built-in commands:");
            let catalog = commands::slash_command_catalog();
            for group in commands::HELP_GROUP_ORDER {
                println!("{}", repl.theme.paint(repl_polish::ColorRole::Dim, group));
                for (name, desc) in &catalog {
                    if commands::help_group(name) == *group {
                        println!("  {:<16} {}", name, desc);
                    }
                }
            }

            let custom = zen_ext::discover_slash_commands(cwd, home);
            if !custom.is_empty() {
                println!("Custom commands and skills:");
                for c in &custom {
                    match &c.description {
                        Some(d) => println!("  /{}  {}", c.name, d),
                        None => println!("  /{}", c.name),
                    }
                }
            }
        }
        "/clear" => {
            crate::agent_run::run_session_end(&agent.hooks, session_id, cwd, cfg, "clear").await;
            agent.set_messages(vec![]);

            let new_id = crate::session::new_session_id();
            if cli.ephemeral {
                agent.ctx.session_dir = None;
                *repl.writer = crate::session::TranscriptWriter::ephemeral(&new_id, cwd);
            } else {
                let path = repl
                    .writer
                    .path
                    .as_ref()
                    .expect("non-ephemeral writer always has a path")
                    .with_file_name(format!("{}.jsonl", new_id));
                agent.ctx.session_dir = Some(crate::session::session_scratch_dir(&path));
                if let Some(r) = &repl.recorder {
                    r.set_path(path.clone());
                }
                *repl.writer = crate::session::TranscriptWriter::new(path, &new_id, cwd);
            }
            *repl.session_id = new_id.clone();
            agent.ctx.session_id = new_id.clone();
            repl.goal = None;
            repl_session_start(agent, &new_id, cwd, cfg, "clear").await;
            println!("context cleared, new session: {}", new_id);
        }
        "/compact" => {
            let focus = if arg.is_empty() {
                None
            } else {
                Some(arg.as_str())
            };
            match agent.compact(focus, "manual").await {
                Ok(()) => {
                    println!("context compacted");
                    repl_session_start(agent, session_id, cwd, cfg, "compact").await;
                }
                Err(e) => eprintln!("compact failed: {}", e),
            }
        }
        "/model" => {
            let (rows, stale_warning) = available_model_rows(home, &cfg.base_url);
            if let Some(w) = &stale_warning {
                eprintln!("{w}");
            }
            if arg.is_empty() {
                print!(
                    "{}",
                    format_model_list(&agent.config.model, &rows, repl.auto_model)
                );
            } else if crate::models::is_auto_model(&arg) {
                // Auto mode back on (Leader tin 7962): the current
                // provider's best free model.
                match crate::models::provider_for_base_url(&cfg.base_url)
                    .and_then(crate::models::provider_default_model)
                {
                    Some(entry) => {
                        let id = entry.id.to_string();
                        match apply_model_switch(
                            cfg,
                            agent,
                            repl.recorder.as_ref(),
                            home,
                            &repl.home_api_key,
                            &id,
                        ) {
                            Ok(msg) => {
                                repl.auto_model = true;
                                report_model_switch(
                                    cwd,
                                    true,
                                    &cfg.model,
                                    &cfg.base_url,
                                    &format!("{msg} (auto)"),
                                );
                            }
                            Err(e) => eprintln!("{e}"),
                        }
                    }
                    None => {
                        eprintln!("auto needs a known provider; the current base URL is custom")
                    }
                }
            } else {
                match pick_model(&arg, &rows) {
                    Ok(id) => {
                        match apply_model_switch(
                            cfg,
                            agent,
                            repl.recorder.as_ref(),
                            home,
                            &repl.home_api_key,
                            &id,
                        ) {
                            Ok(msg) => {
                                // A pinned model turns auto off.
                                repl.auto_model = false;
                                report_model_switch(cwd, false, &cfg.model, &cfg.base_url, &msg);
                            }
                            Err(e) => eprintln!("{e}"),
                        }
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
        }
        "/provider" => {
            // Switch provider (Leader tin 7962): the model resets to
            // the new provider's best free model, and auto turns on.
            // apply_model_switch moves base URL, key and backend
            // together, so the switch cannot strand a model on the
            // wrong gateway (the PR #71 HTTP 400 bug).
            if arg.is_empty() {
                println!("usage: /provider <experientiallabs|openrouter> — switch provider; model resets to that provider's best free model");
            } else {
                match crate::models::provider_default_model(&arg) {
                    Some(entry) => {
                        let id = entry.id.to_string();
                        match apply_model_switch(
                            cfg,
                            agent,
                            repl.recorder.as_ref(),
                            home,
                            &repl.home_api_key,
                            &id,
                        ) {
                            Ok(msg) => {
                                repl.auto_model = true;
                                // The provider switch also moves the
                                // base URL; persist it alongside `auto`
                                // so a restart lands on the same
                                // provider and its best model.
                                report_model_switch(
                                    cwd,
                                    true,
                                    &cfg.model,
                                    &cfg.base_url,
                                    &format!("{msg} (auto)"),
                                );
                            }
                            Err(e) => eprintln!("{e}"),
                        }
                    }
                    None => eprintln!("unknown provider '{arg}' (experientiallabs, openrouter)"),
                }
            }
        }
        "/btw" => {
            if arg.is_empty() {
                println!("usage: /btw <question> \u{2014} ask a quick side question; the main conversation is untouched");
            } else {
                let model = cfg
                    .btw_model
                    .clone()
                    .unwrap_or_else(|| agent.config.model.clone());
                match crate::btw::run_btw(&agent.llm, &model, &arg).await {
                    Ok(answer) => {
                        if answer.trim().is_empty() {
                            println!("(no answer)");
                        } else {
                            println!("{}", answer.trim_end());
                        }
                    }
                    Err(e) => eprintln!("btw failed: {:#}", e),
                }
            }
        }
        "/cost" => println!(
            "{}",
            commands::cost_report(&agent.usage, &agent.config.model, &cfg.base_url)
        ),
        "/quota" => {
            let store = home.map(zen_agent::RateGate::default_store_path);
            let key_url = zen_agent::rategate::openrouter_key_url(&cfg.base_url);
            let key = cfg.api_key();
            println!(
                "{}",
                zen_agent::rategate::quota_command_report(
                    key_url.as_deref(),
                    key,
                    store.as_deref()
                )
                .await
            );
        }
        "/commit" => {
            let files = crate::git::changed_files(cwd);
            if files.is_empty() {
                println!("nothing to commit (working tree clean)");
            } else {
                println!("Changed files:");
                for (status, path) in &files {
                    println!("  {} {}", status, path);
                }
                let (message, confirm) = if let Some(m) = arg.strip_prefix("-m ") {
                    (m.trim().trim_matches('"').to_string(), false)
                } else if arg.trim() == "-y" {
                    (crate::git::suggest_commit_message(&files), false)
                } else {
                    (crate::git::suggest_commit_message(&files), true)
                };
                if message.trim().is_empty() {
                    eprintln!("usage: /commit [-y | -m \"message\"]");
                } else {
                    println!("Message: {}", message);
                    let do_commit = !confirm || prompt_yes_no("Commit these changes?");
                    if do_commit {
                        match crate::git::stage_and_commit(cwd, &message) {
                            Ok(summary) => println!("{}", summary),
                            Err(e) => eprintln!("commit failed: {}", e),
                        }
                    } else {
                        println!("commit cancelled");
                    }
                }
            }
        }
        "/worktree" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            match parts.as_slice() {
                [] => {
                    let wts = crate::git::list_worktrees(cwd);
                    if wts.is_empty() {
                        println!("no worktrees (or not a git repo)");
                    } else {
                        for (p, b, c) in wts {
                            println!("{} [{}] {}", p, b, &c[..7.min(c.len())]);
                        }
                    }
                }
                ["rm", name] if crate::git::valid_worktree_name(name) => {
                    let wt_path = cwd.join(format!(
                        "../{}-{}",
                        cwd.file_name().and_then(|n| n.to_str()).unwrap_or("repo"),
                        name
                    ));
                    match crate::git::remove_worktree(cwd, &wt_path) {
                        Ok(()) => println!("removed worktree {}", wt_path.display()),
                        Err(e) => eprintln!("failed: {}", e),
                    }
                }
                [name] if crate::git::valid_worktree_name(name) && *name != "rm" => {
                    let repo = cwd.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
                    let wt_path = cwd.join(format!("../{}-{}", repo, name));
                    match crate::git::create_worktree(cwd, &wt_path, name) {
                        Ok(p) => println!("created worktree {} on branch {}\ncd to it and run zen there for an isolated session", p.display(), name),
                        Err(e) => eprintln!("failed: {}", e),
                    }
                }
                _ => eprintln!(
                    "usage: /worktree [name] | /worktree rm <name>  (name: letters, digits, - _ .)"
                ),
            }
        }
        "/context" => println!(
            "{}",
            commands::context_report(
                &agent.config.system_prompt,
                &zen_ext::memory_prompt(cwd, home),
                &agent.tools_for_request(),
                agent.messages(),
                agent.config.context_window,
                agent.last_prompt_tokens,
            )
        ),
        "/expand" => {
            let parsed = if arg.is_empty() {
                Ok(None)
            } else {
                arg.parse::<usize>()
                    .map(Some)
                    .map_err(|_| "usage: /expand [card-number]")
            };
            match parsed {
                Ok(n) => println!("{}", repl.progress.toggle_expand(n)),
                Err(msg) => println!("{msg}"),
            }
        }
        "/undo" => match arg.as_str() {
            "" | "--force" => undo_last_turn(repl, agent, arg == "--force"),
            _ => eprintln!("usage: /undo [--force]"),
        },
        "/diff" => {
            let mut staged = false;
            let mut stat = false;
            let mut turn = false;
            let mut unknown: Option<&str> = None;
            for tok in arg.split_whitespace() {
                match tok {
                    "--staged" => staged = true,
                    "--stat" => stat = true,
                    "--turn" | "--last" => turn = true,
                    _ => unknown = Some(tok),
                }
            }
            if let Some(flag) = unknown {
                eprintln!("unknown flag '{flag}'; usage: /diff [--staged] [--stat] [--turn]");
            } else if turn {
                match repl
                    .checkpoints
                    .diff_last_turn(&zen_tools::canonicalize_lenient(cwd))
                {
                    None => println!("(no turn has changed files with Write/Edit/MultiEdit yet)"),
                    Some((t, diff)) => {
                        println!("Turn {} ({}): {} file(s)", t.id, t.label, t.entries.len());
                        print!("{diff}");
                    }
                }
            } else if !crate::git::is_git_repo(cwd) {
                eprintln!(
                    "not a git repository: {}\n/diff needs a git repo; read files directly or run `!git ...` instead.",
                    cwd.display()
                );
            } else {
                let mode = if stat {
                    crate::git::DiffMode::Stat
                } else if staged {
                    crate::git::DiffMode::Staged
                } else {
                    crate::git::DiffMode::WorkingTree
                };
                let color = std::io::IsTerminal::is_terminal(&std::io::stdout())
                    && !crate::repl_polish::Theme::colors_disabled();
                match crate::git::git_diff(cwd, mode, color) {
                    Some(out) if !out.trim().is_empty() => print!("{out}"),
                    _ => println!("(no changes)"),
                }

                if !staged && !stat {
                    match crate::git::untracked_files(cwd) {
                        Some(u) if !u.is_empty() => {
                            println!("\nUntracked files ({}):", u.len());
                            for f in u {
                                println!("  {f}");
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "/terse" => {
            let sp = std::mem::take(&mut agent.config.system_prompt);
            if crate::system_prompt::has_terse_style(&sp) {
                agent.config.system_prompt = crate::system_prompt::without_terse_style(&sp);
                println!("terse mode off");
            } else {
                agent.config.system_prompt = crate::system_prompt::with_terse_style(&sp);
                println!("terse mode on");
            }
        }
        "/add-dir" => {
            if arg.is_empty() {
                eprintln!("usage: /add-dir <path>");
            } else {
                let p = cwd.join(&arg);
                match p.canonicalize() {
                    Ok(dir) if dir.is_dir() => {
                        if !agent.ctx.extra_dirs.contains(&dir) {
                            agent.ctx.extra_dirs.push(dir.clone());
                        }
                        println!("added working directory {}", dir.display());
                    }
                    _ => eprintln!("not a directory: {}", p.display()),
                }
            }
        }
        "/done" => {
            let todos = agent
                .ctx
                .todos
                .lock()
                .map_err(|e| anyhow::anyhow!("todos lock poisoned: {e}"))?
                .clone();
            let checks = crate::doneguard::run_done_checks(cwd, &todos).await;
            let changed = crate::doneguard::changed_files(cwd);
            let violations = repl.scope.violations(&changed);
            let yagni = crate::doneguard::yagni_scan(cwd);
            println!(
                "{}",
                crate::doneguard::format_report(
                    cwd,
                    &crate::doneguard::DoneReport {
                        checks,
                        changed,
                        scope: repl.scope.declared.clone(),
                        scope_violations: violations,
                        yagni,
                    }
                )
            );
        }
        "/scope" => {
            if arg == "clear" {
                repl.scope.clear();
                agent.config.done_scope.clear();
                println!("scope cleared");
            } else if arg.is_empty() {
                if repl.scope.declared.is_empty() {
                    println!(
                        "(no declared scope - /scope <glob>... to set one, e.g. /scope src/**)"
                    );
                } else {
                    println!("scope: {}", repl.scope.declared.join(" "));
                    let changed = crate::doneguard::changed_files(cwd);
                    let violations = repl.scope.violations(&changed);
                    if violations.is_empty() {
                        println!("all {} changed file(s) within scope", changed.len());
                    } else {
                        println!("outside scope: {}", violations.join(", "));
                    }
                }
            } else {
                repl.scope
                    .set(arg.split_whitespace().map(|s| s.to_string()).collect());
                repl.scope_seeded = true;
                agent.config.done_scope = repl.scope.declared.clone();
                println!("scope set: {}", repl.scope.declared.join(" "));
            }
        }
        "/export" => {
            let file = if arg.is_empty() {
                format!("zen-session-{}.md", session_id)
            } else {
                arg.clone()
            };
            let path = cwd.join(&file);
            match std::fs::write(
                &path,
                commands::transcript_markdown(session_id, agent.messages()),
            ) {
                Ok(()) => println!("conversation exported to {}", path.display()),
                Err(e) => eprintln!("export failed: {}", e),
            }
        }
        "/rename" => {
            if arg.is_empty() {
                eprintln!("usage: /rename <name>");
            } else {
                match repl.writer.path.as_ref() {
                    Some(p) => match crate::session::append_title(p, session_id, &arg) {
                        Ok(()) => println!("session renamed to {}", arg),
                        Err(e) => eprintln!("rename failed: {}", e),
                    },
                    None => println!("session rename is unavailable in ephemeral mode"),
                }
            }
        }
        "/loop" => match repl_polish::parse_loop_args(&arg) {
            Ok((n, prompt)) => {
                for i in 1..=n {
                    println!(
                        "{}",
                        repl.theme
                            .paint(repl_polish::ColorRole::Accent, &format!("[loop {i}/{n}]"))
                    );
                    let result =
                        run_turn_interruptible(agent, &prompt, repl.interrupts.as_deref_mut())
                            .await;
                    let failed = result.as_ref().map(|r| r.is_error).unwrap_or(true);
                    print_turn(result);
                    if failed {
                        eprintln!("loop stopped: iteration {i} failed");
                        break;
                    }
                }
            }
            Err(e) => eprintln!("{e}"),
        },
        "/goal" => {
            if arg.is_empty() {
                match &repl.goal {
                    Some(g) => println!("session goal: {g}"),
                    None => println!("(no session goal; set one with /goal <text>)"),
                }
            } else if arg == "clear" {
                repl.goal = None;
                println!("session goal cleared");
            } else {
                repl.goal = Some(arg.clone());
                println!("session goal set: {arg}");
            }
        }
        "/personality" => {
            if arg.is_empty() {
                println!("personality: {}", repl.personality.name());
            } else {
                match repl_polish::Personality::parse(&arg) {
                    Some(p) => {
                        repl.personality = p;
                        agent.config.system_prompt =
                            format!("{}{}", repl.base_system_prompt, p.system_suffix());
                        println!("personality set to {}", p.name());
                    }
                    None => eprintln!("unknown personality {arg:?} (concise | normal | detailed)"),
                }
            }
        }
        "/vim" => {
            repl.vim_mode = !repl.vim_mode;
            if repl.vim_mode {
                println!("vim keybindings: on (vi editing at the prompt on a terminal)");
            } else {
                println!("vim keybindings: off");
            }
        }
        "/theme" => {
            if arg.is_empty() {
                let note = if repl_polish::Theme::colors_disabled() {
                    " (NO_COLOR is set: colors disabled)"
                } else {
                    ""
                };
                println!("theme: {}{}", repl.theme.name(), note);
            } else {
                match repl_polish::Theme::parse(&arg) {
                    Some(t) => {
                        repl.theme = t;
                        if repl_polish::Theme::colors_disabled() && t != repl_polish::Theme::None {
                            println!(
                                "theme set to {}; note: NO_COLOR is set, colors stay disabled",
                                t.name()
                            );
                        } else {
                            println!("theme set to {}", t.name());
                        }
                    }
                    None => eprintln!("unknown theme {arg:?} (dark | light | none)"),
                }
            }
        }
        "/review" | "/init" => {
            let prompt = if cmd == "/review" {
                commands::review_prompt(&arg)
            } else {
                init_prompt_with_import_offer(cwd)
            };
            let usage_before = agent.usage.clone();
            let result =
                run_turn_interruptible(agent, &prompt, repl.interrupts.as_deref_mut()).await;
            record_turn_cost(home, repl.session_id.as_str(), cfg, agent, &usage_before);
            print_turn(result);
        }
        "/map" => match crate::repomap::repo_map(cwd, cli.map_tokens) {
            Some(map) => println!("{}", map.text()),
            None => println!("no repo map (no Rust files found, or --repomap-budget 0)"),
        },
        "/map-refresh" => match crate::repomap::repo_map(cwd, cli.map_tokens) {
            Some(map) => {
                println!("{}", map.text());
                if map.truncated {
                    println!("[increase --repomap-budget to see more]");
                }
            }
            None => println!("no repo map (no Rust files found, or --repomap-budget 0)"),
        },
        "/deep-research" => {
            use crate::deep_research as dr;
            match dr::parse_deep_research_args(&arg) {
                Err(e) => eprintln!("{e}"),
                Ok(mut opts) => {
                    if opts.max_budget_usd.is_none() {
                        opts.max_budget_usd = dr::resolve_budget_usd(None);
                    }
                    if let Err(e) = dr::check_budget_usd(opts.max_budget_usd, opts.max_subagents) {
                        eprintln!("deep-research: {e}");
                        return Ok(LineAction::Continue);
                    }
                    let Some(task) = agent.tools.get("Task") else {
                        eprintln!(
                            "deep-research needs the Task tool (sub-agents are disabled in this session)"
                        );
                        return Ok(LineAction::Continue);
                    };
                    let angles = dr::research_angles(opts.max_subagents);
                    println!(
                        "deep-research: spawning {} researcher(s) on \"{}\"",
                        angles.len(),
                        opts.topic
                    );
                    let ctx = agent.ctx.clone();
                    let model = agent.config.model.clone();
                    let findings = dr::fan_out_research(
                        &task,
                        &ctx,
                        &opts.topic,
                        &opts,
                        &(|s: String| println!("deep-research {s}")),
                    )
                    .await;
                    let ok_count = findings.iter().filter(|f| f.ok).count();
                    println!(
                        "deep-research: {ok_count}/{} researchers finished ok; synthesizing…",
                        findings.len()
                    );
                    let synth_prompt = dr::synthesis_prompt(&opts.topic, &findings);
                    let turn = run_turn_interruptible(
                        agent,
                        &synth_prompt,
                        repl.interrupts.as_deref_mut(),
                    )
                    .await;
                    let report = match turn {
                        Some(r) if !r.is_error && !r.result.trim().is_empty() => {
                            dr::render_report(&opts.topic, &model, &opts, &findings, &r.result)
                        }
                        Some(r) => dr::render_fallback_report(
                            &opts.topic,
                            &model,
                            &opts,
                            &findings,
                            &format!(
                                "synthesis turn failed: {}",
                                r.result.chars().take(160).collect::<String>()
                            ),
                        ),
                        None => dr::render_fallback_report(
                            &opts.topic,
                            &model,
                            &opts,
                            &findings,
                            "interrupted",
                        ),
                    };
                    println!("\n{report}");
                }
            }
        }
        "/pr_comments" => {
            let pr_arg = if arg.is_empty() {
                None
            } else {
                Some(arg.as_str())
            };
            match crate::github::fetch_pr_comments(cwd, pr_arg) {
                Ok(fetched) => {
                    println!("{}", crate::github::format_comments_text(&fetched));
                    if fetched.total() > 0 {
                        offer_fix_turn(
                            &crate::github::pr_fix_prompt(&fetched),
                            "these comments",
                            cfg,
                            repl,
                            agent,
                        )
                        .await;
                    }
                }
                Err(e) => eprintln!("pr_comments failed: {:#}", e),
            }
        }
        "/ci-fix" => match crate::github::fetch_ci_status(cwd) {
            Ok(status) => {
                if status.failing.is_empty() {
                    println!("all CI checks passing on branch {}", status.branch);
                } else {
                    println!("{}", crate::github::format_ci_text(&status));
                    offer_fix_turn(
                        &crate::github::ci_fix_prompt(&status),
                        "these failures",
                        cfg,
                        repl,
                        agent,
                    )
                    .await;
                }
            }
            Err(e) => eprintln!("ci-fix failed: {:#}", e),
        },
        "/status" => {
            println!("model: {}", agent.config.model);
            println!("base_url: {}", cfg.base_url);
            println!("permission_mode: {}", cfg.permission_mode);
            println!("session_id: {}", session_id);
            println!("goal: {}", repl.goal.as_deref().unwrap_or("(none)"));
            println!("personality: {}", repl.personality.name());
            println!("theme: {}", repl.theme.name());
            println!("vim_mode: {}", repl.vim_mode);
            println!("ephemeral: {}", cli.ephemeral);
            if cfg.loaded_sources.is_empty() {
                println!("settings: (none)");
            } else {
                for s in &cfg.loaded_sources {
                    println!("settings: {}", s);
                }
            }
        }
        "/plan" => {
            if agent.config.permission_mode == zen_core::PermissionMode::Plan {
                let restore = repl.pre_plan_mode;
                crate::planmode::set_permission_mode(
                    &mut agent.config,
                    restore,
                    &repl.base_system_prompt,
                );
                repl.pending_plan = None;
                println!("plan mode OFF - permission mode: {}", restore.as_str());
            } else {
                repl.pre_plan_mode = agent.config.permission_mode;
                crate::planmode::set_permission_mode(
                    &mut agent.config,
                    zen_core::PermissionMode::Plan,
                    &repl.base_system_prompt,
                );
                repl.pending_plan = None;
                println!(
                    "plan mode ON - read-only. Research with the available tools, then present \
                     a plan ending with `ExitPlanMode`. /plan again to exit without approving."
                );
            }
        }
        "/permissions" => {
            println!("allow: {:?}", cfg.permissions.allow);
            println!("ask:   {:?}", cfg.permissions.ask);
            println!("deny:  {:?}", cfg.permissions.deny);
            println!("mode:  {}", cfg.permission_mode);
        }
        "/config" => {
            if arg.is_empty() {
                println!("model: {}", agent.config.model);
                println!("permission_mode: {}", cfg.permission_mode);
                println!("base_url: {}", cfg.base_url);
                println!(
                    "autoCompact: {}",
                    cfg.auto_compact
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "true".into())
                );
                println!("set a value with /config key=value (saved to .zen/settings.local.json)");
            } else if let Some((k, v)) = arg.split_once('=') {
                match save_local_setting(cwd, k.trim(), v.trim()) {
                    Ok(p) => println!(
                        "saved {} = {} to {} (applies to new sessions)",
                        k.trim(),
                        v.trim(),
                        p.display()
                    ),
                    Err(e) => eprintln!("could not save setting: {}", e),
                }
            } else {
                eprintln!("usage: /config key=value");
            }
        }
        "/memory" => {
            for (p, _) in zen_ext::memory_files(cwd, home) {
                println!("{}", p.display());
            }
        }
        "/memories" => {
            println!("{}", zen_ext::memories_report(cwd, home));
        }
        "/todos" => {
            let todos = agent
                .ctx
                .todos
                .lock()
                .map_err(|e| anyhow::anyhow!("todos lock poisoned: {e}"))?
                .clone();
            if todos.is_empty() {
                println!("(no todos)");
            } else {
                println!("● Todos");
                for t in todos {
                    let box_char = if t.status == "completed" {
                        "☑"
                    } else {
                        "□"
                    };
                    println!("└ {box_char} {}", t.content);
                }
            }
        }
        "/agents" => {
            let agents = zen_mcp::load_agents(cwd, home, cli.agents.as_deref());
            for a in agents {
                println!("{}: {}", a.name, a.description);
            }
        }
        "/mcp" => println!("{}", commands::mcp_report(&repl.mcp_status)),
        "/hooks" => {
            if cfg.hooks.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                println!("(no hooks)");
            } else {
                println!("{}", serde_json::to_string_pretty(&cfg.hooks)?);
            }
        }
        "/skills" => {
            let skills = zen_ext::discover_skills(cwd, home);
            if skills.is_empty() {
                println!("(no skills)");
            } else {
                for s in skills {
                    println!("{}: {}", s.name, s.description);
                }
            }
        }
        "/resume" | "/continue" => {
            if cli.ephemeral {
                println!("cannot resume sessions in ephemeral mode (nothing is persisted)");
                return Ok(LineAction::Continue);
            }
            let home_dir = home
                .map(Path::to_path_buf)
                .unwrap_or_else(std::env::temp_dir);
            let sessions = crate::session::list_sessions_for_cwd(&home_dir, cwd);
            if arg.is_empty() {
                if sessions.is_empty() {
                    println!("(no previous sessions)");
                } else {
                    println!("Available sessions (/resume <id|name>):");
                    for (id, path) in sessions {
                        match crate::session::session_title(&path) {
                            Some(t) => println!("  {}  {}", id, t),
                            None => println!("  {}", id),
                        }
                    }
                }
            } else {
                let found = crate::session::find_session_path(&home_dir, &arg).or_else(|| {
                    sessions
                        .into_iter()
                        .find(|(_, p)| {
                            crate::session::session_title(p).as_deref() == Some(arg.as_str())
                        })
                        .map(|(_, p)| p)
                });
                match found {
                    Some(path) => {
                        resume_into(path, cwd, agent, repl)?;
                    }
                    None => eprintln!("no session found: {}", arg),
                }
            }
        }
        other => {
            let name = other.trim_start_matches('/');

            if zen_ext::find_slash_command(cwd, home, name).is_some() {
                let (expanded, slash_cmd) = crate::agent_run::expand_slash_prompt_cmd(
                    cli, cwd, home, line, cfg, session_id,
                )?;

                let saved_model = agent.config.model.clone();
                let saved_perms = agent.permissions.clone();
                // A slash `model:` pin routes for this turn only; the
                // startup route is restored after the turn (below).
                let saved_base_url = cfg.base_url.clone();
                let saved_api_key = cfg.api_key.clone();
                let saved_cfg_model = cfg.model.clone();
                let saved_llm = agent.llm.clone();
                if let Some(c) = &slash_cmd {
                    if let Some(m) = c
                        .model
                        .as_ref()
                        .filter(|m| !m.is_empty() && *m != "inherit")
                    {
                        // Route the pin like /model for this turn only:
                        // model + base URL + key + live backend together.
                        // A refused route (the provider key is unset)
                        // prints the reason and skips the turn instead
                        // of sending the id to the wrong gateway.
                        if let Err(e) = apply_model_switch(
                            cfg,
                            agent,
                            repl.recorder.as_ref(),
                            home,
                            &repl.home_api_key,
                            m,
                        ) {
                            eprintln!("{e}");
                            return Ok(LineAction::Continue);
                        }
                    }
                    if !c.allowed_tools.is_empty() {
                        agent.permissions = crate::agent_run::build_permissions_with(
                            cwd,
                            cfg,
                            cli,
                            &c.allowed_tools,
                        );
                    }
                }
                let usage_before = agent.usage.clone();
                let result =
                    run_turn_interruptible(agent, &expanded, repl.interrupts.as_deref_mut()).await;
                record_turn_cost(home, repl.session_id.as_str(), cfg, agent, &usage_before);
                cfg.base_url = saved_base_url;
                cfg.api_key = saved_api_key;
                cfg.model = saved_cfg_model;
                agent.llm = saved_llm;
                agent.config.model = saved_model;
                agent.permissions = saved_perms;
                print_turn(result);
            } else {
                eprintln!("unknown command: {}", other);
            }
        }
    }
    Ok(LineAction::Continue)
}

fn save_local_setting(cwd: &Path, key: &str, value: &str) -> Result<std::path::PathBuf> {
    save_local_settings(cwd, &[(key, value)])
}

/// Write several keys to `.zen/settings.local.json` in a single
/// read-modify-write, so a `/model` switch records the model and its
/// base URL together (neither lands without the other).
fn save_local_settings(cwd: &Path, entries: &[(&str, &str)]) -> Result<std::path::PathBuf> {
    for (key, _) in entries {
        if key.is_empty() {
            anyhow::bail!("empty key");
        }
    }
    let path = cwd.join(".zen").join("settings.local.json");
    let mut obj = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| anyhow::anyhow!("{} is not valid JSON: {}", path.display(), e))?,
        Err(_) => serde_json::json!({}),
    };
    let map = obj
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a JSON object", path.display()))?;
    for &(key, value) in entries {
        let parsed =
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.into()));
        map.insert(key.to_string(), parsed);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(&obj)? + "\n")?;
    Ok(path)
}
/// Record a successful `/model` or `/provider` switch in
/// `.zen/settings.local.json`, so the next `zen` run starts on it.
/// `auto` saves the literal `auto` (auto mode stays on); otherwise the
/// gateway id in `model` is saved. The base URL is saved alongside the
/// model: without it a cross-provider switch breaks after restart (the
/// saved model reaches the wrong gateway), and `auto` silently loses
/// its provider. A write failure is reported but never refuses the
/// in-memory switch.
fn save_model_choice(
    cwd: &Path,
    auto: bool,
    model: &str,
    base_url: &str,
) -> Result<std::path::PathBuf> {
    save_local_settings(
        cwd,
        &[
            ("model", if auto { "auto" } else { model }),
            ("baseUrl", base_url),
        ],
    )
}

fn report_model_switch(cwd: &Path, auto: bool, model: &str, base_url: &str, msg: &str) {
    match save_model_choice(cwd, auto, model, base_url) {
        Ok(path) => println!("{msg} (saved to {})", path.display()),
        Err(e) => eprintln!("{msg} (could not save model choice: {e})"),
    }
}

fn build_repl_config(
    cli: &Cli,
    cfg: &crate::settings::ResolvedConfig,
    cwd: &Path,
) -> zen_agent::AgentConfig {
    let mut config = zen_agent::AgentConfig {
        model: cfg.model.clone(),
        fallback_models: cfg.fallback_models.clone(),
        system_prompt: crate::build_system_prompt(cli, cwd, None).unwrap_or_default(),
        max_turns: cfg.max_turns.max(1),
        permission_mode: crate::agent_run::core_mode(cfg.permission_mode),
        tool_timeouts: cfg.tool_timeouts.clone(),
        max_continuations: cfg.max_continuations,
        tool_bundle: crate::agent_run::resolve_tool_bundle(cli, cfg),
        max_parallel_tool_calls: cfg.max_parallel_tool_calls,
        ..Default::default()
    };
    // Bug hunt B2, direction (a): the window tracks the catalog model
    // and the threshold tracks the window (80%), instead of the
    // `AgentConfig::default` 128k / 100k pair.
    let context_window = crate::models::effective_context_window(cfg.context_window, &cfg.model);
    config.context_window = context_window;
    config.auto_compact_threshold = crate::agent_run::resolve_auto_compact_threshold(
        cli.autocompact.as_deref(),
        cfg.auto_compact.as_ref(),
        context_window,
    );
    crate::agent_run::apply_verify_settings(&mut config, cfg, true);
    config.budget = crate::budget::resolve_budget(
        cli.max_budget_usd,
        cli.budget_warn_at,
        cfg.max_budget_usd,
        cfg.budget_warn_at,
    );
    config
}

fn run_shell_passthrough(
    cmd: &str,
    cwd: &Path,
    cfg: &crate::settings::ResolvedConfig,
    agent: &mut zen_agent::Agent,
) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        eprintln!("usage: !<shell command>");
        return;
    }
    match crate::mentions::decide_shell(
        &agent.permissions,
        agent.prompter.as_ref(),
        &agent.ctx,
        cmd,
    ) {
        crate::mentions::ShellDecision::Denied(reason) => {
            eprintln!("!{}: {}", cmd, reason);
        }
        crate::mentions::ShellDecision::Run => {
            let out = crate::mentions::run_shell_command(cmd, cwd, &cfg.env);
            print!("{}", out.combined);
            if !out.success {
                eprintln!("(exit {})", out.exit_code);
            }
            agent.messages.push(zen_core::ChatMessage::user(
                crate::mentions::shell_passthrough_record(cmd, &out),
            ));
        }
    }
}

fn undo_last_turn(repl: &mut ReplState<'_>, agent: &mut zen_agent::Agent, force: bool) {
    use zen_tools::checkpoint::UndoError;
    match repl.checkpoints.undo(force) {
        Ok(r) => {
            let mut lines = Vec::new();
            for p in &r.restored {
                lines.push(format!("restored {}", p.display()));
            }
            for p in &r.removed {
                lines.push(format!("deleted {} (created in that turn)", p.display()));
            }
            for (p, why) in &r.skipped {
                lines.push(format!("NOT restored {} ({})", p.display(), why));
            }
            let head = if r.label.is_empty() {
                format!("Undid turn {}", r.turn_id)
            } else {
                format!("Undid turn {} \"{}\"", r.turn_id, r.label)
            };
            println!("{head}:");
            for l in &lines {
                println!("  {l}");
            }
            println!("(changes made through Bash are not undone)");
            agent.messages.push(zen_core::ChatMessage::user(format!(
                "[/undo] The user undid the file changes of an earlier turn ({}). Files are back \
                 to their content before that turn; read them again before editing:\n{}",
                if r.label.is_empty() {
                    format!("turn {}", r.turn_id)
                } else {
                    format!("\"{}\"", r.label)
                },
                lines.join("\n")
            )));
        }
        Err(e @ UndoError::Nothing) => println!("{e}"),
        Err(e) => eprintln!("{e}"),
    }
}

fn persist(
    writer: &mut crate::session::TranscriptWriter,
    before: &[zen_core::ChatMessage],
    agent: &zen_agent::Agent,
) {
    if let Err(e) = writer.persist_turn(before, agent.messages()) {
        eprintln!("warning: could not write transcript: {}", e);
    }
}

pub type ReplSink = Arc<dyn EventSink>;

fn record_turn_cost(
    home: Option<&Path>,
    session_id: &str,
    cfg: &crate::settings::ResolvedConfig,
    agent: &zen_agent::Agent,
    before: &zen_core::UsageStats,
) {
    let delta_in = agent.usage.input_tokens.saturating_sub(before.input_tokens);
    let delta_out = agent
        .usage
        .output_tokens
        .saturating_sub(before.output_tokens);
    crate::cost::record(
        home,
        session_id,
        &agent.config.model,
        &cfg.base_url,
        delta_in,
        delta_out,
    );
}

#[cfg(test)]
mod multiline_tests {
    use super::read_multiline;

    fn run(first: &str, rest: &[&str]) -> String {
        let mut it = rest.iter().map(|s| format!("{s}\n"));
        read_multiline(format!("{first}\n"), || it.next())
    }

    #[test]
    fn single_line_unchanged() {
        assert_eq!(run("hello", &[]), "hello");
    }

    #[test]
    fn backslash_continues() {
        assert_eq!(run("a \\", &["b \\", "c"]), "a \nb \nc");
    }

    #[test]
    fn triple_quote_block() {
        assert_eq!(
            run("\"\"\"line1", &["line2", "line3\"\"\""]),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn unterminated_block_ends_at_eof() {
        assert_eq!(run("\"\"\"x", &["y"]), "x\ny\n");
    }

    #[test]
    fn one_line_triple_quotes() {
        assert_eq!(run("\"\"\"hi\"\"\"", &[]), "hi");
    }
}

#[cfg(test)]
mod turn_summary_tests {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64) -> UsageStats {
        UsageStats {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
        }
    }

    #[test]
    fn done_line_names_cached_tokens_when_cache_reads_exist() {
        assert_eq!(
            format_turn_summary(1234, &usage(1000, 50, 800)),
            "Done in 1.2s - 1050 tokens used (800 from cache)"
        );
    }

    #[test]
    fn done_line_omits_cache_note_without_cache_reads() {
        assert_eq!(
            format_turn_summary(500, &usage(100, 50, 0)),
            "Done in 0.5s - 150 tokens used"
        );
    }

    #[test]
    fn cache_reads_are_part_of_the_total_not_added_twice() {
        // input_tokens already includes the 800 cached tokens, so the
        // total is 1000 + 50, not 1000 + 800 + 50.
        assert_eq!(
            format_turn_summary(100, &usage(1000, 50, 800)),
            "Done in 0.1s - 1050 tokens used (800 from cache)"
        );
    }
}

#[cfg(test)]
mod model_picker_tests {
    use super::*;

    fn rows() -> Vec<ModelRow> {
        vec![
            ModelRow {
                id: "deepseek-v4.1-flash".to_string(),
                provider: "experientiallabs".to_string(),
                short: "deepseek-v4.1-flash".to_string(),
                context_window: Some(1_000_000),
                latency_ms: Some(210),
            },
            ModelRow {
                id: "cohere/north-mini-code:free".to_string(),
                provider: "cohere".to_string(),
                short: "north-mini-code:free".to_string(),
                context_window: Some(131_072),
                latency_ms: Some(180),
            },
            ModelRow {
                id: "nvidia/nemotron-3-nano:free".to_string(),
                provider: "nvidia".to_string(),
                short: "nemotron-3-nano:free".to_string(),
                context_window: Some(131_072),
                latency_ms: None,
            },
        ]
    }

    #[test]
    fn model_list_marks_the_current_model() {
        let out = format_model_list("cohere/north-mini-code:free", &rows(), false);
        assert!(out.contains("Models — current: north-mini-code:free"));
        // Short name + provider + context + latency columns.
        assert!(out.contains("1. deepseek-v4.1-flash"));
        assert!(out.contains("experientiallabs"));
        assert!(out.contains("1M ctx"));
        assert!(out.contains("210ms"));
        // Current row gets the leading ▸ marker.
        assert!(out.contains("▸ 2. north-mini-code:free"));
        assert!(out.contains("3. nemotron-3-nano:free"));
        // Catalog rows (never probed) show — for latency.
        assert!(out.contains("—"));
        assert!(out.contains("/model 2, /model nemotron"));
    }

    #[test]
    fn pick_by_number_is_one_based() {
        assert_eq!(pick_model("1", &rows()).unwrap(), "deepseek-v4.1-flash");
        assert_eq!(
            pick_model("3", &rows()).unwrap(),
            "nvidia/nemotron-3-nano:free"
        );
        assert!(pick_model("0", &rows()).is_err());
        assert!(pick_model("4", &rows()).is_err());
    }

    #[test]
    fn catalog_rows_are_scoped_to_the_current_provider() {
        // No probe cache at home None, so the catalog fallback runs.
        let (experiential, _) = available_model_rows(None, crate::settings::DEFAULT_BASE_URL);
        assert_eq!(experiential.len(), 1);
        assert_eq!(experiential[0].id, "deepseek-v4.1-flash");
        let (openrouter, _) = available_model_rows(None, crate::models::OPENROUTER_BASE_URL);
        assert_eq!(openrouter.len(), 4);
        assert!(openrouter.iter().all(|r| r.id != "deepseek-v4.1-flash"));
        // A custom base has no known provider: nothing is excluded.
        let (custom, _) = available_model_rows(None, "https://example.invalid/v1");
        assert_eq!(custom.len(), 5);
        // Numbered picks follow the filtered list, so an id outside
        // the current provider is not pickable at all.
        assert!(pick_model("qwen/qwen3-coder:free", &experiential).is_err());
    }

    #[test]
    fn stale_probe_cache_rows_carry_a_warning() {
        let home = std::env::temp_dir().join(format!("zen_picker_home_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let probe = crate::models::ModelProbe {
            model: "probe/model-x".to_string(),
            provider: "probe".to_string(),
            context_window: Some(1000),
            free: true,
            latency_ms: 42,
            success: true,
            score: 1.0,
            last_error: None,
        };
        let mut cache = crate::models::ModelsCache {
            verified_at: (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
            base_url: crate::settings::DEFAULT_BASE_URL.to_string(),
            results: vec![probe.clone()],
        };
        crate::models::save_cache(Some(&home), &cache).unwrap();
        let (rows, warning) = available_model_rows(Some(&home), crate::settings::DEFAULT_BASE_URL);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "probe/model-x");
        let warning = warning.expect("stale cache must warn");
        assert!(warning.contains("zen models --refresh"), "{warning}");

        cache.verified_at = chrono::Utc::now().to_rfc3339();
        cache.results = vec![probe];
        crate::models::save_cache(Some(&home), &cache).unwrap();
        let (rows, warning) = available_model_rows(Some(&home), crate::settings::DEFAULT_BASE_URL);
        assert_eq!(rows.len(), 1);
        assert!(warning.is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pick_by_exact_id_wins_over_substring() {
        assert_eq!(
            pick_model("cohere/north-mini-code:free", &rows()).unwrap(),
            "cohere/north-mini-code:free"
        );
    }

    #[test]
    fn pick_by_id_is_case_insensitive() {
        // `/model DEEPSEEK-V4.1-FLASH` must pick the same entry as the
        // lowercase id (PR #74 review follow-up), and the canonical
        // catalog id is returned, not the user's casing.
        assert_eq!(
            pick_model("DEEPSEEK-V4.1-FLASH", &rows()).unwrap(),
            "deepseek-v4.1-flash"
        );
        assert_eq!(
            pick_model("CoHere/North-Mini-Code:Free", &rows()).unwrap(),
            "cohere/north-mini-code:free"
        );
        // Substring matching is case-insensitive too.
        assert_eq!(
            pick_model("NEMOTRON", &rows()).unwrap(),
            "nvidia/nemotron-3-nano:free"
        );
    }

    #[test]
    fn pick_by_unique_substring() {
        assert_eq!(
            pick_model("nemotron", &rows()).unwrap(),
            "nvidia/nemotron-3-nano:free"
        );
    }

    #[test]
    fn pick_rejects_ambiguous_and_unknown() {
        assert!(pick_model("free", &rows()).is_err());
        assert!(pick_model("does-not-exist", &rows()).is_err());
    }

    // Mock backend that records the model string each request
    // actually carries - the wire id, not the picker label.
    struct RecordingBackend {
        models: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl zen_core::LlmBackend for RecordingBackend {
        async fn chat(&self, req: zen_core::ChatRequest) -> anyhow::Result<zen_core::ChatResponse> {
            self.models.lock().unwrap().push(req.model);
            Ok(zen_core::ChatResponse {
                message: zen_core::ChatMessage::assistant(Some("ok".into()), None),
                usage: zen_core::Usage::default(),
                finish_reason: None,
            })
        }
    }

    struct AllowAll;

    impl zen_core::PermissionChecker for AllowAll {
        fn check(
            &self,
            _name: &str,
            _input: &serde_json::Value,
            _read_only: bool,
            _ctx: &zen_core::ToolContext,
        ) -> zen_core::PermissionDecision {
            zen_core::PermissionDecision::Allow
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_cfg() -> crate::settings::ResolvedConfig {
        use clap::Parser;
        // Caller holds ENV_LOCK; isolate from ambient model/base-url env
        // so the resolved config is the plain default one.
        let old_model = std::env::var("ZEN_MODEL").ok();
        let old_base = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        let cli = crate::cli::Cli::parse_from(["zen"]);
        let dir = std::env::temp_dir().join(format!("zen_model400_cfg_{}", uuid::Uuid::new_v4()));
        let home = std::env::temp_dir().join(format!("zen_model400_home_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let cfg = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&home).ok();
        match old_model {
            Some(v) => std::env::set_var("ZEN_MODEL", v),
            None => std::env::remove_var("ZEN_MODEL"),
        }
        match old_base {
            Some(v) => std::env::set_var("ZEN_BASE_URL", v),
            None => std::env::remove_var("ZEN_BASE_URL"),
        }
        cfg
    }

    fn test_agent(llm: std::sync::Arc<RecordingBackend>) -> zen_agent::Agent {
        let dir = std::env::temp_dir().join(format!("zen_model400_ctx_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = zen_core::ToolContext::new(dir, "test-session");
        let config = zen_agent::AgentConfig {
            model: "google/gemini-4-argon:free".to_string(),
            ..Default::default()
        };
        zen_agent::Agent::new(
            llm,
            zen_core::ToolRegistry::new(),
            std::sync::Arc::new(AllowAll),
            None,
            std::sync::Arc::new(zen_core::NoHooks),
            std::sync::Arc::new(zen_core::NullSink),
            ctx,
            config,
        )
    }

    #[tokio::test]
    async fn switch_to_experiential_model_sends_bare_slug() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        let old_llm = agent.llm.clone();

        let msg = apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &None,
            "deepseek-v4.1-flash",
        )
        .unwrap();
        assert!(msg.contains("deepseek-v4.1-flash"), "{msg}");
        assert_eq!(agent.config.model, "deepseek-v4.1-flash");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        // Same provider, same key: the backend is left alone.
        assert!(std::sync::Arc::ptr_eq(&old_llm, &agent.llm));

        // The request the agent would send carries the gateway id,
        // not the catalog id the gateway rejects with HTTP 400.
        // The env lock only guards setup; drop it before the await.
        drop(_env);
        let req = zen_core::ChatRequest {
            model: agent.config.model.clone(),
            messages: vec![zen_core::ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: Some(10),
        };
        agent.llm.chat(req).await.unwrap();
        assert_eq!(
            backend.models.lock().unwrap().as_slice(),
            ["deepseek-v4.1-flash"]
        );
    }

    #[test]
    fn switch_to_openrouter_model_changes_provider() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        let old_llm = agent.llm.clone();

        let msg = apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &None,
            "qwen/qwen3-coder:free",
        )
        .unwrap();
        assert!(msg.contains("openrouter.ai"), "{msg}");
        assert_eq!(agent.config.model, "qwen/qwen3-coder:free");
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("test-openrouter-key"));
        // Provider changed: the live backend must have been rebuilt.
        assert!(!std::sync::Arc::ptr_eq(&old_llm, &agent.llm));

        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn switch_rebuilds_backend_when_only_base_url_changes() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "shared-key");
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        // Same key on both sides: only the base URL moves.
        cfg.api_key = Some("shared-key".to_string());
        let old_llm = agent.llm.clone();

        let msg = apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &None,
            "qwen/qwen3-coder:free",
        )
        .unwrap();
        assert!(msg.contains("openrouter.ai"), "{msg}");
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("shared-key"));
        // The live backend was built for the old base URL, so it
        // must be rebuilt even though the key is identical.
        assert!(!std::sync::Arc::ptr_eq(&old_llm, &agent.llm));

        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn switch_rebuilds_backend_when_only_key_changes() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        // A stale key from an earlier switch (PR #71 review F1): the
        // base URL is already the home gateway, only the key moves.
        cfg.api_key = Some("stale-key".to_string());
        let old_llm = agent.llm.clone();

        let msg = apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &None,
            "deepseek-v4.1-flash",
        )
        .unwrap();
        assert!(msg.contains("deepseek-v4.1-flash"), "{msg}");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        assert_eq!(cfg.api_key, None);
        // The live backend was built with the stale key, so it must
        // be rebuilt even though the base URL is identical.
        assert!(!std::sync::Arc::ptr_eq(&old_llm, &agent.llm));
    }

    #[test]
    fn switch_to_openrouter_model_without_key_is_refused() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::remove_var("OPENROUTER_API_KEY");
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        let old_llm = agent.llm.clone();

        let err = apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &None,
            "qwen/qwen3-coder:free",
        )
        .unwrap_err();
        assert!(err.contains("OPENROUTER_API_KEY"), "{err}");
        // Refused switch: previous model, provider and backend untouched.
        assert_eq!(agent.config.model, "google/gemini-4-argon:free");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        assert!(std::sync::Arc::ptr_eq(&old_llm, &agent.llm));

        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[tokio::test]
    async fn slash_pin_to_openrouter_model_without_key_is_refused() {
        // Bug hunt #1 (Scout B P1-1 + Scout C F3): a slash command pinning
        // a cross-provider catalog id must route like /model does. With
        // the provider key unset the turn must be refused before any
        // request is sent - not run against the current gateway with
        // the foreign id (HTTP 400, PR #71 class).
        use clap::Parser;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::remove_var("OPENROUTER_API_KEY");
        let dir = std::env::temp_dir().join(format!("zen_slash_pin_{}", uuid::Uuid::new_v4()));
        let cmd_dir = dir.join(".zen").join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        std::fs::write(
            cmd_dir.join("probe.md"),
            "---\nmodel: qwen/qwen3-coder:free\n---\nSay hi.\n",
        )
        .unwrap();
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        let cli = crate::cli::Cli::parse_from(["zen"]);
        let session_path = dir.join("session.jsonl");
        let mut session_id = crate::session::new_session_id();
        let mut writer = crate::session::TranscriptWriter::new(session_path, &session_id, &dir);
        let mut repl = ReplState {
            session_id: &mut session_id,
            writer: &mut writer,
            mcp_status: vec![],
            interrupts: None,
            recorder: None,
            home_api_key: None,
            auto_model: false,
            goal: None,
            personality: repl_polish::Personality::default(),
            theme: repl_polish::Theme::default(),
            vim_mode: false,
            base_system_prompt: String::new(),
            pre_plan_mode: zen_core::PermissionMode::Default,
            pending_plan: None,
            scope: crate::doneguard::ScopeGuard::default(),
            scope_seeded: false,
            checkpoints: std::sync::Arc::new(zen_tools::checkpoint::Checkpoints::new(
                dir.join("checkpoints"),
            )),
            progress: std::sync::Arc::new(crate::repl_progress::ReplProgressSink::new()),
        };
        // The env lock only guards setup; drop it before the await
        // (clippy::await_holding_lock), as the sibling async test does.
        drop(_env);
        let action = handle_slash(
            "/probe hi",
            &cli,
            &dir,
            None,
            &mut cfg,
            &mut repl,
            &mut agent,
        )
        .await
        .unwrap();
        assert!(matches!(action, LineAction::Continue));
        // Refused before the turn: no request sent, nothing switched.
        assert!(backend.models.lock().unwrap().is_empty());
        assert_eq!(agent.config.model, "google/gemini-4-argon:free");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);

        // Re-acquire the lock for the env restore.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn switch_to_unknown_pool_id_keeps_string_only() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        let msg = apply_model_switch(&mut cfg, &mut agent, None, None, &None, "some-probed-model")
            .unwrap();
        assert_eq!(msg, "model set to some-probed-model");
        assert_eq!(agent.config.model, "some-probed-model");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
    }

    #[test]
    fn model_list_marks_current_by_gateway_id() {
        let out = format_model_list("deepseek-v4.1-flash", &rows(), false);
        assert!(out.contains("Models — current: deepseek-v4.1-flash"));
        let first = out.lines().nth(1).unwrap();
        assert!(first.starts_with("▸ 1."), "{first}");
    }

    #[test]
    fn humanise_context_formats() {
        assert_eq!(humanise_context(Some(1_000_000)), "1M");
        assert_eq!(humanise_context(Some(262_144)), "262K");
        assert_eq!(humanise_context(Some(131_072)), "131K");
        assert_eq!(humanise_context(None), "—");
    }

    #[test]
    fn switch_round_trip_restores_home_key() {
        // F1: experiential -> openrouter -> experiential must restore
        // the startup key, not leak the OpenRouter key.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend.clone());
        let mut cfg = test_cfg();
        cfg.api_key = Some("exp-key".to_string());
        let home_key = cfg.api_key.clone();

        apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &home_key,
            "qwen/qwen3-coder:free",
        )
        .unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("or-key"));
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");

        apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &home_key,
            "deepseek-v4.1-flash",
        )
        .unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("exp-key"));
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        assert_eq!(agent.config.model, "deepseek-v4.1-flash");
        assert_eq!(cfg.model, "deepseek-v4.1-flash");

        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_keeps_experiential_default_on_default_base() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = test_cfg();
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
        assert!(routed);
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
    }

    #[test]
    fn build_repl_config_uses_catalog_context_window() {
        // Bug hunt B2, direction (a): the default model is a 1M
        // catalog entry, so a fresh session must compact at 800k,
        // not at the 128k / 100k default pair.
        use clap::Parser;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cli = crate::cli::Cli::parse_from(["zen"]);
        let mut cfg = test_cfg();
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        assert_eq!(cfg.context_window, None);
        let config = build_repl_config(&cli, &cfg, &std::env::temp_dir());
        assert_eq!(config.context_window, 1_000_000);
        assert_eq!(config.auto_compact_threshold, Some(800_000));

        // An explicit setting wins over the catalog.
        cfg.context_window = Some(50_000);
        let config = build_repl_config(&cli, &cfg, &std::env::temp_dir());
        assert_eq!(config.context_window, 50_000);
        assert_eq!(config.auto_compact_threshold, Some(40_000));

        // An unknown model falls back to the 128k default.
        cfg.context_window = None;
        cfg.model = "some/unknown-model".to_string();
        let config = build_repl_config(&cli, &cfg, &std::env::temp_dir());
        assert_eq!(config.context_window, 128_000);
        assert_eq!(config.auto_compact_threshold, Some(102_400));
    }

    #[test]
    fn model_switch_persists_to_local_settings() {
        use clap::Parser;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zen_model_persist_{}", uuid::Uuid::new_v4()));
        let home =
            std::env::temp_dir().join(format!("zen_model_persist_home_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let old_model = std::env::var("ZEN_MODEL").ok();
        let old_base = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        // Pin: the saved id is what the next run starts on.
        let path = save_model_choice(
            &dir,
            false,
            "deepseek-v4.1-flash",
            crate::settings::DEFAULT_BASE_URL,
        )
        .unwrap();
        assert_eq!(path, dir.join(".zen").join("settings.local.json"));
        let cli = crate::cli::Cli::parse_from(["zen", "--trust-folder"]);
        let cfg = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        assert!(!cfg.model_auto);
        // Auto: the literal `auto` is saved, so auto mode survives.
        save_model_choice(
            &dir,
            true,
            "deepseek-v4.1-flash",
            crate::settings::DEFAULT_BASE_URL,
        )
        .unwrap();
        let cfg = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        assert!(cfg.model_auto);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&home).ok();
        match old_model {
            Some(v) => std::env::set_var("ZEN_MODEL", v),
            None => std::env::remove_var("ZEN_MODEL"),
        }
        match old_base {
            Some(v) => std::env::set_var("ZEN_BASE_URL", v),
            None => std::env::remove_var("ZEN_BASE_URL"),
        }
    }

    #[test]
    fn model_switch_persists_base_url_across_restart() {
        // Bug hunt #2 (Scout A B1 + Scout C F2): /model persists the
        // model id but not its base URL, so a cross-provider switch
        // breaks after restart - and /model auto silently loses the
        // provider. A restart must land on the same provider.
        use clap::Parser;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old_key = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let old_model = std::env::var("ZEN_MODEL").ok();
        let old_base = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        let dir = std::env::temp_dir().join(format!("zen_model_baseurl_{}", uuid::Uuid::new_v4()));
        let home =
            std::env::temp_dir().join(format!("zen_model_baseurl_home_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let backend = std::sync::Arc::new(RecordingBackend {
            models: std::sync::Mutex::new(vec![]),
        });
        let mut agent = test_agent(backend);
        let cli = crate::cli::Cli::parse_from(["zen", "--trust-folder"]);
        let mut cfg = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        assert_eq!(cfg.base_url, crate::settings::DEFAULT_BASE_URL);
        let home_key = cfg.api_key.clone();
        apply_model_switch(
            &mut cfg,
            &mut agent,
            None,
            None,
            &home_key,
            "qwen/qwen3-coder:free",
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        report_model_switch(&dir, false, &cfg.model, &cfg.base_url, "switched");
        let cfg2 = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        assert_eq!(cfg2.model, "qwen/qwen3-coder:free");
        assert_eq!(cfg2.base_url, "https://openrouter.ai/api/v1");
        // /model auto after the pin must keep the provider too.
        report_model_switch(&dir, true, &cfg.model, &cfg.base_url, "switched (auto)");
        let cfg3 = crate::settings::resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        assert!(cfg3.model_auto);
        assert_eq!(cfg3.base_url, "https://openrouter.ai/api/v1");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&home).ok();
        match old_key {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
        match old_model {
            Some(v) => std::env::set_var("ZEN_MODEL", v),
            None => std::env::remove_var("ZEN_MODEL"),
        }
        match old_base {
            Some(v) => std::env::set_var("ZEN_BASE_URL", v),
            None => std::env::remove_var("ZEN_BASE_URL"),
        }
    }

    #[test]
    fn report_model_switch_persists_choice() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zen_report_switch_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old_model = std::env::var("ZEN_MODEL").ok();
        let old_base = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        report_model_switch(
            &dir,
            false,
            "deepseek-v4.1-flash",
            crate::settings::DEFAULT_BASE_URL,
            "switched",
        );
        let path = dir.join(".zen").join("settings.local.json");
        assert!(path.exists(), "report_model_switch must persist choice");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("deepseek-v4.1-flash"),
            "saved model id missing"
        );
        std::fs::remove_dir_all(&dir).ok();
        match old_model {
            Some(v) => std::env::set_var("ZEN_MODEL", v),
            None => std::env::remove_var("ZEN_MODEL"),
        }
        match old_base {
            Some(v) => std::env::set_var("ZEN_BASE_URL", v),
            None => std::env::remove_var("ZEN_BASE_URL"),
        }
    }

    #[test]
    fn route_moves_openrouter_model_to_its_provider() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let mut cfg = test_cfg();
        cfg.model = "qwen/qwen3-coder:free".to_string();
        let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
        assert!(routed);
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("or-key"));
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_refuses_openrouter_model_without_key() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::remove_var("OPENROUTER_API_KEY");
        let mut cfg = test_cfg();
        cfg.model = "qwen/qwen3-coder:free".to_string();
        let err = crate::settings::route_catalog_model(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("OPENROUTER_API_KEY"), "{err}");
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_auto_with_ollama_bails_with_real_reason() {
        // `--provider ollama --model auto` must not end at the Ollama
        // readiness check, whose hint would be `ollama pull auto`
        // (PR #72 review follow-up).
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = test_cfg();
        cfg.ollama = true;
        cfg.model = "auto".to_string();
        let err = crate::settings::route_catalog_model(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("Ollama"), "{err}");
        assert!(!err.to_string().contains("pull"), "{err}");
        // Casing must not change the meaning (PR #73 review
        // follow-up): `Auto` is refused with the same real reason.
        let mut cfg = test_cfg();
        cfg.ollama = true;
        cfg.model = "Auto".to_string();
        let err = crate::settings::route_catalog_model(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("Ollama"), "{err}");
        assert!(!err.to_string().contains("pull"), "{err}");
    }

    #[test]
    fn route_auto_resolves_to_provider_default() {
        // Auto mode (Leader tin 7962): `auto` means the current
        // provider's best free model.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = test_cfg();
        cfg.model = "auto".to_string();
        let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
        assert!(routed);
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        // On the OpenRouter base, auto is that provider's default.
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let mut cfg = test_cfg();
        cfg.model = "auto".to_string();
        cfg.base_url = crate::models::OPENROUTER_BASE_URL.to_string();
        let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
        assert!(routed);
        assert_eq!(cfg.model, "qwen/qwen3-coder:free");
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
        // Casing must not change the meaning (PR #73 review
        // follow-up): `Auto` and `AUTO` resolve the same way.
        for spelling in ["Auto", "AUTO"] {
            let mut cfg = test_cfg();
            cfg.model = spelling.to_string();
            let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
            assert!(routed, "{spelling}");
            assert_eq!(cfg.model, "deepseek-v4.1-flash", "{spelling}");
        }
        // A custom base has no known provider: auto refuses clearly
        // instead of sending the literal string "auto" to the gateway.
        let mut cfg = test_cfg();
        cfg.model = "auto".to_string();
        cfg.base_url = "https://example.invalid/v1".to_string();
        let err = crate::settings::route_catalog_model(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("known provider"), "{err}");
    }

    #[test]
    fn route_leaves_custom_base_url_untouched() {
        // F3: the user pointed the client at another endpoint by
        // hand while the model is still a catalog entry. Routing
        // must not rewrite the model or the key for an endpoint it
        // does not own - that broke working custom-base setups.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = test_cfg();
        cfg.base_url = "https://openrouter.ai/api/v1".to_string();
        cfg.api_key = Some("custom-key".to_string());
        let routed = crate::settings::route_catalog_model(&mut cfg).unwrap();
        assert!(!routed);
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("custom-key"));
    }
}

#[cfg(test)]
mod welcome_box_tests {
    use super::*;
    use repl_polish::Theme;

    fn plain_box(
        version: &str,
        model: &str,
        cwd: &str,
        branch: Option<&str>,
        sessions: &[(String, std::path::PathBuf)],
        ephemeral: bool,
    ) -> String {
        welcome_box(
            Theme::None,
            version,
            model,
            cwd,
            branch,
            sessions,
            ephemeral,
        )
    }

    #[test]
    fn welcome_box_has_border_and_contents() {
        let out = plain_box(
            "0.1.0",
            "google/gemini-4-argon:free",
            "/home/zen",
            None,
            &[],
            false,
        );
        assert!(out.starts_with('╭'));
        assert!(out.ends_with('╯'));
        assert!(out.contains("✓ zen 0.1.0"));
        assert!(out.contains("model  gemini-4-argon:free  ·  google"));
        assert!(out.contains("cwd    /home/zen"));
        assert!(out.contains("/model to switch · /help for commands"));
        assert!(!out.contains("EPHEMERAL"));
        // First run: no greeting without a previous session.
        assert!(!out.contains("Welcome back!"));
        // v2: the block-Z mark is gone.
        assert!(!out.contains('█'));
    }

    #[test]
    fn welcome_box_marks_ephemeral_mode() {
        let out = plain_box("0.1.0", "m", "/tmp", None, &[], true);
        assert!(out.contains("EPHEMERAL MODE"));
    }

    #[test]
    fn welcome_box_lines_share_one_width() {
        let out = plain_box(
            "0.1.0",
            "a-very-long-model-id-that-stretches-the-box",
            "/x",
            None,
            &[],
            false,
        );
        let widths: Vec<usize> = out.lines().map(|l| l.chars().count()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn welcome_box_caps_width_at_sixty() {
        let out = plain_box(
            "0.1.0",
            "google/gemini-4-argon:free",
            "/home/zen/a/very/long/path/that/should/not/set/the/box/width/at/all",
            None,
            &[],
            false,
        );
        // Inner width 60 + 2 border chars + 2 padding = 64 max per line.
        assert!(out.lines().all(|l| l.chars().count() <= 64));
        assert!(out.contains('…'), "cwd should be middle-truncated");
    }

    #[test]
    fn welcome_box_greets_returning_users() {
        let sessions = vec![(
            "sess-1".to_string(),
            std::path::PathBuf::from("/tmp/x.jsonl"),
        )];
        let out = plain_box("0.1.0", "m", "/tmp", Some("dev/x"), &sessions, false);
        assert!(out.contains("Welcome back!"));
        assert!(out.contains("branch dev/x"));
    }

    #[test]
    fn split_model_id_handles_plain_ids() {
        assert_eq!(
            split_model_id("google/gemini-4-argon:free"),
            ("google", "gemini-4-argon:free")
        );
        assert_eq!(split_model_id("local-model"), ("", "local-model"));
    }

    #[test]
    fn truncate_middle_keeps_head_and_tail() {
        let out = truncate_middle("~/a/b/c/d/e/f/g", 12);
        assert!(out.contains('…'));
        assert!(out.chars().count() <= 12);
        assert!(out.starts_with("~/a"));
    }
}
