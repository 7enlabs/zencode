use crate::cli::{Cli, PermissionMode};
use anyhow::Result;
use zen_core::{PermissionSettings, Settings};

pub const DEFAULT_BASE_URL: &str = "https://api.experientiallabs.ai/api/v1";
pub const DEFAULT_MODEL: &str = "deepseek-v4.1-flash";
pub const DEFAULT_MAX_TURNS: usize = 50;

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub model: String,
    /// True when the session runs in auto model mode: the model was
    /// requested as `auto`, or no model was chosen anywhere (auto is
    /// the default). An explicitly pinned model - even the provider's
    /// default - is not auto (PR #72 review follow-up).
    pub model_auto: bool,

    pub btw_model: Option<String>,
    /// Merged "telemetry" setting ("off" opts out of the anonymous
    /// user counter, Viec 7); read from the settings extra map.
    pub telemetry: Option<String>,
    pub base_url: String,
    pub api_key: Option<String>,

    pub api_key_env: Option<String>,

    pub allow_private_network: bool,

    pub ollama: bool,
    pub fallback_models: Vec<String>,
    pub permissions: PermissionSettings,
    pub permission_mode: PermissionMode,
    pub hooks: serde_json::Value,
    pub env: std::collections::HashMap<String, String>,
    pub auto_compact: Option<serde_json::Value>,

    pub auto_commit: bool,

    pub verify: Option<serde_json::Value>,

    pub retry_max_wait_secs: u64,

    pub request_timeout_secs: Option<u64>,

    pub idle_timeout_secs: Option<u64>,

    pub tool_timeouts: std::collections::HashMap<String, u64>,

    pub max_continuations: usize,

    pub tool_bundle: zen_agent::ToolBundle,

    pub auto_archive_days: u64,

    pub max_parallel_tool_calls: usize,
    pub max_turns: usize,

    pub max_budget_usd: Option<f64>,

    pub budget_warn_at: Option<f64>,
    pub context_window: Option<u64>,
    pub loaded_sources: Vec<String>,
    pub warnings: Vec<String>,
    pub setting_sources: Vec<String>,
    pub bypass_forbidden: bool,

    pub folder_trusted: bool,

    pub trust_skipped: Vec<String>,

    pub rate_gate: zen_agent::RateGateSettings,

    pub mention_token_budget: u64,
}

impl ResolvedConfig {
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn llm_client(
        &self,
        sink: std::sync::Arc<dyn zen_core::EventSink>,
    ) -> std::sync::Arc<dyn zen_core::LlmBackend> {
        zen_agent::make_backend_with(
            self.base_url.clone(),
            self.api_key.clone(),
            sink,
            self.retry_max_wait_secs.saturating_mul(1000),
            self.http_timeouts(),
        )
    }

    pub fn http_timeouts(&self) -> zen_agent::HttpTimeouts {
        zen_agent::HttpTimeouts::from_secs(
            &self.base_url,
            self.request_timeout_secs,
            self.idle_timeout_secs,
        )
    }

    pub fn rate_gate_for(
        &self,
        home: Option<&std::path::Path>,
    ) -> (zen_agent::RateGateSettings, Option<std::path::PathBuf>) {
        let store_path = home.map(zen_agent::RateGate::default_store_path);
        (self.rate_gate.clone(), store_path)
    }

    pub fn gated_llm_client(
        &self,
        sink: std::sync::Arc<dyn zen_core::EventSink>,
        home: Option<&std::path::Path>,
    ) -> std::sync::Arc<dyn zen_core::LlmBackend> {
        self.gated_llm_client_for(&self.base_url, self.api_key.clone(), sink, home)
    }

    /// Build the rate-gated backend for an explicit provider endpoint
    /// and key (used by `/model` when a switch changes provider).
    pub fn gated_llm_client_for(
        &self,
        base_url: &str,
        api_key: Option<String>,
        sink: std::sync::Arc<dyn zen_core::EventSink>,
        home: Option<&std::path::Path>,
    ) -> std::sync::Arc<dyn zen_core::LlmBackend> {
        let client = zen_agent::make_backend_with(
            base_url.to_string(),
            api_key,
            sink,
            self.retry_max_wait_secs.saturating_mul(1000),
            zen_agent::HttpTimeouts::from_secs(
                base_url,
                self.request_timeout_secs,
                self.idle_timeout_secs,
            ),
        );
        let (settings, store_path) = self.rate_gate_for(home);
        std::sync::Arc::new(zen_agent::RateGate::new(
            client,
            base_url.to_string(),
            settings,
            store_path,
        ))
    }
}

pub fn tool_timeouts(v: Option<&serde_json::Value>) -> std::collections::HashMap<String, u64> {
    v.and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                .collect()
        })
        .unwrap_or_default()
}

pub fn max_continuations(v: Option<&serde_json::Value>) -> usize {
    v.and_then(|v| v.as_u64())
        .map_or(zen_agent::DEFAULT_MAX_CONTINUATIONS, |n| n as usize)
}

