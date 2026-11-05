use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zen_core::{ChatMessage, ChatRequest, ChatResponse, LlmBackend, Usage};

pub struct CatalogModel {
    pub id: &'static str,
    pub provider: &'static str,
    pub context_window: u32,
}

pub const FREE_MODEL_CATALOG: &[CatalogModel] = &[
    CatalogModel {
        id: "google/gemini-4-argon:free",
        provider: "experientiallabs",
        context_window: 1_000_000,
    },
    CatalogModel {
        id: "qwen/qwen3-coder:free",
        provider: "openrouter",
        context_window: 262_144,
    },
    CatalogModel {
        id: "deepseek/deepseek-r1:free",
        provider: "openrouter",
        context_window: 131_072,
    },
    CatalogModel {
        id: "nvidia/nemotron-3-ultra-550b-a55b:free",
        provider: "openrouter",
        context_window: 131_072,
    },
    CatalogModel {
        id: "cohere/north-mini-code:free",
        provider: "openrouter",
        context_window: 131_072,
    },
];

struct ProbeTask {
    prompt: &'static str,
    check: fn(&str) -> f64,
}

fn check_exact_ok(out: &str) -> f64 {
    let t = out.trim();
    if t == "OK" {
        1.0
    } else if t.contains("OK") {
        0.5
    } else {
        0.15
    }
}

fn check_arithmetic(out: &str) -> f64 {
    let t = out.trim();
    if t == "4" {
        1.0
    } else if t.contains('4') {
        0.5
    } else {
        0.15
    }
}

fn check_code_completion(out: &str) -> f64 {
    if out.contains("a + b") || out.contains("a+b") {
        1.0
    } else if out.contains("return") {
        0.5
    } else {
        0.15
    }
}

const PROBE_TASKS: &[ProbeTask] = &[
    ProbeTask {
        prompt: "Reply with exactly: OK",
        check: check_exact_ok,
    },
    ProbeTask {
        prompt: "What is 2+2? Reply with just the number.",
        check: check_arithmetic,
    },
    ProbeTask {
        prompt:
            "Complete this Rust function, reply with code only:\nfn add(a: i32, b: i32) -> i32 {",
        check: check_code_completion,
    },
];

pub const PROBE_MAX_TOKENS: u32 = 32;

pub const PROBE_TIMEOUT_SECS: u64 = 30;

pub const PROBE_GAP: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProbe {
    pub model: String,
    pub provider: String,
    pub context_window: Option<u32>,

    pub free: bool,

    pub latency_ms: u64,

    pub success: bool,

    pub score: f64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCache {
    pub verified_at: String,

    pub base_url: String,
    pub results: Vec<ModelProbe>,
}

pub const CACHE_STALE_SECS: i64 = 24 * 3600;

pub fn cache_path(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".zen").join("models-cache.json"))
}

pub fn load_cache(home: Option<&Path>) -> Option<ModelsCache> {
    let path = cache_path(home)?;
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_cache(home: Option<&Path>, cache: &ModelsCache) -> Result<()> {
    let path = cache_path(home).context("home directory not available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cache)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn cache_age_secs(cache: &ModelsCache) -> Option<i64> {
    let verified = chrono::DateTime::parse_from_rfc3339(&cache.verified_at).ok()?;
    Some(
        chrono::Utc::now()
            .signed_duration_since(verified)
            .num_seconds(),
    )
}

pub fn cache_is_stale(cache: &ModelsCache) -> bool {
    cache_age_secs(cache)
        .map(|a| a > CACHE_STALE_SECS)
        .unwrap_or(true)
}

// --- P0-2: free-model radar discipline -------------------------------------
//
// A fresh probe never replaces the cached pool silently:
//  1. the pool diff (added / removed / score-changed / best) is shown and
//     written to the radar log;
//  2. switching the pool needs the user's approval (default No);
//  3. the old pool is backed up before it is overwritten, and can be
//     restored with `zen models --restore`.

/// What changed between the cached pool and a freshly probed pool.
#[derive(Debug, Default, PartialEq)]
pub struct PoolDiff {
    /// Models present in the fresh probe but not in the cached pool.
    pub added: Vec<String>,
    /// Models present in the cached pool but missing from the fresh probe.
    pub removed: Vec<String>,
    /// Models present in both whose score moved: (model, old score, new score).
    pub score_changed: Vec<(String, f64, f64)>,
}

impl PoolDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.score_changed.is_empty()
    }
}

/// Score moves smaller than this are display noise, not a real change.
const SCORE_CHANGE_EPSILON: f64 = 0.005;

