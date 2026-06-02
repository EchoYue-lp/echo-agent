//! Localized mode engine with Chinese-language prompt overrides
//!
//! Wraps `DefaultModeEngine` and allows injecting locale-specific
//! system prompt overrides per mode. The `LocalizedModeEngine::with_chinese()`
//! factory provides the Chinese prompts currently used in the CLI application.

use echo_core::agent::mode::{AgentMode, DefaultModeEngine, ModeConfig, ModeEngine};
use std::collections::HashMap;

/// Mode engine that supports localized prompt overrides.
///
/// Falls back to `DefaultModeEngine` for any mode that doesn't have
/// an override, preserving the recommended tool list and other config
/// from the default engine.
pub struct LocalizedModeEngine {
    defaults: DefaultModeEngine,
    prompt_overrides: HashMap<AgentMode, String>,
    /// Additional display name overrides (e.g. Chinese names).
    display_name_overrides: HashMap<AgentMode, String>,
}

impl LocalizedModeEngine {
    /// Create a new engine with no overrides (equivalent to `DefaultModeEngine`).
    pub fn new() -> Self {
        Self {
            defaults: DefaultModeEngine,
            prompt_overrides: HashMap::new(),
            display_name_overrides: HashMap::new(),
        }
    }

    /// Create with Chinese-language prompt overrides.
    ///
    /// This moves the Chinese prompts from the CLI's `modes.rs` into the
    /// framework level, making them available to any consumer.
    pub fn with_chinese() -> Self {
        let mut overrides = HashMap::new();
        overrides.insert(
            AgentMode::General,
            "你是一个智能助手，可以回答各种问题并帮助用户完成任务。当需要时，你可以使用工具来获取信息或执行操作。".into(),
        );
        overrides.insert(
            AgentMode::Coding,
            "你是一个专业的编程助手。你可以阅读、编写、调试和重构代码。在修改代码前，先理解现有代码的结构和逻辑。遵循项目的代码风格和约定。提供清晰、安全的代码修改，并解释你的变更。当执行危险操作（如删除文件、运行命令）时，需要获得用户确认。\n\n\
             工作流程：\n\
             1. 理解需求：先阅读相关代码，理解上下文\n\
             2. 设计方案：修改前说明计划和影响范围\n\
             3. 实施修改：编写代码，遵循项目风格\n\
             4. 验证结果：运行测试确认修改正确\n\
             5. 总结变更：说明做了什么、为什么".into(),
        );
        overrides.insert(
            AgentMode::Research,
            "你是一个学术研究助手。你擅长搜索、分析和总结学术论文与研究信息。在进行研究时，你会：\n\
             1. 明确研究问题和关键词\n\
             2. 使用 arxiv_search 和 semantic_scholar_search 搜索多个学术数据库\n\
             3. 用 pdf_fetch 下载并阅读重要论文\n\
             4. 交叉验证信息，比较不同研究的方法和结论\n\
             5. 用 bibtex_generate 理引用\n\
             6. 给出结构化的文献综述和研究报告\n\n\
             当撰写论文时，你会生成带完整引用的学术文本，确保每个论点都有来源支持。".into(),
        );
        overrides.insert(
            AgentMode::Data,
            "你是一个数据分析助手。你可以读取和分析数据文件（CSV、Excel、JSON、Parquet 等），进行数据清洗和转换，生成统计摘要，创建可视化图表，并提供数据驱动的洞察。\n\n\
             分析流程：\n\
             1. 理解问题：明确分析目标和关键指标\n\
             2. 数据探索：用 profile_data 了解数据结构、类型和质量\n\
             3. 数据清洗：处理缺失值、异常值和类型不一致\n\
             4. 分析执行：选择合适的统计方法和工具\n\
             5. 可视化：用 generate_chart 呈现关键发现\n\
             6. 结论：给出数据驱动的洞察和建议，附带置信度和局限性说明\n\n\
             对大数据集优先使用采样和聚合，避免全量加载。始终报告样本量和统计显著性。".into(),
        );
        overrides.insert(
            AgentMode::Writing,
            "你是一个写作助手。你擅长撰写、编辑和优化各类文本内容，包括技术文档、文章、报告、邮件等。你会根据目标受众和场景调整写作风格。\n\n\
             写作流程：\n\
             1. 明确目标：受众、用途、篇幅要求\n\
             2. 构建大纲：确定主要章节和逻辑结构\n\
             3. 撰写初稿：按章节逐步完成\n\
             4. 优化润色：检查逻辑、语法和表达\n\
             5. 输出文件：支持 Markdown、LaTeX、DOCX 格式".into(),
        );

        let mut display_names = HashMap::new();
        display_names.insert(AgentMode::General, "通用".into());
        display_names.insert(AgentMode::Coding, "编程".into());
        display_names.insert(AgentMode::Research, "研究".into());
        display_names.insert(AgentMode::Data, "数据".into());
        display_names.insert(AgentMode::Writing, "写作".into());

        Self {
            defaults: DefaultModeEngine,
            prompt_overrides: overrides,
            display_name_overrides: display_names,
        }
    }

