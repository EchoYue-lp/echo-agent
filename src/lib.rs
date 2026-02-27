pub mod agent;
pub mod compression;
pub mod error;
pub mod human_loop;
pub mod llm;
pub mod mcp;
pub mod skills;
pub mod tasks;
pub mod tools;

pub mod prelude {
    pub use crate::agent::react_agent::ReactAgent;
    pub use crate::agent::{Agent, AgentConfig, AgentRole};
    pub use crate::compression::compressor::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor, SummaryPromptBuilder,
    };
    pub use crate::compression::{
        CompressionInput, CompressionOutput, ContextCompressor, ContextManager, ForceCompressStats,
    };
    pub use crate::error::Result;
    pub use crate::mcp::{McpManager, McpServerConfig, TransportConfig};
    pub use crate::skills::{
        Skill, SkillInfo, SkillManager,
        builtin::{CalculatorSkill, FileSystemSkill, ShellSkill, WeatherSkill},
        external::{LoadedSkill, ResourceRef, SkillLoader, SkillMeta},
    };
    pub use crate::tools::{Tool, ToolParameters, ToolResult};
}