pub fn diff_pools(old: &[ModelProbe], new: &[ModelProbe]) -> PoolDiff {
    let mut diff = PoolDiff::default();
    for n in new {
        match old.iter().find(|o| o.model == n.model) {
            None => diff.added.push(n.model.clone()),
            Some(o) => {
                if (o.score - n.score).abs() > SCORE_CHANGE_EPSILON {
                    diff.score_changed.push((n.model.clone(), o.score, n.score));
                }
            }
        }
    }
    for o in old {
        if !new.iter().any(|n| n.model == o.model) {
            diff.removed.push(o.model.clone());
        }
    }
    diff
}

/// One human-readable summary of a pool diff, for stderr and the radar log.
pub fn format_pool_diff(diff: &PoolDiff, old_best: Option<&str>, new_best: Option<&str>) -> String {
    if diff.is_empty() && old_best == new_best {
        return "free-model pool diff: no changes".to_string();
    }
    let mut lines = vec!["free-model pool diff (cached -> probed):".to_string()];
    for m in &diff.added {
        lines.push(format!("  + added: {}", m));
    }
    for m in &diff.removed {
        lines.push(format!("  - removed: {}", m));
    }
    for (m, o, n) in &diff.score_changed {
        lines.push(format!("  ~ score: {} {:.2} -> {:.2}", m, o, n));
    }
    if old_best != new_best {
        lines.push(format!(
            "  best: {} -> {}",
            old_best.unwrap_or("(none)"),
            new_best.unwrap_or("(none)")
        ));
    }
    lines.join("\n")
}

/// Path of the pool backup: `models-cache.json.bak` next to the cache.
pub fn backup_path(home: Option<&Path>) -> Option<PathBuf> {
    cache_path(home).map(|p| p.with_extension("json.bak"))
}

/// Copy the current cache file to the backup path.
/// Returns Ok(true) when a cache file existed and was backed up,
/// Ok(false) when there was nothing to back up.
pub fn backup_cache(home: Option<&Path>) -> Result<bool> {
    let Some(path) = cache_path(home) else {
        return Ok(false);
    };
    if !path.is_file() {
        return Ok(false);
    }
    let bak = backup_path(home).context("home directory not available")?;
    std::fs::copy(&path, &bak)?;
    Ok(true)
}

/// Restore the pool from its backup file, returning the restored cache.
pub fn restore_cache(home: Option<&Path>) -> Result<ModelsCache> {
    let bak = backup_path(home).context("home directory not available")?;
    if !bak.is_file() {
        anyhow::bail!("no model pool backup found (expected {})", bak.display());
    }
    let path = cache_path(home).context("home directory not available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&bak, &path)?;
    load_cache(home).context("backup file is not a valid models cache")
}

/// Path of the append-only radar log: `~/.zen/models-radar.log`.
pub fn radar_log_path(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".zen").join("models-radar.log"))
}