fn tool_bundle(v: Option<&serde_json::Value>) -> zen_agent::ToolBundle {
    v.and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

pub fn sessions_auto_archive_days(v: Option<&serde_json::Value>) -> u64 {
    v.and_then(|v| v.get("autoArchiveDays"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0)
}

pub fn api_timeout_secs(
    api: Option<&serde_json::Value>,
    key: &str,
    env: Option<&str>,
) -> Option<u64> {
    let num = |v: &serde_json::Value| {
        v.as_u64()
            .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
    };
    if let Some(n) = env
        .and_then(|e| e.trim().parse::<f64>().ok())
        .filter(|f| *f >= 0.0)
    {
        return Some(n as u64);
    }
    api.and_then(|a| a.get(key)).and_then(num)
}

pub fn max_parallel_tool_calls(v: Option<&serde_json::Value>) -> usize {
    v.and_then(|v| v.as_u64())
        .map_or(zen_agent::DEFAULT_MAX_PARALLEL_TOOL_CALLS, |n| n as usize)
        .max(1)
}

pub const DEFAULT_RETRY_MAX_WAIT_SECS: u64 = 300;

pub fn retry_max_wait_secs(retry: Option<&serde_json::Value>) -> u64 {
    retry
        .and_then(|r| r.get("maxWaitSecs"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        })
        .unwrap_or(DEFAULT_RETRY_MAX_WAIT_SECS)
}

pub fn rate_gate_settings(cfg: &zen_core::RateGateFileConfig) -> zen_agent::RateGateSettings {
    zen_agent::RateGateSettings {
        enabled: cfg.enabled.unwrap_or(true),
        daily_limit: cfg.daily_limit.unwrap_or(zen_agent::DEFAULT_DAILY_LIMIT),
        limits: cfg.limits.clone(),
    }
}

pub fn merge_settings(base: &Settings, over: &Settings) -> Settings {
    let mut out = base.clone();
    if over.model.is_some() {
        out.model = over.model.clone();
    }
    if over.base_url.is_some() {
        out.base_url = over.base_url.clone();
    }
    if over.api_key_env.is_some() {
        out.api_key_env = over.api_key_env.clone();
    }
    dedup_append(&mut out.fallback_models, &over.fallback_models);

    dedup_append(&mut out.permissions.allow, &over.permissions.allow);
    dedup_append(&mut out.permissions.ask, &over.permissions.ask);
    dedup_append(&mut out.permissions.deny, &over.permissions.deny);
    if over.permissions.default_mode.is_some() {
        out.permissions.default_mode = over.permissions.default_mode.clone();
    }
    dedup_append(
        &mut out.permissions.additional_directories,
        &over.permissions.additional_directories,
    );

    for (k, v) in &over.env {
        out.env.insert(k.clone(), v.clone());
    }
    if let Some(obj) = over.hooks.as_object() {
        if !out.hooks.is_object() {
            out.hooks = serde_json::Value::Object(Default::default());
        }
        if let Some(dst) = out.hooks.as_object_mut() {
            for (k, v) in obj {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    if over.auto_compact.is_some() {
        out.auto_compact = over.auto_compact.clone();
    }
    if over.auto_commit.is_some() {
        out.auto_commit = over.auto_commit;
    }
    if over.max_turns.is_some() {
        out.max_turns = over.max_turns;
    }
    if over.context_window.is_some() {
        out.context_window = over.context_window;
    }

    if over.rate_gate.enabled.is_some() {
        out.rate_gate.enabled = over.rate_gate.enabled;
    }
    if over.rate_gate.daily_limit.is_some() {
        out.rate_gate.daily_limit = over.rate_gate.daily_limit;
    }
    for (k, v) in &over.rate_gate.limits {
        out.rate_gate.limits.insert(k.clone(), *v);
    }
    out
}

fn dedup_append<T: PartialEq + Clone>(target: &mut Vec<T>, source: &[T]) {
    for item in source {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

fn resolve_model(cli: &Cli, settings_model: Option<&str>) -> String {
    if let Some(m) = &cli.model {
        return m.clone();
    }
    if let Ok(v) = std::env::var("ZEN_MODEL") {
        if !v.is_empty() {
            return v;
        }
    }
    settings_model
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn resolve_base_url(cli: &Cli, settings_url: Option<&str>) -> String {
    if let Some(u) = &cli.base_url {
        return u.clone();
    }
    if let Ok(v) = std::env::var("ZEN_BASE_URL") {
        if !v.is_empty() {
            return v;
        }
    }
    settings_url
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// Route a catalog model to the provider that serves it: rewrite
/// `cfg.model` to the id that provider accepts, and - when the base
/// URL is still the default - point `base_url` and `api_key` at that
/// provider. A custom base URL is the user's explicit endpoint
/// choice: routing leaves the whole config untouched then, so a
/// catalog id is never rewritten for a gateway that would reject
/// the rewritten form (PR #71 review F2/F3). Returns Ok(true) when
/// the model was routed. Shared by the REPL and headless entry
/// points so every path sends an id its gateway accepts.
pub fn route_catalog_model(cfg: &mut ResolvedConfig) -> anyhow::Result<bool> {
    let routed = route_catalog_model_inner(cfg)?;
    normalize_fallback_models(cfg);
    Ok(routed)
}

fn route_catalog_model_inner(cfg: &mut ResolvedConfig) -> anyhow::Result<bool> {
    // Auto has no catalog meaning for the Ollama provider (PR #72
    // review follow-up): refuse here with the real reason instead of
    // letting the literal "auto" reach the Ollama readiness check,
    // which would report the misleading hint `ollama pull auto`.
    if cfg.ollama && crate::models::is_auto_model(&cfg.model) {
        anyhow::bail!(
            "model '{}' is not supported with the Ollama provider; \
             name a local model instead (e.g. --model ollama/qwen3-coder:30b)",
            cfg.model
        );
    }
    if cfg.ollama {
        return Ok(false);
    }
    // Auto mode (Leader tin 7962): `auto` means the current
    // provider's best free model. Resolve it to that entry's id
    // first so the routing below treats it like any catalog pick.
    if crate::models::is_auto_model(&cfg.model) {
        let Some(provider) = crate::models::provider_for_base_url(&cfg.base_url) else {
            anyhow::bail!(
                "model '{}' needs a known provider base URL; set a model id instead",
                cfg.model
            );
        };
        let Some(entry) = crate::models::provider_default_model(provider) else {
            anyhow::bail!("provider '{provider}' has no free model in the catalog");
        };
        cfg.model = entry.id.to_string();
    }
    let Some(entry) = crate::models::catalog_entry(&cfg.model) else {
        return Ok(false);
    };
    let Some(base_url) = crate::models::provider_base_url(entry.provider) else {
        return Ok(false);
    };
    if cfg.base_url == DEFAULT_BASE_URL && base_url != DEFAULT_BASE_URL {
        // Cross-provider: the request must carry that provider's key.
        if let Some(env) = crate::models::provider_api_key_env(entry.provider) {
            match std::env::var(env).ok().filter(|v| !v.is_empty()) {
                Some(key) => cfg.api_key = Some(key),
                None => anyhow::bail!(
                    "model '{}' needs its provider's key: set {} before starting",
                    entry.id,
                    env
                ),
            }
        }
        cfg.base_url = base_url.to_string();
        cfg.model = entry.gateway_id.to_string();
        return Ok(true);
    }
    if cfg.base_url == DEFAULT_BASE_URL || cfg.base_url == base_url {
        cfg.model = entry.gateway_id.to_string();
        return Ok(true);
    }
    Ok(false)
}

/// Bring `cfg.fallback_models` in line with the routed primary: the
/// fallback chain sends every candidate through the one client bound
/// to `cfg.base_url`, so a catalog fallback must be translated to
/// the gateway id its provider accepts (as the primary is), and a
/// catalog entry served by a different provider can only fail with
/// a 400 - drop it with a warning instead of letting it pose as a
/// working fallback (bug hunt B P2-4). A custom base URL has no
/// known provider, so nothing is dropped there.
fn normalize_fallback_models(cfg: &mut ResolvedConfig) {
    let provider = crate::models::provider_for_base_url(&cfg.base_url);
    cfg.fallback_models.retain(|id| {
        match (provider, crate::models::catalog_entry(id)) {
            (Some(p), Some(entry)) if entry.provider != p => {
                eprintln!(
                    "warning: fallback model '{id}' is served by provider '{}', not '{p}'; ignoring it",
                    entry.provider
                );
                false
            }
            _ => true,
        }
    });
    for id in &mut cfg.fallback_models {
        *id = crate::models::gateway_model_id(id).to_string();
    }
}

/// Route `cfg` to a catalog entry's provider: the model string that
/// provider accepts, that provider's base URL, and its key. Unlike
/// `route_catalog_model`, an explicit base URL does NOT win here: the
/// entry names a specific provider's model (a `/model` switch or a
/// slash-command `model:` pin), and sending it to another gateway
/// fails with HTTP 400 "Invalid Model Name" (PR #71). `home_api_key`
/// is the key resolved at startup for the configured route, restored
/// for the home provider (which has no dedicated env var). On error
/// (the provider key is unset) `cfg` is left untouched.
pub fn route_to_catalog_entry(
    cfg: &mut ResolvedConfig,
    entry: &crate::models::CatalogModel,
    home_api_key: &Option<String>,
) -> anyhow::Result<()> {
    let Some(base_url) = crate::models::provider_base_url(entry.provider) else {
        return Ok(());
    };
    let key = match crate::models::provider_api_key_env(entry.provider) {
        Some(env) => match std::env::var(env).ok().filter(|v| !v.is_empty()) {
            Some(k) => Some(k),
            None => anyhow::bail!(
                "model '{}' needs its provider's key: set {} before starting",
                entry.id,
                env
            ),
        },
        None => home_api_key.clone(),
    };
    cfg.base_url = base_url.to_string();
    cfg.api_key = key;
    cfg.model = entry.gateway_id.to_string();
    Ok(())
}

fn resolve_api_key(api_key_env: Option<&str>) -> Option<String> {
    if let Ok(v) = std::env::var("ZEN_API_KEY") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Some(env_name) = api_key_env {
        if !env_name.is_empty() {
            if let Ok(v) = std::env::var(env_name) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn resolve_permission_mode(
    cli: &Cli,
    default_mode: Option<&str>,
    bypass_forbidden: bool,
) -> Result<PermissionMode> {
    if let Some(m) = &cli.permission_mode {
        if bypass_forbidden && m == &PermissionMode::BypassPermissions {
            anyhow::bail!(
                "bypassPermissions mode is disabled by settings (disableBypassPermissionsMode)"
            );
        }
        return Ok(*m);
    }
    if let Some(m) = default_mode {
        if let Some(parsed) = PermissionMode::parse(m) {
            if bypass_forbidden && parsed == PermissionMode::BypassPermissions {
                anyhow::bail!(
                    "bypassPermissions mode is disabled by settings (disableBypassPermissionsMode)"
                );
            }
            return Ok(parsed);
        }
    }
    Ok(PermissionMode::Default)
}

fn sources_filter(raw: &Option<String>) -> Option<Vec<String>> {
    raw.as_ref().map(|s| {
        s.split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect::<Vec<String>>()
    })
}

fn split_rules(inputs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in inputs {
        for part in raw.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() && !out.contains(&trimmed.to_string()) {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

pub fn resolve_config(cli: &Cli, cwd: &std::path::Path) -> anyhow::Result<ResolvedConfig> {
    resolve_config_with_home(cli, cwd, dirs::home_dir().as_deref())
}

/// Project and local settings are honored only in a trusted folder.
/// Otherwise the project layers are reported as skipped, so checking out
/// an unfamiliar repository cannot silently change the model, API keys,
/// permissions, or hooks used for the session.
pub fn resolve_config_with_home(
    cli: &Cli,
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
) -> anyhow::Result<ResolvedConfig> {
    let enabled_sources: Vec<String> = match &cli.setting_sources {
        Some(s) => s
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect(),
        None => vec![
            "user".to_string(),
            "project".to_string(),
            "local".to_string(),
        ],
    };
    let sources_opt = sources_filter(&cli.setting_sources);
    let folder_trusted = folder_trusted(cli, cwd, home);
    let loaded = zen_ext::load_settings_trust(
        cwd,
        home,
        cli.settings.as_deref(),
        sources_opt.as_deref(),
        folder_trusted,
    );
    let trust_skipped = if folder_trusted {
        Vec::new()
    } else {
        zen_ext::gated_project_config(cwd)
    };
    let merged = loaded.settings;
    let loaded_sources = loaded.sources;
    let mut warnings = loaded.warnings;
    if cli.permission_mode.is_none() && merged.permissions.default_mode.as_deref() == Some("auto") {
        warnings.push("permission mode 'auto' is not supported; using 'default'".to_string());
    }

    if !cli.print {
        for w in &warnings {
            eprintln!("warning: {}", w);
        }
    }

    let raw_model = resolve_model(cli, merged.model.as_deref());
    // Auto model mode (PR #72 review follow-up): on when the model
    // was requested as `auto` or when no model was chosen anywhere -
    // an explicit pin of the provider's default model is still a pin.
    let model_auto = crate::models::is_auto_model(&raw_model)
        || (cli.model.is_none()
            && std::env::var("ZEN_MODEL")
                .map(|v| v.is_empty())
                .unwrap_or(true)
            && merged.model.as_deref().is_none_or(|s| s.is_empty()));
    let btw_model = cli
        .btw_model
        .clone()
        .or_else(|| crate::btw::settings_btw_model(&merged.extra));
    let telemetry = merged
        .extra
        .get("telemetry")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Catalog ids win over direct provider routing (bug hunt fix #4):
    // `deepseek/deepseek-r1:free` is a free OpenRouter catalog entry,
    // but its `deepseek/` prefix is also a registry provider, so
    // ModelSpec::parse would strip it and route_direct would send it to
    // DeepSeek's paid API. Check the catalog first; only non-catalog
    // ids take the direct path.
    let is_catalog_id = crate::models::catalog_entry(&raw_model).is_some();
    let spec = zen_agent::providers::ModelSpec::parse(&raw_model, DEFAULT_BASE_URL, false);
    let provider_key = spec
        .api_key_env
        .as_deref()
        .and_then(|e| std::env::var(e).ok())
        .filter(|v| !v.is_empty());
    let route_direct = spec.provider != "default" && provider_key.is_some() && !is_catalog_id;
    let model = if route_direct {
        spec.model.clone()
    } else {
        raw_model.clone()
    };
    let mut base_url = resolve_base_url(cli, merged.base_url.as_deref());
    if route_direct {
        let base_url_was_default = cli.base_url.is_none()
            && std::env::var("ZEN_BASE_URL")
                .map(|v| v.is_empty())
                .unwrap_or(true)
            && merged.base_url.as_deref().is_none_or(|u| u.is_empty())
            && base_url == DEFAULT_BASE_URL;
        if base_url_was_default {
            base_url = spec.base_url.clone();
        }
    } else if spec.provider != "default"
        && spec.provider != "ollama"
        && !is_catalog_id
        && (cli.model.is_some()
            || std::env::var("ZEN_MODEL")
                .map(|v| !v.is_empty())
                .unwrap_or(false))
    {
        warnings.push(format!(
            "provider '{}' recognized but {} is not set; sending '{}' to the default gateway",
            spec.provider,
            spec.api_key_env.as_deref().unwrap_or("its API key"),
            raw_model,
        ));
        warnings.push(format!(
            "set {} for direct routing to {}",
            spec.api_key_env.as_deref().unwrap_or("the API key"),
            spec.provider,
        ));
    }
    let api_key = match provider_key {
        Some(k) => Some(k),
        None => resolve_api_key(merged.api_key_env.as_deref()),
    };

    let provider = cli
        .provider
        .clone()
        .or_else(|| std::env::var("ZEN_PROVIDER").ok().filter(|s| !s.is_empty()));
    if let Some(p) = provider.as_deref() {
        if p != "ollama" {
            anyhow::bail!("unknown --provider '{p}' (supported providers: ollama)");
        }
    }
    let base_url_explicit = cli.base_url.is_some()
        || std::env::var("ZEN_BASE_URL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        || merged.base_url.as_deref().is_some_and(|s| !s.is_empty());
    let (model, base_url, api_key, ollama) = crate::ollama::apply_ollama_routing(
        provider.as_deref() == Some("ollama"),
        &model,
        &base_url,
        base_url_explicit,
        api_key,
        &crate::ollama::ollama_base_url(),
    );
    if ollama && model.is_empty() {
        anyhow::bail!(
            "empty model name after 'ollama/' prefix (e.g. --model ollama/qwen3-coder:30b)"
        );
    }

    let mut cli_allow = split_rules(&cli.allowed_tools);
    let mut cli_deny = split_rules(&cli.disallowed_tools);

    let mut allow = merged.permissions.allow.clone();
    dedup_append(&mut allow, &cli_allow);
    cli_allow = allow;
    let mut deny = merged.permissions.deny.clone();
    dedup_append(&mut deny, &cli_deny);
    cli_deny = deny;

    let permission_mode = resolve_permission_mode(
        cli,
        merged.permissions.default_mode.as_deref(),
        loaded.bypass_forbidden,
    )?;

    let max_turns = cli
        .max_turns
        .or(merged.max_turns)
        .unwrap_or(DEFAULT_MAX_TURNS);

    Ok(ResolvedConfig {
        model,
        model_auto,
        btw_model,
        telemetry,
        base_url,
        api_key,
        api_key_env: merged.api_key_env.clone(),
        allow_private_network: merged.allow_private_network,
        ollama,
        fallback_models: merged
            .fallback_models
            .iter()
            .map(|m| {
                crate::ollama::strip_ollama_prefix(m)
                    .unwrap_or(m)
                    .to_string()
            })
            .collect(),
        permissions: PermissionSettings {
            allow: cli_allow,
            ask: merged.permissions.ask.clone(),
            deny: cli_deny,
            default_mode: Some(permission_mode.as_str().to_string()),
            additional_directories: merged.permissions.additional_directories.clone(),
        },
        permission_mode,

        hooks: if merged.extra.get("disableAllHooks") == Some(&serde_json::Value::Bool(true)) {
            serde_json::json!({})
        } else {
            merged.hooks.clone()
        },
        env: merged.env.clone(),
        auto_compact: merged.auto_compact.clone(),
        auto_commit: merged.auto_commit.unwrap_or(false),
        verify: merged.extra.get("verify").cloned(),
        retry_max_wait_secs: retry_max_wait_secs(merged.extra.get("retry")),
        request_timeout_secs: api_timeout_secs(
            merged.extra.get("api"),
            "timeoutSecs",
            std::env::var("ZEN_REQUEST_TIMEOUT").ok().as_deref(),
        ),
        idle_timeout_secs: api_timeout_secs(
            merged.extra.get("api"),
            "idleTimeoutSecs",
            std::env::var("ZEN_IDLE_TIMEOUT").ok().as_deref(),
        ),
        tool_timeouts: tool_timeouts(merged.extra.get("toolTimeouts")),
        max_continuations: max_continuations(merged.extra.get("maxContinuations")),
        tool_bundle: tool_bundle(merged.extra.get("toolBundle")),
        auto_archive_days: sessions_auto_archive_days(merged.extra.get("sessions")),
        max_parallel_tool_calls: max_parallel_tool_calls(merged.extra.get("maxParallelToolCalls")),
        max_turns,
        max_budget_usd: merged.extra.get("maxBudgetUsd").and_then(|v| v.as_f64()),
        budget_warn_at: merged.extra.get("budgetWarnAt").and_then(|v| v.as_f64()),
        context_window: merged.context_window,
        loaded_sources,
        warnings,
        setting_sources: enabled_sources.clone(),
        bypass_forbidden: loaded.bypass_forbidden,
        folder_trusted,
        trust_skipped,
        rate_gate: rate_gate_settings(&merged.rate_gate),
        mention_token_budget: crate::mentions::mention_token_budget(merged.extra.get("mentions")),
    })
}

/// A folder counts as trusted if any one of these holds: the explicit
/// --trust-folder flag, the trust environment variable, or a previously
/// stored trust record for this folder.
pub fn folder_trusted(cli: &Cli, cwd: &std::path::Path, home: Option<&std::path::Path>) -> bool {
    cli.trust_folder || zen_ext::trust_env_set() || zen_ext::is_folder_trusted(cwd, home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;
    use std::fs;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn base_cli() -> Cli {
        Cli::parse_from(["zen"])
    }

    #[test]
    fn config_precedence_model_cli_over_env_over_default() {
        let _env = env_lock();
        let mut cli = base_cli();
        cli.model = Some("cli-model".into());

        let old = std::env::var("ZEN_MODEL").ok();
        std::env::set_var("ZEN_MODEL", "env-model");
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        std::env::remove_var("ZEN_MODEL");
        if let Some(o) = old {
            std::env::set_var("ZEN_MODEL", o);
        }
        assert_eq!(cfg.model, "cli-model");
    }

    #[test]
    fn config_precedence_env_over_default_when_no_cli() {
        let _env = env_lock();
        let cli = base_cli();
        let old = std::env::var("ZEN_MODEL").ok();
        std::env::set_var("ZEN_MODEL", "env-model");
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        std::env::remove_var("ZEN_MODEL");
        if let Some(o) = old {
            std::env::set_var("ZEN_MODEL", o);
        }
        assert_eq!(cfg.model, "env-model");
    }

    #[test]
    fn config_default_model() {
        let _env = env_lock();
        let cli = base_cli();
        let old_m = std::env::var("ZEN_MODEL").ok();
        let old_b = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        // Isolate from the real user settings file as well as the ambient
        // env: defaults must hold with no user/project/local layer at all.
        let dir = std::env::temp_dir().join(format!("zen_cfg_def_{}", std::process::id()));
        let home = std::env::temp_dir().join(format!("zen_cfg_def_home_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&home).unwrap();
        let cfg = resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        if let Some(o) = old_m {
            std::env::set_var("ZEN_MODEL", o);
        }
        if let Some(o) = old_b {
            std::env::set_var("ZEN_BASE_URL", o);
        }
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&home);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(cfg.permission_mode, PermissionMode::Default);
    }

    #[test]
    fn config_permission_mode_precedence() {
        let _env = env_lock();
        let mut cli = base_cli();
        cli.permission_mode = Some(PermissionMode::Plan);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.permission_mode, PermissionMode::Plan);
    }

    #[test]
    fn config_default_mode_plan_from_settings_file() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_test_plan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        fs::write(
            dir.join(".zen/settings.json"),
            r#"{"permissions":{"defaultMode":"plan"}}"#,
        )
        .unwrap();

        let cli = Cli::parse_from(["zen", "--setting-sources", "project", "--trust-folder"]);
        let cfg = resolve_config(&cli, &dir).unwrap();
        assert_eq!(cfg.permission_mode, PermissionMode::Plan);
    }

    #[test]
    fn config_settings_file_merge() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        fs::write(
            dir.join(".zen/settings.json"),
            r#"{"model":"from-file","baseUrl":"https://file.example.com/v1","permissions":{"defaultMode":"acceptEdits","allow":["Read"]}}"#,
        )
        .unwrap();

        // Env overrides beat files, so a ZEN_MODEL/ZEN_BASE_URL left in the
        // ambient environment would mask the project file. Resolve against
        // an empty user home so the real user settings file cannot leak in.
        let old_m = std::env::var("ZEN_MODEL").ok();
        let old_b = std::env::var("ZEN_BASE_URL").ok();
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        let home = std::env::temp_dir().join(format!("zen_cfg_test_home_{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        let cli = Cli::parse_from(["zen", "--setting-sources", "project", "--trust-folder"]);
        let cfg = resolve_config_with_home(&cli, &dir, Some(&home)).unwrap();
        if let Some(o) = old_m {
            std::env::set_var("ZEN_MODEL", o);
        }
        if let Some(o) = old_b {
            std::env::set_var("ZEN_BASE_URL", o);
        }
        let _ = fs::remove_dir_all(&home);
        assert_eq!(cfg.model, "from-file");
        assert_eq!(cfg.base_url, "https://file.example.com/v1");
        assert_eq!(cfg.permission_mode, PermissionMode::AcceptEdits);
        assert!(cfg.permissions.allow.contains(&"Read".to_string()));
        assert!(!cfg.loaded_sources.is_empty());
    }

    #[test]
    fn config_cli_model_overrides_settings() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_test2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        fs::write(dir.join(".zen/settings.json"), r#"{"model":"from-file"}"#).unwrap();

        let cli = Cli::parse_from([
            "zen",
            "--model",
            "cli-override",
            "--setting-sources",
            "project",
        ]);
        let cfg = resolve_config(&cli, &dir).unwrap();
        assert_eq!(cfg.model, "cli-override");
    }

    #[test]
    fn c1_3_env_model_overrides_settings_and_flag_overrides_env() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_c13_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        fs::write(dir.join(".zen/settings.json"), r#"{"model":"from-file"}"#).unwrap();
        let old = std::env::var("ZEN_MODEL").ok();
        std::env::set_var("ZEN_MODEL", "env-model");
        let cli = Cli::parse_from(["zen", "--setting-sources", "project"]);
        let from_env = resolve_config(&cli, &dir).unwrap().model;
        let cli = Cli::parse_from([
            "zen",
            "--model",
            "flag-model",
            "--setting-sources",
            "project",
        ]);
        let from_flag = resolve_config(&cli, &dir).unwrap().model;
        std::env::remove_var("ZEN_MODEL");
        let cli = Cli::parse_from(["zen", "--setting-sources", "project"]);
        let from_file = resolve_config(&cli, &dir).unwrap().model;
        if let Some(o) = old {
            std::env::set_var("ZEN_MODEL", o);
        }
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(from_env, "env-model");
        assert_eq!(from_flag, "flag-model");
        assert_eq!(from_file, "from-file");
    }

    #[test]
    fn c2_11_settings_mode_aliases() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_c211_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        let cli = Cli::parse_from(["zen", "-p", "--setting-sources", "project"]);
        fs::write(
            dir.join(".zen/settings.json"),
            r#"{"permissions":{"defaultMode":"manual"}}"#,
        )
        .unwrap();
        let manual = resolve_config(&cli, &dir).unwrap();
        fs::write(
            dir.join(".zen/settings.json"),
            r#"{"permissions":{"defaultMode":"auto"}}"#,
        )
        .unwrap();
        let auto = resolve_config(&cli, &dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(manual.permission_mode, PermissionMode::Default);
        assert!(manual.warnings.is_empty());
        assert_eq!(auto.permission_mode, PermissionMode::Default);
        assert!(
            auto.warnings.iter().any(|w| w.contains("'auto'")),
            "{:?}",
            auto.warnings
        );
    }

    #[test]
    fn config_allowed_tools_split_and_dedup() {
        let _env = env_lock();
        let cli = Cli::parse_from([
            "zen",
            "--allowedTools",
            "Read,Bash",
            "--allowedTools",
            "Bash,Write",
        ]);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(
            cfg.permissions.allow,
            vec!["Read".to_string(), "Bash".to_string(), "Write".to_string()]
        );
    }

    #[test]
    fn config_deny_dedup() {
        let _env = env_lock();
        let cli = Cli::parse_from([
            "zen",
            "--disallowedTools",
            "Write",
            "--disallowedTools",
            "Write",
        ]);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.permissions.deny, vec!["Write".to_string()]);
    }

    #[test]
    fn config_no_settings_file_ok() {
        let _env = env_lock();
        let cli = base_cli();
        let dir = std::env::temp_dir().join(format!("zen_cfg_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = resolve_config(&cli, &dir).unwrap();
        assert!(cfg.warnings.is_empty());
        assert!(cfg.loaded_sources.is_empty());
    }

    #[test]
    fn sessions_auto_archive_days_parsing() {
        use serde_json::json;
        assert_eq!(sessions_auto_archive_days(None), 0);
        assert_eq!(sessions_auto_archive_days(Some(&json!({}))), 0);
        assert_eq!(
            sessions_auto_archive_days(Some(&json!({"autoArchiveDays": 30}))),
            30
        );
        assert_eq!(
            sessions_auto_archive_days(Some(&json!({"autoArchiveDays": 0}))),
            0
        );

        assert_eq!(
            sessions_auto_archive_days(Some(&json!({"autoArchiveDays": -5}))),
            0
        );
        assert_eq!(
            sessions_auto_archive_days(Some(&json!({"autoArchiveDays": "soon"}))),
            0
        );
    }

    #[test]
    fn config_setting_sources_filter() {
        let _env = env_lock();
        let cli = Cli::parse_from(["zen", "--setting-sources", ""]);
        assert!(cli.setting_sources.is_some());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.setting_sources.is_empty());
    }

    #[test]
    fn folder_trust_gates_project_hooks_and_allow_rules() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_trust_{}", std::process::id()));
        let home = dir.join("home");
        let proj = dir.join("proj");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(proj.join(".zen")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            proj.join(".zen/settings.json"),
            r#"{"model":"pm","hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]},"permissions":{"allow":["Bash"]}}"#,
        )
        .unwrap();
        let old = std::env::var(zen_ext::TRUST_ENV).ok();
        std::env::remove_var(zen_ext::TRUST_ENV);

        let cfg = resolve_config_with_home(&base_cli(), &proj, Some(&home)).unwrap();
        assert!(!cfg.folder_trusted);
        assert_eq!(cfg.model, "pm");
        assert!(cfg.hooks.get("Stop").is_none());
        assert!(cfg.permissions.allow.is_empty());
        assert_eq!(
            cfg.trust_skipped,
            vec![".zen/settings.json: hooks, permissions.allow"]
        );

        let cli = Cli::parse_from(["zen", "--trust-folder"]);
        let cfg = resolve_config_with_home(&cli, &proj, Some(&home)).unwrap();
        assert!(cfg.folder_trusted && cfg.hooks.get("Stop").is_some());
        assert!(!zen_ext::is_folder_trusted(&proj, Some(&home)));

        std::env::set_var(zen_ext::TRUST_ENV, "1");
        let cfg = resolve_config_with_home(&base_cli(), &proj, Some(&home)).unwrap();
        std::env::remove_var(zen_ext::TRUST_ENV);
        assert!(cfg.folder_trusted && cfg.permissions.allow == vec!["Bash"]);

        zen_ext::trust_folder(&proj, Some(&home)).unwrap();
        assert!(home.join(".zen/trusted-folders.json").is_file());
        let cfg = resolve_config_with_home(&base_cli(), &proj, Some(&home)).unwrap();
        assert!(
            cfg.folder_trusted && cfg.hooks.get("Stop").is_some() && cfg.trust_skipped.is_empty()
        );

        if let Some(o) = old {
            std::env::set_var(zen_ext::TRUST_ENV, o);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn c3_1_disable_all_hooks_clears_hooks() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("zen_cfg_nohooks_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".zen")).unwrap();
        fs::write(
            dir.join(".zen/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let cli = Cli::parse_from([
            "zen",
            "--setting-sources",
            "project,local",
            "--trust-folder",
        ]);
        let cfg = resolve_config(&cli, &dir).unwrap();
        assert!(cfg.hooks.get("Stop").is_some());

        fs::write(
            dir.join(".zen/settings.local.json"),
            r#"{"disableAllHooks":true}"#,
        )
        .unwrap();
        let cfg = resolve_config(&cli, &dir).unwrap();
        assert_eq!(cfg.hooks, serde_json::json!({}));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trunc_max_continuations_parse() {
        assert_eq!(
            max_continuations(None),
            zen_agent::DEFAULT_MAX_CONTINUATIONS
        );
        assert_eq!(max_continuations(Some(&serde_json::json!(0))), 0);
        assert_eq!(max_continuations(Some(&serde_json::json!(5))), 5);
        assert_eq!(
            max_continuations(Some(&serde_json::json!(-1))),
            zen_agent::DEFAULT_MAX_CONTINUATIONS
        );
        assert_eq!(
            max_continuations(Some(&serde_json::json!("x"))),
            zen_agent::DEFAULT_MAX_CONTINUATIONS
        );
    }

    #[test]
    fn max_parallel_tool_calls_parse() {
        assert_eq!(
            max_parallel_tool_calls(None),
            zen_agent::DEFAULT_MAX_PARALLEL_TOOL_CALLS
        );
        assert_eq!(max_parallel_tool_calls(Some(&serde_json::json!(8))), 8);

        assert_eq!(max_parallel_tool_calls(Some(&serde_json::json!(0))), 1);
        assert_eq!(max_parallel_tool_calls(Some(&serde_json::json!(1))), 1);
        assert_eq!(
            max_parallel_tool_calls(Some(&serde_json::json!(-1))),
            zen_agent::DEFAULT_MAX_PARALLEL_TOOL_CALLS
        );
        assert_eq!(
            max_parallel_tool_calls(Some(&serde_json::json!("x"))),
            zen_agent::DEFAULT_MAX_PARALLEL_TOOL_CALLS
        );
    }

    #[test]
    fn p0b_2_tool_timeouts_parse() {
        let t = tool_timeouts(Some(
            &serde_json::json!({"Bash": 180000, "default": 0, "Bad": "x", "Neg": -1}),
        ));
        assert_eq!(t.get("Bash"), Some(&180000));
        assert_eq!(t.get("default"), Some(&0));
        assert_eq!(t.len(), 2);
        assert!(tool_timeouts(None).is_empty());
        assert!(tool_timeouts(Some(&serde_json::json!([1]))).is_empty());
    }

    #[test]
    fn p0_2_api_timeouts_parse_env_over_settings() {
        let api = serde_json::json!({"timeoutSecs": 600, "idleTimeoutSecs": 30});
        assert_eq!(api_timeout_secs(Some(&api), "timeoutSecs", None), Some(600));
        assert_eq!(
            api_timeout_secs(Some(&api), "idleTimeoutSecs", None),
            Some(30)
        );
        assert_eq!(
            api_timeout_secs(Some(&api), "timeoutSecs", Some("45")),
            Some(45),
            "env wins"
        );
        assert_eq!(
            api_timeout_secs(Some(&api), "timeoutSecs", Some("junk")),
            Some(600)
        );
        assert_eq!(
            api_timeout_secs(
                Some(&serde_json::json!({"timeoutSecs": 0})),
                "timeoutSecs",
                None
            ),
            Some(0)
        );
        assert_eq!(
            api_timeout_secs(
                Some(&serde_json::json!({"timeoutSecs": -1})),
                "timeoutSecs",
                None
            ),
            None
        );
        assert_eq!(api_timeout_secs(None, "timeoutSecs", None), None);

        let t = zen_agent::HttpTimeouts::from_secs("https://openrouter.ai/api/v1", None, None);
        assert_eq!(t.request, Some(std::time::Duration::from_secs(300)));
        assert_eq!(t.idle, None);
        assert_eq!(t.connect, std::time::Duration::from_secs(10));
        let local = zen_agent::HttpTimeouts::from_secs("http://localhost:11434/v1", None, Some(0));
        assert_eq!(local.request, Some(std::time::Duration::from_secs(900)));
        assert_eq!(local.idle, None);
        let off = zen_agent::HttpTimeouts::from_secs("https://x.example/v1", Some(0), Some(20));
        assert_eq!(off.request, None);
        assert_eq!(off.idle, Some(std::time::Duration::from_secs(20)));
    }

    #[test]
    fn p0_4_retry_max_wait_secs_parses_with_default() {
        use serde_json::json;
        assert_eq!(retry_max_wait_secs(None), 300);
        assert_eq!(retry_max_wait_secs(Some(&json!({}))), 300);
        assert_eq!(retry_max_wait_secs(Some(&json!({"maxWaitSecs": 30}))), 30);
        assert_eq!(retry_max_wait_secs(Some(&json!({"maxWaitSecs": 0}))), 0);
        assert_eq!(retry_max_wait_secs(Some(&json!({"maxWaitSecs": "x"}))), 300);
        assert_eq!(retry_max_wait_secs(Some(&json!({"maxWaitSecs": -5}))), 300);
    }
    #[test]
    fn provider_routing_default_model_untouched_without_key() {
        let _env = env_lock();
        let old = std::env::var("GOOGLE_API_KEY").ok();
        std::env::remove_var("GOOGLE_API_KEY");
        let old_m = std::env::var("ZEN_MODEL").ok();
        std::env::remove_var("ZEN_MODEL");
        let cli = base_cli();
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert!(cfg
            .warnings
            .iter()
            .all(|w| !w.contains("provider 'google'")));
        if let Some(o) = old {
            std::env::set_var("GOOGLE_API_KEY", o);
        }
        if let Some(o) = old_m {
            std::env::set_var("ZEN_MODEL", o);
        }
    }

    #[test]
    fn provider_routing_direct_when_key_set() {
        let _env = env_lock();
        let old = std::env::var("ZAI_API_KEY").ok();
        std::env::set_var("ZAI_API_KEY", "test-key-zai");
        let cli = Cli::parse_from(["zen", "--model", "zai/glm-5.2"]);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.model, "glm-5.2", "prefix stripped on the wire");
        assert_eq!(cfg.base_url, "https://api.z.ai/api/paas/v4");
        assert_eq!(cfg.api_key.as_deref(), Some("test-key-zai"));
        if let Some(o) = old {
            std::env::set_var("ZAI_API_KEY", o);
        } else {
            std::env::remove_var("ZAI_API_KEY");
        }
    }

    #[test]
    fn provider_routing_warns_without_key() {
        let _env = env_lock();
        let old = std::env::var("MOONSHOT_API_KEY").ok();
        std::env::remove_var("MOONSHOT_API_KEY");
        let cli = Cli::parse_from(["zen", "--model", "moonshot/kimi-k2.7"]);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(
            cfg.model, "moonshot/kimi-k2.7",
            "full spec kept for gateway"
        );
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert!(
            cfg.warnings.iter().any(|w| w.contains("MOONSHOT_API_KEY")),
            "expected key warning, got: {:?}",
            cfg.warnings
        );
        if let Some(o) = old {
            std::env::set_var("MOONSHOT_API_KEY", o);
        }
    }

    #[test]
    fn provider_routing_explicit_base_url_wins() {
        let _env = env_lock();
        let old = std::env::var("DEEPSEEK_API_KEY").ok();
        std::env::set_var("DEEPSEEK_API_KEY", "test-key-ds");
        let cli = Cli::parse_from([
            "zen",
            "--model",
            "deepseek/deepseek-v4-flash",
            "--base-url",
            "https://proxy.example.com/v1",
        ]);
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.model, "deepseek-v4-flash");
        assert_eq!(cfg.base_url, "https://proxy.example.com/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("test-key-ds"));
        if let Some(o) = old {
            std::env::set_var("DEEPSEEK_API_KEY", o);
        } else {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
    }

    #[test]
    fn catalog_id_wins_over_direct_routing_when_provider_key_set() {
        // Bug hunt fix #4: `deepseek/deepseek-r1:free` is a free OpenRouter
        // catalog entry, but its `deepseek/` prefix is also a registry
        // provider, so ModelSpec::parse used to strip it and route_direct
        // sent it to DeepSeek's paid API. Catalog ids must win.
        let _env = env_lock();
        let old_ds = std::env::var("DEEPSEEK_API_KEY").ok();
        let old_or = std::env::var("OPENROUTER_API_KEY").ok();
        let old_m = std::env::var("ZEN_MODEL").ok();
        let old_u = std::env::var("ZEN_BASE_URL").ok();
        std::env::set_var("DEEPSEEK_API_KEY", "test-key-ds");
        std::env::set_var("OPENROUTER_API_KEY", "test-key-or");
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        let cli = Cli::parse_from(["zen", "--model", "deepseek/deepseek-r1:free"]);
        let mut cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        // resolve_config alone must keep the full catalog id on the
        // default base URL (no strip, no DeepSeek re-point).
        assert_eq!(cfg.model, "deepseek/deepseek-r1:free");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert!(cfg
            .warnings
            .iter()
            .all(|w| !w.contains("provider 'deepseek'")));
        // The catalog router then sends it to OpenRouter with its key.
        assert!(route_catalog_model(&mut cfg).unwrap());
        assert_eq!(cfg.base_url, crate::models::OPENROUTER_BASE_URL);
        assert_eq!(cfg.model, "deepseek/deepseek-r1:free");
        assert_eq!(cfg.api_key.as_deref(), Some("test-key-or"));
        if let Some(o) = old_ds {
            std::env::set_var("DEEPSEEK_API_KEY", o);
        } else {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        if let Some(o) = old_or {
            std::env::set_var("OPENROUTER_API_KEY", o);
        } else {
            std::env::remove_var("OPENROUTER_API_KEY");
        }
        if let Some(o) = old_m {
            std::env::set_var("ZEN_MODEL", o);
        }
        if let Some(o) = old_u {
            std::env::set_var("ZEN_BASE_URL", o);
        }
    }

    #[test]
    fn route_catalog_model_drops_cross_provider_fallbacks() {
        let _env = env_lock();
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = DEFAULT_MODEL.to_string();
        cfg.base_url = DEFAULT_BASE_URL.to_string();
        cfg.fallback_models = vec![
            "deepseek-v4.1-flash".to_string(),
            "qwen/qwen3-coder:free".to_string(),
            "my-custom-model".to_string(),
        ];
        route_catalog_model(&mut cfg).unwrap();
        // The OpenRouter catalog id is dropped (it would only 400 on
        // the Experiential gateway); the same-provider entry and the
        // non-catalog id stay.
        assert_eq!(
            cfg.fallback_models,
            ["deepseek-v4.1-flash", "my-custom-model"]
        );
    }

    #[test]
    fn route_catalog_model_filters_fallbacks_by_the_routed_provider() {
        let _env = env_lock();
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = "deepseek/deepseek-r1:free".to_string();
        cfg.base_url = DEFAULT_BASE_URL.to_string();
        cfg.fallback_models = vec![
            "deepseek-v4.1-flash".to_string(),
            "qwen/qwen3-coder:free".to_string(),
        ];
        route_catalog_model(&mut cfg).unwrap();
        assert_eq!(cfg.base_url, crate::models::OPENROUTER_BASE_URL);
        // Filtering uses the FINAL base URL: once the primary routes
        // to OpenRouter, the Experiential entry is the foreign one.
        assert_eq!(cfg.fallback_models, ["qwen/qwen3-coder:free"]);
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_to_catalog_entry_routes_cross_provider_pin() {
        let _env = env_lock();
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = DEFAULT_MODEL.to_string();
        cfg.base_url = DEFAULT_BASE_URL.to_string();
        cfg.api_key = Some("exp-key".to_string());
        let entry = crate::models::catalog_entry("qwen/qwen3-coder:free").unwrap();
        route_to_catalog_entry(&mut cfg, entry, &None).unwrap();
        assert_eq!(cfg.base_url, crate::models::OPENROUTER_BASE_URL);
        assert_eq!(cfg.api_key.as_deref(), Some("or-key"));
        assert_eq!(cfg.model, "qwen/qwen3-coder:free");
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn catalog_id_not_rerouted_to_paid_api_without_openrouter_key() {
        // Same catalog id, only the DeepSeek key set: resolve_config must
        // still not rewrite it, and the catalog router must refuse with
        // the OpenRouter key hint instead of silently sending the id to
        // DeepSeek's paid API.
        let _env = env_lock();
        let old_ds = std::env::var("DEEPSEEK_API_KEY").ok();
        let old_or = std::env::var("OPENROUTER_API_KEY").ok();
        let old_m = std::env::var("ZEN_MODEL").ok();
        let old_u = std::env::var("ZEN_BASE_URL").ok();
        std::env::set_var("DEEPSEEK_API_KEY", "test-key-ds");
        std::env::remove_var("OPENROUTER_API_KEY");
        std::env::remove_var("ZEN_MODEL");
        std::env::remove_var("ZEN_BASE_URL");
        let cli = Cli::parse_from(["zen", "--model", "deepseek/deepseek-r1:free"]);
        let mut cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert_eq!(cfg.model, "deepseek/deepseek-r1:free");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        let err = route_catalog_model(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("OPENROUTER_API_KEY"), "{err}");
        if let Some(o) = old_ds {
            std::env::set_var("DEEPSEEK_API_KEY", o);
        } else {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        if let Some(o) = old_or {
            std::env::set_var("OPENROUTER_API_KEY", o);
        } else {
            std::env::remove_var("OPENROUTER_API_KEY");
        }
        if let Some(o) = old_m {
            std::env::set_var("ZEN_MODEL", o);
        }
        if let Some(o) = old_u {
            std::env::set_var("ZEN_BASE_URL", o);
        }
    }

    #[test]
    fn route_to_catalog_entry_pin_wins_over_explicit_base_url() {
        let _env = env_lock();
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::set_var("OPENROUTER_API_KEY", "or-key");
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = DEFAULT_MODEL.to_string();
        cfg.base_url = "http://127.0.0.1:9".to_string();
        cfg.api_key = None;
        let entry = crate::models::catalog_entry("qwen/qwen3-coder:free").unwrap();
        route_to_catalog_entry(&mut cfg, entry, &None).unwrap();
        assert_eq!(cfg.base_url, crate::models::OPENROUTER_BASE_URL);
        assert_eq!(cfg.api_key.as_deref(), Some("or-key"));
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_to_catalog_entry_missing_key_errors_without_mutating() {
        let _env = env_lock();
        let old = std::env::var("OPENROUTER_API_KEY").ok();
        std::env::remove_var("OPENROUTER_API_KEY");
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = DEFAULT_MODEL.to_string();
        cfg.base_url = DEFAULT_BASE_URL.to_string();
        cfg.api_key = Some("exp-key".to_string());
        let before = (cfg.model.clone(), cfg.base_url.clone(), cfg.api_key.clone());
        let entry = crate::models::catalog_entry("qwen/qwen3-coder:free").unwrap();
        let err = route_to_catalog_entry(&mut cfg, entry, &None).unwrap_err();
        assert!(err.to_string().contains("OPENROUTER_API_KEY"), "{err}");
        assert_eq!((cfg.model, cfg.base_url, cfg.api_key), before);
        match old {
            Some(v) => std::env::set_var("OPENROUTER_API_KEY", v),
            None => std::env::remove_var("OPENROUTER_API_KEY"),
        }
    }

    #[test]
    fn route_to_catalog_entry_restores_home_key_for_home_provider() {
        let _env = env_lock();
        let mut cfg = resolve_config(&base_cli(), std::env::temp_dir().as_path()).unwrap();
        cfg.model = "qwen/qwen3-coder:free".to_string();
        cfg.base_url = crate::models::OPENROUTER_BASE_URL.to_string();
        cfg.api_key = Some("or-key".to_string());
        let entry = crate::models::catalog_entry("deepseek-v4.1-flash").unwrap();
        route_to_catalog_entry(&mut cfg, entry, &Some("exp-key".to_string())).unwrap();
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.api_key.as_deref(), Some("exp-key"));
        assert_eq!(cfg.model, "deepseek-v4.1-flash");
    }

    fn clear_ollama_env() {
        for v in ["ZEN_MODEL", "ZEN_BASE_URL", "ZEN_PROVIDER", "OLLAMA_HOST"] {
            std::env::remove_var(v);
        }
    }

    #[test]
    fn ollama_model_prefix_routes_to_local() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.model = Some("ollama/qwen3-coder:30b".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.ollama);
        assert_eq!(cfg.model, "qwen3-coder:30b");
        assert_eq!(cfg.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.api_key, None);
    }

    #[test]
    fn ollama_provider_flag_routes_without_prefix() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.provider = Some("ollama".into());
        cli.model = Some("llama3.1".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.ollama);
        assert_eq!(cfg.model, "llama3.1");
        assert_eq!(cfg.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn model_auto_defaults_on_and_pin_turns_it_off() {
        let _env = env_lock();
        clear_ollama_env();
        // No model chosen anywhere: auto is on by default.
        let cli = base_cli();
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.model_auto);
        // Pinning even the provider's default model is a pin, not
        // auto (PR #72 review follow-up).
        let mut cli = base_cli();
        cli.model = Some(DEFAULT_MODEL.into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(!cfg.model_auto);
        // Asking for auto explicitly turns it on.
        let mut cli = base_cli();
        cli.model = Some("auto".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.model_auto);
        // Casing must not change the meaning (PR #73 review
        // follow-up): `Auto` turns auto on too.
        let mut cli = base_cli();
        cli.model = Some("Auto".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.model_auto);
    }

    #[test]
    fn ollama_provider_env_routes() {
        let _env = env_lock();
        clear_ollama_env();
        std::env::set_var("ZEN_PROVIDER", "ollama");
        let mut cli = base_cli();
        cli.model = Some("ollama/mistral:7b".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        std::env::remove_var("ZEN_PROVIDER");
        assert!(cfg.ollama);
        assert_eq!(cfg.model, "mistral:7b");
    }

    #[test]
    fn ollama_host_env_overrides_default() {
        let _env = env_lock();
        clear_ollama_env();
        std::env::set_var("OLLAMA_HOST", "192.168.1.5:11434");
        let mut cli = base_cli();
        cli.model = Some("ollama/qwen3-coder:30b".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        std::env::remove_var("OLLAMA_HOST");
        assert!(cfg.ollama);
        assert_eq!(cfg.base_url, "http://192.168.1.5:11434/v1");
    }

    #[test]
    fn ollama_explicit_base_url_wins_over_default() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.model = Some("ollama/qwen3-coder:30b".into());
        cli.base_url = Some("http://gateway.internal:11434/v1".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(cfg.ollama);
        assert_eq!(cfg.model, "qwen3-coder:30b");
        assert_eq!(cfg.base_url, "http://gateway.internal:11434/v1");
    }

    #[test]
    fn ollama_empty_model_name_rejected() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.model = Some("ollama/".into());
        let err = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap_err();
        assert!(err.to_string().contains("ollama/"), "unexpected: {err}");
    }

    #[test]
    fn unknown_provider_rejected() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.provider = Some("wat".into());
        let err = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap_err();
        assert!(
            err.to_string().contains("unknown --provider"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn non_ollama_model_untouched() {
        let _env = env_lock();
        clear_ollama_env();
        let mut cli = base_cli();
        cli.model = Some("gpt-4o".into());
        let cfg = resolve_config(&cli, std::env::temp_dir().as_path()).unwrap();
        assert!(!cfg.ollama);
        assert_eq!(cfg.model, "gpt-4o");
    }
}
