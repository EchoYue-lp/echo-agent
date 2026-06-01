//! Agent mode definitions and mode engine interface
//!
//! Provides a framework-level `AgentMode` enum and `ModeEngine` trait that any
//! consumer can use to configure agents for specific domains (coding, research,
//! data analysis, writing). The `DefaultModeEngine` supplies English-language
//! defaults; `LocalizedModeEngine` (in the facade crate) allows injecting
//! localized prompt overrides.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Domain-specific agent operating mode.
///
/// Each mode carries a default system prompt template and a set of recommended
/// tool names. Framework consumers can use `ModeEngine` to retrieve these
/// defaults, then override them at the application layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum AgentMode {
    /// General-purpose assistant (no domain specialization)
    General,
    /// Code reading, writing, debugging, refactoring
    Coding,
    /// Academic paper search, analysis, literature review
    Research,
    /// Data analysis, statistics, visualization
    Data,
    /// Writing, editing, formatting documents
    Writing,
}

impl AgentMode {
    /// Parse a mode name (English) into an `AgentMode`.
    ///
    /// Supports: "general", "coding"/"code", "research", "data", "writing".
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "general" => Some(AgentMode::General),
            "coding" | "code" => Some(AgentMode::Coding),
            "research" => Some(AgentMode::Research),
            "data" => Some(AgentMode::Data),
            "writing" => Some(AgentMode::Writing),
            _ => None,
        }
    }

    /// All currently defined modes.
    pub fn all() -> &'static [AgentMode] {
        &[
            AgentMode::General,
            AgentMode::Coding,
            AgentMode::Research,
            AgentMode::Data,
            AgentMode::Writing,
        ]
    }

    /// English display name for the mode.
    pub fn name(&self) -> &str {
        match self {
            AgentMode::General => "General",
            AgentMode::Coding => "Coding",
            AgentMode::Research => "Research",
            AgentMode::Data => "Data Analysis",
            AgentMode::Writing => "Writing",
        }
    }
}

impl fmt::Display for AgentMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Configuration produced by a `ModeEngine` for a given mode.
#[derive(Debug, Clone)]
pub struct ModeConfig {
    /// System prompt template that defines the agent's personality and workflow.
    pub system_prompt_template: String,
    /// Recommended tool names that the agent should have access to.
    /// Empty list means "all registered tools" (no restriction).
    pub recommended_tools: Vec<String>,
    /// Human-readable display name.
    pub display_name: String,
    /// Short icon/emoji for UI rendering.
    pub icon: String,
}

/// Trait for retrieving mode-specific configuration.
///
/// The `DefaultModeEngine` provides English defaults. Applications can
/// implement their own `ModeEngine` to supply localized prompts or
/// domain-specific tool selections.
pub trait ModeEngine: Send + Sync {
    /// Retrieve the full configuration for a mode.
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig;

    /// List all supported modes.
    fn all_modes(&self) -> Vec<AgentMode> {
        AgentMode::all().to_vec()
    }

    /// Retrieve the system prompt template for a mode.
    fn system_prompt(&self, mode: &AgentMode) -> String {
        self.mode_config(mode).system_prompt_template
    }

    /// Retrieve the recommended tool names for a mode.
    fn recommended_tools(&self, mode: &AgentMode) -> Vec<String> {
        self.mode_config(mode).recommended_tools
    }
}

/// Default mode engine with English-language prompt templates.
///
/// These prompts are designed as neutral defaults that any framework consumer
/// can use. Applications should override with `LocalizedModeEngine` or custom
/// `ModeEngine` implementations for specific locales or domain tuning.
pub struct DefaultModeEngine;