/// Append one timestamped radar event line. Best-effort: callers warn on error.
pub fn append_radar_log(home: Option<&Path>, event: &str) -> Result<()> {
    let path = radar_log_path(home).context("home directory not available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "[{}] {}", chrono::Utc::now().to_rfc3339(), event)?;
    Ok(())
}

/// Ask the user to approve switching to a freshly probed pool.
/// Default is No: a non-terminal stdin, an unreadable answer, or anything
/// but y/yes refuses the switch.
pub fn confirm_pool_switch(reason: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("Free-model pool change: {}. Switch pool? [y/N] ", reason);
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    crate::session_mgmt::parse_confirmation_answer(&answer)
}

/// Outcome of the radar-discipline gate for a freshly probed pool.
pub enum SwitchOutcome {
    /// No old pool, or the switch was approved (old pool backed up).
    /// The caller may save the fresh cache.
    UseFresh,
    /// The switch was refused: keep the cached pool. Carries its results.
    UseOld(Vec<ModelProbe>),
}

fn back_up_old_pool(home: Option<&Path>) {
    match backup_cache(home) {
        Ok(true) => {
            if let Err(e) = append_radar_log(home, "pool backup saved") {
                eprintln!("warning: could not write radar log: {:#}", e);
            }
        }
        Ok(false) => {}
        Err(e) => eprintln!("warning: could not back up old model pool: {:#}", e),
    }
}

fn log_radar(home: Option<&Path>, event: &str) {
    if let Err(e) = append_radar_log(home, event) {
        eprintln!("warning: could not write radar log: {:#}", e);
    }
}

/// Apply the P0-2 radar discipline to a freshly probed pool.
///
/// Shows the diff, writes it to the radar log, asks for approval when the
/// best model or the free catalog changed, and backs up the old pool before
/// it may be overwritten. `auto_approve` (`--yes`) records an approval
/// without prompting; it never approves a switch to a paid model because
/// `best_model` only ever picks free models. The `confirm` callback is the
/// prompt used when approval is needed (pass `confirm_pool_switch` in
/// production; tests pass a scripted answer).
pub fn maybe_switch_pool(
    home: Option<&Path>,
    cached: Option<&ModelsCache>,
    fresh: &ModelsCache,
    auto_approve: bool,
    confirm: &dyn Fn(&str) -> bool,
) -> SwitchOutcome {
    let Some(old) = cached else {
        return SwitchOutcome::UseFresh;
    };
    if old.base_url != fresh.base_url {
        // A different provider's pool: nothing to compare against, but
        // preserve it before the caller overwrites the file.
        back_up_old_pool(home);
        return SwitchOutcome::UseFresh;
    }

    let old_best = best_model(&old.results).map(|m| m.model.clone());
    let new_best = best_model(&fresh.results).map(|m| m.model.clone());
    let diff = diff_pools(&old.results, &fresh.results);

    if !diff.is_empty() || old_best != new_best {
        let summary = format_pool_diff(&diff, old_best.as_deref(), new_best.as_deref());
        eprintln!("{}", summary);
        log_radar(
            home,
            &format!("pool diff: {}", summary.replace('\n', " | ")),
        );
    }

    let catalog_changed = !diff.added.is_empty() || !diff.removed.is_empty();
    if old_best == new_best && !catalog_changed {
        // Scores moved only: no pool switch to approve, but still preserve
        // the old pool before the caller overwrites it.
        back_up_old_pool(home);
        return SwitchOutcome::UseFresh;
    }

    let reason = if old_best != new_best {
        format!(
            "best changes '{}' -> '{}'",
            old_best.as_deref().unwrap_or("(none)"),
            new_best.as_deref().unwrap_or("(none)")
        )
    } else {
        format!(
            "catalog changes ({} added, {} removed)",
            diff.added.len(),
            diff.removed.len()
        )
    };
    let approved = auto_approve || confirm(&reason);
    let decision = if approved {
        if auto_approve {
            "approved (--yes)"
        } else {
            "approved (prompt)"
        }
    } else {
        "refused (default No; re-run with --yes to approve)"
    };
    log_radar(home, &format!("pool switch {}: {}", decision, reason));
    if !approved {
        eprintln!(
            "pool switch refused; keeping cached pool (best: {})",
            old_best.as_deref().unwrap_or("(none)")
        );
        return SwitchOutcome::UseOld(old.results.clone());
    }
    back_up_old_pool(home);
    SwitchOutcome::UseFresh
}

pub fn aggregate_score(qualities: &[f64], avg_latency_ms: u64) -> f64 {
    if qualities.is_empty() {
        return 0.0;
    }
    let mean_q = qualities.iter().sum::<f64>() / qualities.len() as f64;
    let secs = avg_latency_ms as f64 / 1000.0;
    (mean_q / (1.0 + secs / 30.0)).clamp(0.0, 1.0)
}

pub fn best_model(results: &[ModelProbe]) -> Option<&ModelProbe> {
    // P0-2: the pool only ever recommends a free model. A non-free result
    // (e.g. a catalog entry that lost its free status) is never picked,
    // even when it scores highest.
    results
        .iter()
        .filter(|r| r.success && r.free)
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal))
}

pub async fn probe_model(
    backend: &dyn LlmBackend,
    model_id: &str,
    provider: &str,
    context_window: Option<u32>,
    free: bool,
) -> ModelProbe {
    let mut qualities = Vec::with_capacity(PROBE_TASKS.len());
    let mut latencies = Vec::with_capacity(PROBE_TASKS.len());

    for task in PROBE_TASKS {
        let req = ChatRequest {
            model: model_id.to_string(),
            messages: vec![ChatMessage::user(task.prompt)],
            tools: vec![],
            max_tokens: Some(PROBE_MAX_TOKENS),
        };
        let start = Instant::now();
        match backend.chat(req).await {
            Ok(resp) => {
                let text = resp.message.content.unwrap_or_default();
                latencies.push(start.elapsed().as_millis() as u64);
                qualities.push((task.check)(&text));
            }
            Err(e) => {
                return ModelProbe {
                    model: model_id.to_string(),
                    provider: provider.to_string(),
                    context_window,
                    free,
                    latency_ms: 0,
                    success: false,
                    score: 0.0,
                    last_error: Some(short_error(&e.to_string())),
                };
            }
        }
    }

    let avg_latency = latencies.iter().sum::<u64>() / latencies.len().max(1) as u64;
    ModelProbe {
        model: model_id.to_string(),
        provider: provider.to_string(),
        context_window,
        free,
        latency_ms: avg_latency,
        success: true,
        score: aggregate_score(&qualities, avg_latency),
        last_error: None,
    }
}