    /// Set a prompt override for a specific mode.
    pub fn with_override(mut self, mode: AgentMode, prompt: String) -> Self {
        self.prompt_overrides.insert(mode, prompt);
        self
    }

    /// Set a display name override for a specific mode.
    pub fn with_display_name(mut self, mode: AgentMode, name: String) -> Self {
        self.display_name_overrides.insert(mode, name);
        self
    }

    /// Parse mode name supporting both English and Chinese aliases.
    pub fn from_str(s: &str) -> Option<AgentMode> {
        match s.to_lowercase().as_str() {
            "general" | "通用" => Some(AgentMode::General),
            "coding" | "code" | "编程" | "代码" => Some(AgentMode::Coding),
            "research" | "研究" => Some(AgentMode::Research),
            "data" | "数据分析" | "数据" => Some(AgentMode::Data),
            "writing" | "写作" | "写" => Some(AgentMode::Writing),
            _ => AgentMode::from_name(s),
        }
    }
}

impl Default for LocalizedModeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ModeEngine for LocalizedModeEngine {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig {
        let base = self.defaults.mode_config(mode);

        // Override system prompt if a localized version exists
        let system_prompt = self
            .prompt_overrides
            .get(mode)
            .cloned()
            .unwrap_or(base.system_prompt_template);

        // Override display name if a localized version exists
        let display_name = self
            .display_name_overrides
            .get(mode)
            .cloned()
            .unwrap_or(base.display_name);

        ModeConfig {
            system_prompt_template: system_prompt,
            recommended_tools: base.recommended_tools,
            display_name,
            icon: base.icon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_localized_mode_engine_chinese() {
        let engine = LocalizedModeEngine::with_chinese();
        let config = engine.mode_config(&AgentMode::Coding);
        assert!(config.system_prompt_template.contains("编程助手"));
        assert_eq!(config.display_name, "编程");
        assert_eq!(config.recommended_tools.len(), 7);
    }

    #[test]
    fn test_localized_mode_engine_fallback() {
        let engine = LocalizedModeEngine::new(); // no overrides
        let config = engine.mode_config(&AgentMode::Coding);
        assert!(config.system_prompt_template.contains("coding assistant"));
        assert_eq!(config.display_name, "Coding");
    }

    #[test]
    fn test_from_str_chinese() {
        let engine = LocalizedModeEngine::with_chinese();
        assert_eq!(
            LocalizedModeEngine::from_str("编程"),
            Some(AgentMode::Coding)
        );
        assert_eq!(
            LocalizedModeEngine::from_str("代码"),
            Some(AgentMode::Coding)
        );
        assert_eq!(
            LocalizedModeEngine::from_str("研究"),
            Some(AgentMode::Research)
        );
        assert_eq!(LocalizedModeEngine::from_str("数据"), Some(AgentMode::Data));
        assert_eq!(
            LocalizedModeEngine::from_str("写作"),
            Some(AgentMode::Writing)
        );
    }

    #[test]
    fn test_from_str_english() {
        let engine = LocalizedModeEngine::with_chinese();
        assert_eq!(
            LocalizedModeEngine::from_str("coding"),
            Some(AgentMode::Coding)
        );
        assert_eq!(
            LocalizedModeEngine::from_str("research"),
            Some(AgentMode::Research)
        );
    }

    #[test]
    fn test_custom_override() {
        let engine = LocalizedModeEngine::new()
            .with_override(AgentMode::Coding, "Custom coding prompt".into());
        let config = engine.mode_config(&AgentMode::Coding);
        assert_eq!(config.system_prompt_template, "Custom coding prompt");
        // Recommended tools still from defaults
        assert_eq!(config.recommended_tools.len(), 7);
    }
}
