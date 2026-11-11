#![allow(clippy::module_inception)] // floor-guard: module agent shares the crate name, rename deferred

pub mod budget;
mod client;
pub mod compaction;
pub mod finish;
pub mod guard;
pub mod loop_guard;
pub mod plan;
pub mod providers;
pub mod prune;
pub mod repair;
pub mod safety;
mod subagent;

pub mod agent;
pub mod rategate;
pub mod retry;
pub mod tool_bundles;
pub use agent::{
    Agent, AgentConfig, DEFAULT_MAX_CONTINUATIONS, DEFAULT_MAX_PARALLEL_TOOL_CALLS,
    DEFAULT_STOP_HOOK_CONTINUES, DEFAULT_TOOL_TIMEOUT_MS,
};
pub use client::{
    AnthropicClient, HttpTimeouts, OpenAiClient, is_anthropic_url, make_backend, make_backend_with,
    request_id_from_error,
};
pub use providers::{ApiStandard, ModelSpec, ProviderInfo};
pub use rategate::{DEFAULT_DAILY_LIMIT, RateGate, RateGateSettings};
pub use safety::{ErrorClass, classify_error, user_agent};
pub use subagent::SubAgentExecutor;
pub use tool_bundles::{ToolBundle, ToolPhase};

#[cfg(test)]
mod testutil;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_p0;
#[cfg(test)]
mod tests_p045;
#[cfg(test)]
mod tests_p0b;
#[cfg(test)]
mod tests_timeout;
#[cfg(test)]
mod tests_trunc;