fn short_error(msg: &str) -> String {
    let first_line = msg.lines().next().unwrap_or(msg);
    let mut s: String = first_line.chars().take(120).collect();
    if first_line.chars().count() > 120 {
        s.push('…');
    }
    s
}

pub struct ProbeClient {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl ProbeClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[derive(Deserialize)]
struct ProbeChatResponse {
    #[serde(default)]
    choices: Vec<ProbeChoice>,
    #[serde(default)]
    usage: Option<ProbeUsage>,
}

#[derive(Deserialize)]
struct ProbeChoice {
    message: ProbeMessage,
}

#[derive(Deserialize)]
struct ProbeMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ProbeUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[async_trait::async_trait]
impl LlmBackend for ProbeClient {
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        let mut r = self.http.post(self.endpoint()).json(&serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "max_tokens": req.max_tokens,
        }));
        if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            r = r.bearer_auth(key);
        }
        let resp = r.send().await?.error_for_status()?;
        let body: ProbeChatResponse = resp.json().await?;
        let choice = body
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("probe: empty choices in response"))?;
        let usage = body.usage.unwrap_or(ProbeUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });
        Ok(ChatResponse {
            message: ChatMessage::assistant(choice.message.content, None),
            usage: Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
                ..Default::default()
            },
            finish_reason: None,
        })
    }
}

pub struct DiscoveredModel {
    pub id: String,
    pub free: bool,
}

fn id_looks_free(id: &str) -> bool {
    id.ends_with(":free")
}