impl ModeEngine for DefaultModeEngine {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig {
        match mode {
            AgentMode::General => ModeConfig {
                system_prompt_template: "You are an intelligent assistant that can answer \
                    questions and help complete tasks. Use tools when needed to gather \
                    information or perform actions.".into(),
                recommended_tools: vec![],
                display_name: "General".into(),
                icon: "💬".into(),
            },
            AgentMode::Coding => ModeConfig {
                system_prompt_template: "You are a professional coding assistant. You can read, \
                    write, debug, and refactor code. Before modifying code, understand the existing \
                    structure and logic. Follow the project's code style and conventions. Provide \
                    clear, safe modifications and explain your changes. Seek user confirmation for \
                    dangerous operations (deleting files, running commands).\n\n\
                    Workflow:\n\
                    1. Understand requirements: read relevant code, understand the context\n\
                    2. Design plan: explain the approach and impact scope before modifying\n\
                    3. Implement: write code following project style\n\
                    4. Verify: run tests to confirm correctness\n\
                    5. Summarize: explain what was done and why".into(),
                recommended_tools: vec![
                    "shell".into(),
                    "file_read".into(),
                    "file_write".into(),
                    "file_list".into(),
                    "file_delete".into(),
                    "code_search".into(),
                    "git".into(),
                ],
                display_name: "Coding".into(),
                icon: "💻".into(),
            },
            AgentMode::Research => ModeConfig {
                system_prompt_template: "You are an academic research assistant. You excel at \
                    searching, analyzing, and summarizing academic papers and research information.\n\n\
                    Research workflow:\n\
                    1. Clarify research questions and keywords\n\
                    2. Search multiple academic databases using arxiv_search and semantic_scholar_search\n\
                    3. Download and read important papers via pdf_fetch\n\
                    4. Cross-validate information, compare methodologies and conclusions\n\
                    5. Manage citations using bibtex_generate\n\
                    6. Produce structured literature reviews and research reports\n\n\
                    When writing papers, generate academically rigorous text with proper citations, \
                    ensuring every claim is supported by sources.".into(),
                recommended_tools: vec![
                    "arxiv_search".into(),
                    "semantic_scholar_search".into(),
                    "pdf_fetch".into(),
                    "bibtex_generate".into(),
                    "web_search".into(),
                    "web_fetch".into(),
                    "file_read".into(),
                    "file_write".into(),
                ],
                display_name: "Research".into(),
                icon: "🔬".into(),
            },
            AgentMode::Data => ModeConfig {
                system_prompt_template: "You are a data analysis assistant. You can read and analyze \
                    data files (CSV, Excel, JSON, Parquet, etc.), perform data cleaning and \
                    transformations, generate statistical summaries, create visualizations, and \
                    provide data-driven insights.\n\n\
                    Analysis workflow:\n\
                    1. Understand the problem: clarify analysis goals and key metrics\n\
                    2. Data exploration: use profile_data to understand data structure, types, and quality\n\
                    3. Data cleaning: handle missing values, outliers, and type inconsistencies\n\
                    4. Execute analysis: choose appropriate statistical methods and tools\n\
                    5. Visualize: use generate_chart to present key findings (line, bar, pie, scatter)\n\
                    6. Conclude: provide data-driven insights with confidence levels and limitations\n\n\
                    Prefer sampling and aggregation for large datasets. Always report sample sizes \
                    and statistical significance.".into(),
                recommended_tools: vec![
                    "file_read".into(),
                    "read_data".into(),
                    "data_stats".into(),
                    "profile_data".into(),
                    "filter_data".into(),
                    "aggregate_data".into(),
                    "generate_chart".into(),
                    "sample_data".into(),
                    "correlate_data".into(),
                    "pivot_data".into(),
                    "time_series".into(),
                    "hypothesis_test".into(),
                    "regression".into(),
                    "missing_value_analysis".into(),
                    "outlier_detection".into(),
                    "consistency_check".into(),
                ],
                display_name: "Data Analysis".into(),
                icon: "📊".into(),
            },
            AgentMode::Writing => ModeConfig {
                system_prompt_template: "You are a writing assistant. You specialize in drafting, \
                    editing, and polishing various text content, including technical documentation, \
                    articles, reports, and emails. You adapt writing style based on target audience \
                    and context.\n\n\
                    Writing workflow:\n\
                    1. Clarify goals: audience, purpose, length requirements\n\
                    2. Build outline: determine main sections and logical structure\n\
                    3. Draft content: write section by section\n\
                    4. Polish: review logic, grammar, and expression\n\
                    5. Output: support Markdown, LaTeX, DOCX formats".into(),
                recommended_tools: vec![
                    "file_read".into(),
                    "file_write".into(),
                    "web_search".into(),
                    "web_fetch".into(),
                ],
                display_name: "Writing".into(),
                icon: "✍️".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_mode_from_name() {
        assert_eq!(AgentMode::from_name("general"), Some(AgentMode::General));
        assert_eq!(AgentMode::from_name("coding"), Some(AgentMode::Coding));
        assert_eq!(AgentMode::from_name("code"), Some(AgentMode::Coding));
        assert_eq!(AgentMode::from_name("research"), Some(AgentMode::Research));
        assert_eq!(AgentMode::from_name("data"), Some(AgentMode::Data));
        assert_eq!(AgentMode::from_name("writing"), Some(AgentMode::Writing));
        assert_eq!(AgentMode::from_name("unknown"), None);
    }

    #[test]
    fn test_agent_mode_all() {
        assert_eq!(AgentMode::all().len(), 5);
    }

    #[test]
    fn test_default_mode_engine() {
        let engine = DefaultModeEngine;
        let config = engine.mode_config(&AgentMode::Coding);
        assert!(!config.system_prompt_template.is_empty());
        assert!(!config.recommended_tools.is_empty());
        assert_eq!(config.recommended_tools.len(), 7);
    }

    #[test]
    fn test_default_mode_engine_recommended_tools() {
        let engine = DefaultModeEngine;
        assert!(engine.recommended_tools(&AgentMode::General).is_empty());
        assert_eq!(engine.recommended_tools(&AgentMode::Research).len(), 8);
        assert_eq!(engine.recommended_tools(&AgentMode::Data).len(), 16);
        assert_eq!(engine.recommended_tools(&AgentMode::Writing).len(), 4);
    }

    #[test]
    fn test_mode_config_display() {
        let engine = DefaultModeEngine;
        let config = engine.mode_config(&AgentMode::Coding);
        assert_eq!(config.display_name, "Coding");
        assert_eq!(config.icon, "💻");
    }

    #[test]
    fn test_agent_mode_display() {
        assert_eq!(AgentMode::Coding.to_string(), "Coding");
        assert_eq!(AgentMode::Research.to_string(), "Research");
    }

    #[test]
    fn test_agent_mode_serde() {
        let mode = AgentMode::Coding;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"coding\"");
        let decoded: AgentMode = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, AgentMode::Coding);
    }
}