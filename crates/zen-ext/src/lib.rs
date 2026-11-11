mod commands;
mod hooks;
mod memory;
mod settings;
mod skills;
#[cfg(test)]
mod testutil;
mod trust;

pub use commands::{
    ExpandContext, SlashCommand, SlashSource, discover_custom_commands, discover_slash_commands,
    expand_command, expand_slash_prompt, find_slash_command, split_args,
};
pub use hooks::{CommandHookSpec, CommandHooks, HOOK_OUTPUT_LIMIT, OnFailure, matches_target};
pub use memory::{
    MANAGED_MEMORY_PATH, MAX_IMPORT_DEPTH, MemoryFile, append_memory_note,
    detect_import_candidates, memories_report, memory_files, memory_files_with_managed,
    memory_prompt,
};
pub use settings::{
    LoadedSettings, load_settings, load_settings_trust, load_settings_with_managed, merge_json,
};
pub use skills::{
    Frontmatter, SkillTool, discover_skill_entries, discover_skills, load_skill, load_skill_entry,
    skills_prompt, split_frontmatter,
};
pub use trust::{
    TRUST_ENV, gated_project_config, is_folder_trusted, project_mcp_files, trust_env_set,
    trust_folder, trust_store_path,
};