pub async fn fetch_models_list(base_url: &str, api_key: Option<&str>) -> Vec<DiscoveredModel> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build();
    let http = match http {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut req = http.get(&url);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    let body: serde_json::Value = match req.send().await {
        Ok(r) => match r.error_for_status() {
            Ok(ok) => match ok.json().await {
                Ok(v) => v,
                Err(_) => return vec![],
            },
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    data.iter()
        .filter_map(|m| m.get("id")?.as_str().map(|s| s.to_string()))
        .map(|id| {
            let priced_zero = body_pointer_free(&body, &id);
            DiscoveredModel {
                free: id_looks_free(&id) || priced_zero,
                id,
            }
        })
        .collect()
}

fn body_pointer_free(body: &serde_json::Value, id: &str) -> bool {
    let entry = body.get("data").and_then(|d| d.as_array()).and_then(|arr| {
        arr.iter()
            .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(id))
    });
    let pricing = match entry.and_then(|e| e.get("pricing")) {
        Some(p) => p,
        None => return false,
    };
    let zero = |v: &serde_json::Value| v.as_str() == Some("0");
    pricing.get("prompt").map(zero).unwrap_or(false)
        && pricing.get("completion").map(zero).unwrap_or(false)
}

pub struct Candidate {
    pub id: String,
    pub provider: String,
    pub context_window: Option<u32>,
    pub free: bool,
}

pub fn merge_candidates(
    catalog: &[CatalogModel],
    discovered: &[DiscoveredModel],
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = catalog
        .iter()
        .map(|c| Candidate {
            id: c.id.to_string(),
            provider: c.provider.to_string(),
            context_window: Some(c.context_window),
            free: true,
        })
        .collect();
    for d in discovered {
        if out.iter().any(|c| c.id == d.id) {
            continue;
        }

        if d.free {
            out.push(Candidate {
                id: d.id.clone(),
                provider: "discovered".to_string(),
                context_window: None,
                free: true,
            });
        }
    }
    out
}

pub async fn models_command(
    cli: &crate::cli::Cli,
    cwd: &Path,
    home: Option<&Path>,
    args: &crate::cli::ModelsArgs,
) -> Result<()> {
    // P0-2: explicit rollback to the backed-up pool. This is the recovery
    // path when a probe went wrong; the flag itself is the confirmation.
    if args.restore {
        let restored = restore_cache(home)?;
        let best = best_model(&restored.results).map(|m| m.model.clone());
        if !cli.quiet {
            println!(
                "restored model pool from backup ({} model(s), best: {})",
                restored.results.len(),
                best.as_deref().unwrap_or("(none)")
            );
        }
        log_radar(home, "pool restored from backup");
        return Ok(());
    }

    let cfg = crate::settings::resolve_config_with_home(cli, cwd, home)?;

    let cached = load_cache(home);
    let cache_usable = cached.as_ref().is_some_and(|c| c.base_url == cfg.base_url);

    let results: Vec<ModelProbe> = if args.refresh || !cache_usable {
        if !args.best {
            eprintln!("probing free models at {} ...", cfg.base_url);
        }
        let discovered = fetch_models_list(&cfg.base_url, cfg.api_key.as_deref()).await;
        let candidates = merge_candidates(FREE_MODEL_CATALOG, &discovered);
        if candidates.is_empty() {
            anyhow::bail!("no candidate free models found (static catalog is empty and /models discovery failed)");
        }
        let backend = ProbeClient::new(&cfg.base_url, cfg.api_key.clone());
        let mut results = Vec::with_capacity(candidates.len());
        for (i, cand) in candidates.iter().enumerate() {
            if !args.best {
                eprintln!("  [{}/{}] probing {} ...", i + 1, candidates.len(), cand.id);
            }
            let probe = probe_model(
                &backend,
                &cand.id,
                &cand.provider,
                cand.context_window,
                cand.free,
            )
            .await;
            if !args.best {
                eprintln!(
                    "           {}",
                    if probe.success {
                        format!(
                            "ok  score={:.2} latency={}ms",
                            probe.score, probe.latency_ms
                        )
                    } else {
                        format!(
                            "FAIL {}",
                            probe.last_error.as_deref().unwrap_or("unknown error")
                        )
                    }
                );
            }
            results.push(probe);
            if i + 1 < candidates.len() {
                tokio::time::sleep(PROBE_GAP).await;
            }
        }
        let fresh = ModelsCache {
            verified_at: chrono::Utc::now().to_rfc3339(),
            base_url: cfg.base_url.clone(),
            results: results.clone(),
        };
        // P0-2 radar discipline: a fresh probe never replaces the cached
        // pool silently (diff shown + logged, approval on best/catalog
        // change, old pool backed up). A refused switch keeps using the
        // cached pool for the rest of this command.
        match maybe_switch_pool(
            home,
            cached.as_ref(),
            &fresh,
            args.yes,
            &confirm_pool_switch,
        ) {
            SwitchOutcome::UseFresh => {
                if let Err(e) = save_cache(home, &fresh) {
                    eprintln!("warning: could not save models cache: {:#}", e);
                }
                results
            }
            SwitchOutcome::UseOld(old_results) => old_results,
        }
    } else {
        let cache = cached.unwrap();
        if cache_is_stale(&cache) {
            let age = cache_age_secs(&cache).unwrap_or(-1);
            eprintln!(
                "warning: models cache is {} old (>24h); run `zen models --refresh` to re-probe",
                human_age(age)
            );
        }
        cache.results
    };

    if args.best {
        match best_model(&results) {
            Some(m) => {
                if !cli.quiet {
                    println!("{}", m.model);
                }
                Ok(())
            }
            None => {
                eprintln!(
                    "zen models: no working model found (all {} candidates failed probing)",
                    results.len()
                );
                std::process::exit(1);
            }
        }
    } else if (cli.json_output || cli.raw) && !cli.quiet {
        if cli.raw {
            println!("{}", serde_json::to_string(&results)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        Ok(())
    } else {
        if !cli.quiet {
            print_table(&results, &cfg.base_url);
        }
        Ok(())
    }
}

fn human_age(secs: i64) -> String {
    if secs < 0 {
        return "unknown age".to_string();
    }
    if secs < 90 {
        format!("{}s", secs)
    } else if secs < 5400 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn fmt_ctx(ctx: Option<u32>) -> String {
    match ctx {
        Some(c) if c >= 1_000_000 => format!("{}M", c / 1_000_000),
        Some(c) if c >= 1000 => format!("{}k", c / 1000),
        Some(c) => c.to_string(),
        None => "-".to_string(),
    }
}

fn fmt_latency(ms: u64, success: bool) -> String {
    if !success {
        return "-".to_string();
    }
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}ms", ms)
    }
}

fn print_table(results: &[ModelProbe], base_url: &str) {
    println!(
        "zen models - {} candidate(s) from {}",
        results.len(),
        base_url
    );
    println!(
        "  {:<40} {:<16} {:>6} {:>7} {:>8} {:>6}  STATUS",
        "MODEL", "PROVIDER", "CTX", "PRICE", "LATENCY", "SCORE"
    );
    let best = best_model(results).map(|m| m.model.clone());
    for r in results {
        let status = if Some(&r.model) == best.as_ref() {
            "✓ recommended"
        } else if r.success {
            "✓"
        } else {
            "✗ failed"
        };
        let detail = if !r.success {
            format!(" ({})", r.last_error.as_deref().unwrap_or("error"))
        } else {
            String::new()
        };
        println!(
            "  {:<40} {:<16} {:>6} {:>7} {:>8} {:>6}  {}{}",
            truncate(&r.model, 40),
            truncate(&r.provider, 16),
            fmt_ctx(r.context_window),
            if r.free { "$0.00" } else { "?" },
            fmt_latency(r.latency_ms, r.success),
            if r.success {
                format!("{:.2}", r.score)
            } else {
                "-".to_string()
            },
            status,
            detail,
        );
    }
    if best.is_none() {
        println!("  no working model found; run `zen models --refresh` to retry");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct MockBackend {
        queue: Mutex<VecDeque<anyhow::Result<String>>>,
    }

    impl MockBackend {
        fn replies(texts: Vec<&str>) -> Self {
            Self {
                queue: Mutex::new(texts.into_iter().map(|t| Ok(t.to_string())).collect()),
            }
        }
        fn errors(errs: Vec<&str>) -> Self {
            Self {
                queue: Mutex::new(
                    errs.into_iter()
                        .map(|e| Err(anyhow::anyhow!("{}", e)))
                        .collect(),
                ),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmBackend for MockBackend {
        async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
            assert_eq!(
                req.max_tokens,
                Some(PROBE_MAX_TOKENS),
                "probes must stay cheap"
            );
            let text = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock: more probe calls than scripted replies")?;
            Ok(ChatResponse {
                message: ChatMessage::assistant(Some(text), None),
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    ..Default::default()
                },
                finish_reason: Some("stop".into()),
            })
        }
    }

    fn probe(id: &str, success: bool, score: f64) -> ModelProbe {
        ModelProbe {
            model: id.to_string(),
            provider: "test".to_string(),
            context_window: None,
            free: true,
            latency_ms: if success { 1200 } else { 0 },
            success,
            score,
            last_error: if success {
                None
            } else {
                Some("boom".to_string())
            },
        }
    }

    #[tokio::test]
    async fn probe_all_tasks_pass_scores_high() {
        let b = MockBackend::replies(vec![
            "OK",
            "4",
            "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}",
        ]);
        let r = probe_model(&b, "m", "p", None, true).await;
        assert!(r.success);
        assert!(
            r.score > 0.9,
            "perfect replies should score near 1.0, got {}",
            r.score
        );
        assert!(r.last_error.is_none());
    }

    #[tokio::test]
    async fn probe_chatty_but_correct_gets_partial_credit() {
        let b = MockBackend::replies(vec!["Sure! OK", "The answer is 4.", "return a+b; // done"]);
        let r = probe_model(&b, "m", "p", None, true).await;
        assert!(r.success);
        assert!(
            r.score > 0.4 && r.score < 0.9,
            "partial credit expected, got {}",
            r.score
        );
    }

    #[tokio::test]
    async fn probe_error_fails_the_model() {
        let b = MockBackend::errors(vec!["timeout after 30s"]);
        let r = probe_model(&b, "m", "p", None, true).await;
        assert!(!r.success);
        assert_eq!(r.score, 0.0);
        assert_eq!(r.latency_ms, 0);
        assert!(r.last_error.unwrap().contains("timeout"));
    }

    #[tokio::test]
    async fn probe_second_task_error_fails_the_model() {
        let b = MockBackend {
            queue: Mutex::new(VecDeque::from([
                Ok("OK".to_string()),
                Err(anyhow::anyhow!("429 rate limited")),
            ])),
        };
        let r = probe_model(&b, "m", "p", None, true).await;
        assert!(!r.success);
        assert_eq!(r.score, 0.0);
    }

    #[test]
    fn aggregate_score_penalises_latency() {
        let fast = aggregate_score(&[1.0, 1.0, 1.0], 1_000);
        let slow = aggregate_score(&[1.0, 1.0, 1.0], 30_000);
        assert!(fast > slow);
        assert!(
            (slow - 0.5).abs() < 0.01,
            "30s latency should halve the score, got {}",
            slow
        );
    }

    #[test]
    fn aggregate_score_empty_is_zero() {
        assert_eq!(aggregate_score(&[], 100), 0.0);
    }

    #[test]
    fn best_model_picks_highest_score_and_skips_failures() {
        let results = vec![
            probe("low", true, 0.5),
            probe("dead", false, 0.0),
            probe("high", true, 0.9),
        ];
        assert_eq!(best_model(&results).unwrap().model, "high");
    }

    #[test]
    fn best_model_none_when_all_failed() {
        let results = vec![probe("a", false, 0.0), probe("b", false, 0.0)];
        assert!(best_model(&results).is_none());
    }

    #[test]
    fn merge_candidates_dedupes_and_keeps_catalog_metadata() {
        let discovered = vec![
            DiscoveredModel {
                id: "qwen/qwen3-coder:free".to_string(),
                free: true,
            },
            DiscoveredModel {
                id: "new/model:free".to_string(),
                free: true,
            },
            DiscoveredModel {
                id: "paid/model".to_string(),
                free: false,
            },
        ];
        let merged = merge_candidates(FREE_MODEL_CATALOG, &discovered);
        assert_eq!(merged.len(), FREE_MODEL_CATALOG.len() + 1);
        let qwen = merged
            .iter()
            .find(|c| c.id == "qwen/qwen3-coder:free")
            .unwrap();
        assert_eq!(qwen.provider, "openrouter");
        assert_eq!(qwen.context_window, Some(262_144));
        assert!(merged.iter().any(|c| c.id == "new/model:free"));
        assert!(!merged.iter().any(|c| c.id == "paid/model"));
    }

    #[test]
    fn cache_roundtrip_and_staleness() {
        let dir = std::env::temp_dir().join(format!("zen-models-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let fresh = ModelsCache {
            verified_at: chrono::Utc::now().to_rfc3339(),
            base_url: "https://x".to_string(),
            results: vec![probe("m", true, 0.8)],
        };
        save_cache(Some(&dir), &fresh).unwrap();
        let loaded = load_cache(Some(&dir)).unwrap();
        assert_eq!(loaded.results.len(), 1);
        assert_eq!(loaded.results[0].model, "m");
        assert!(!cache_is_stale(&loaded));

        let old = ModelsCache {
            verified_at: "2020-01-01T00:00:00Z".to_string(),
            ..fresh
        };
        assert!(cache_is_stale(&old));
        assert!(cache_age_secs(&old).unwrap() > CACHE_STALE_SECS);

        std::fs::write(dir.join(".zen").join("models-cache.json"), "not json").unwrap();
        assert!(load_cache(Some(&dir)).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn short_error_truncates_long_messages() {
        let long = "e".repeat(500);
        assert!(short_error(&long).chars().count() <= 121);
        assert_eq!(short_error("one\ntwo"), "one");
    }

    #[test]
    fn id_looks_free_detection() {
        assert!(id_looks_free("foo/bar:free"));
        assert!(!id_looks_free("foo/bar"));
    }

    // --- P0-2 radar discipline ---

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zen-models-p02-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cache_for(base_url: &str, results: Vec<ModelProbe>) -> ModelsCache {
        ModelsCache {
            verified_at: chrono::Utc::now().to_rfc3339(),
            base_url: base_url.to_string(),
            results,
        }
    }

    fn paid_probe(id: &str, score: f64) -> ModelProbe {
        ModelProbe {
            model: id.to_string(),
            provider: "test".to_string(),
            context_window: None,
            free: false,
            latency_ms: 100,
            success: true,
            score,
            last_error: None,
        }
    }

    #[test]
    fn diff_pools_lists_added_removed_and_score_changes() {
        let old = vec![
            probe("keep", true, 0.8),
            probe("gone", true, 0.6),
            probe("move", true, 0.8),
        ];
        let new = vec![
            probe("keep", true, 0.8),
            probe("move", true, 0.6),
            probe("new", true, 0.5),
        ];
        let diff = diff_pools(&old, &new);
        assert_eq!(diff.added, vec!["new".to_string()]);
        assert_eq!(diff.removed, vec!["gone".to_string()]);
        assert_eq!(diff.score_changed.len(), 1);
        assert_eq!(diff.score_changed[0].0, "move");
        assert!((diff.score_changed[0].1 - 0.8).abs() < 1e-9);
        assert!((diff.score_changed[0].2 - 0.6).abs() < 1e-9);
        assert!(!diff.is_empty());

        assert!(diff_pools(&old, &old).is_empty());
        // A tiny score wobble is display noise, not a change.
        let wobble = vec![
            probe("keep", true, 0.801),
            probe("gone", true, 0.6),
            probe("move", true, 0.8),
        ];
        assert!(diff_pools(&old, &wobble).is_empty());
    }

    #[test]
    fn format_pool_diff_reports_best_change() {
        let diff = PoolDiff {
            added: vec!["a".to_string()],
            removed: vec![],
            score_changed: vec![],
        };
        let text = format_pool_diff(&diff, Some("old"), Some("new"));
        assert!(text.contains("+ added: a"));
        assert!(text.contains("best: old -> new"));
        assert_eq!(
            format_pool_diff(&PoolDiff::default(), Some("same"), Some("same")),
            "free-model pool diff: no changes"
        );
    }

    #[test]
    fn best_model_never_picks_a_paid_model() {
        let results = vec![
            probe("free-model", true, 0.5),
            paid_probe("expensive/model", 0.99),
        ];
        assert_eq!(best_model(&results).unwrap().model, "free-model");
        assert!(best_model(&[paid_probe("expensive/model", 0.99)]).is_none());
    }

    #[test]
    fn pool_switch_refused_keeps_old_pool() {
        let dir = test_dir("refused");
        let old = cache_for(
            "https://x",
            vec![probe("old-best", true, 0.9), probe("other", true, 0.5)],
        );
        save_cache(Some(&dir), &old).unwrap();
        let fresh = cache_for(
            "https://x",
            vec![probe("new-best", true, 0.95), probe("other", true, 0.5)],
        );
        let outcome = maybe_switch_pool(Some(&dir), Some(&old), &fresh, false, &|_| false);
        let kept = match outcome {
            SwitchOutcome::UseOld(r) => r,
            SwitchOutcome::UseFresh => panic!("refused switch must keep the old pool"),
        };
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].model, "old-best");
        // The cache file on disk is untouched and no backup was created.
        assert_eq!(load_cache(Some(&dir)).unwrap().results[0].model, "old-best");
        assert!(!backup_path(Some(&dir)).unwrap().is_file());
        let log = std::fs::read_to_string(radar_log_path(Some(&dir)).unwrap()).unwrap();
        assert!(
            log.contains("refused"),
            "radar log must record the refusal: {}",
            log
        );
        assert!(log.contains("old-best"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pool_switch_approved_backs_up_old_pool() {
        let dir = test_dir("approved");
        let old = cache_for("https://x", vec![probe("old-best", true, 0.9)]);
        save_cache(Some(&dir), &old).unwrap();
        let fresh = cache_for("https://x", vec![probe("new-best", true, 0.95)]);
        let outcome = maybe_switch_pool(Some(&dir), Some(&old), &fresh, false, &|_| true);
        assert!(matches!(outcome, SwitchOutcome::UseFresh));
        let bak = backup_path(Some(&dir)).unwrap();
        assert!(bak.is_file(), "backup must exist after an approved switch");
        let bak_text = std::fs::read_to_string(&bak).unwrap();
        assert!(bak_text.contains("old-best"));
        assert!(!bak_text.contains("new-best"));
        let log = std::fs::read_to_string(radar_log_path(Some(&dir)).unwrap()).unwrap();
        assert!(
            log.contains("approved (prompt)"),
            "radar log must record the approval: {}",
            log
        );
        assert!(
            log.contains("pool backup saved"),
            "radar log must record the backup: {}",
            log
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pool_switch_yes_flag_skips_prompt() {
        let dir = test_dir("yes");
        let old = cache_for("https://x", vec![probe("old-best", true, 0.9)]);
        save_cache(Some(&dir), &old).unwrap();
        let fresh = cache_for("https://x", vec![probe("new-best", true, 0.95)]);
        let outcome = maybe_switch_pool(Some(&dir), Some(&old), &fresh, true, &|_| {
            panic!("--yes must not prompt")
        });
        assert!(matches!(outcome, SwitchOutcome::UseFresh));
        assert!(backup_path(Some(&dir)).unwrap().is_file());
        let log = std::fs::read_to_string(radar_log_path(Some(&dir)).unwrap()).unwrap();
        assert!(
            log.contains("approved (--yes)"),
            "log must record the --yes approval: {}",
            log
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn score_only_change_needs_no_prompt_and_still_backs_up() {
        let dir = test_dir("score-only");
        let old = cache_for("https://x", vec![probe("m", true, 0.8)]);
        save_cache(Some(&dir), &old).unwrap();
        let fresh = cache_for("https://x", vec![probe("m", true, 0.6)]);
        let outcome = maybe_switch_pool(Some(&dir), Some(&old), &fresh, false, &|_| {
            panic!("score-only change must not prompt")
        });
        assert!(matches!(outcome, SwitchOutcome::UseFresh));
        assert!(backup_path(Some(&dir)).unwrap().is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_cache_roundtrip() {
        let dir = test_dir("restore");
        let old = cache_for("https://x", vec![probe("old-best", true, 0.9)]);
        save_cache(Some(&dir), &old).unwrap();
        assert!(backup_cache(Some(&dir)).unwrap());
        let fresh = cache_for("https://x", vec![probe("new-best", true, 0.95)]);
        save_cache(Some(&dir), &fresh).unwrap();
        assert_eq!(load_cache(Some(&dir)).unwrap().results[0].model, "new-best");
        let restored = restore_cache(Some(&dir)).unwrap();
        assert_eq!(restored.results[0].model, "old-best");
        assert_eq!(load_cache(Some(&dir)).unwrap().results[0].model, "old-best");

        // No backup file at all: restore fails with a clear error.
        let empty = test_dir("restore-empty");
        assert!(restore_cache(Some(&empty)).is_err());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }
}
