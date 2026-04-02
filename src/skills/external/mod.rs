pub mod activate_tool;
pub mod loader;
pub mod prompt_exec;
pub mod resource_tool;
pub mod run_script_tool;
pub mod types;

pub use activate_tool::ActivateSkillTool;
pub use loader::{DiscoveryScope, SkillLoader, parse_skill_md};
pub use prompt_exec::{PromptContext, SkillSource, process_skill_content};
pub use resource_tool::ReadSkillResourceTool;
pub use run_script_tool::RunSkillScriptTool;
pub use types::{
    LegacyResourceRef, SkillContent, SkillDescriptor, SkillResourceEntry, SkillResourceKind,
};

// Backward compatibility re-exports
#[allow(deprecated)]
pub use resource_tool::LoadSkillResourceTool;
#[allow(deprecated)]
pub use types::{LoadedSkill, ResourceRef, SkillMeta};
