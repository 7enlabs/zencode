mod agents;
mod config;
mod delegate;
mod lsp;
mod lsp_tools;
mod manager;
mod task;
mod transport;

pub use agents::{load_agents, load_cli_agents};
pub use config::{
    McpServerConfig, load_mcp_configs, load_mcp_configs_ex, load_mcp_configs_scoped, mcp_add,
    mcp_add_json, mcp_get, mcp_list, mcp_remove, parse_env_kv, parse_headers, scope_path,
};
pub use delegate::{DelegateTool, MAX_DELEGATE_DEPTH};
pub use lsp::{LspLanguage, LspManager};
pub use lsp_tools::{FindReferencesTool, GotoDefinitionTool, HoverTool};
pub use manager::{McpManager, SkipReason};
pub use task::TaskTool;

#[cfg(test)]
mod lsp_tests;
#[cfg(test)]
mod tests;
