//! Skill 系统
//!
//! Skill 是比 Tool 更高层次的能力单元，将一组相关 Tool 与系统提示词注入片段打包，
//! 通过 [`crate::agent::react_agent::ReactAgent::add_skill`] 一次性安装到 Agent。

pub mod builtin;
pub mod external;

use std::collections::HashMap;

use crate::tools::Tool;

// ── Skill Trait ───────────────────────────────────────────────────────────────

/// Agent 技能（Skill）
///
/// Skill 是比 Tool 更高层次的能力单元，代表 Agent 的一个专业领域能力。
/// 它将一组相关 Tool 与对应的 LLM 指引（system_prompt_injection）打包在一起，
/// 作为一个整体安装到 Agent 上。
///
/// # Skill vs Tool
///
/// | 维度 | Tool | Skill |
/// |------|------|-------|
/// | 粒度 | 单一原子操作 | 领域能力包（多 Tool + Prompt 片段） |
/// | 注册 | `agent.add_tool(box)` | `agent.add_skill(box)` |
/// | Prompt | 无 | 可携带指引 LLM 的 prompt injection |
/// | 语义 | "做一件事" | "我掌握某个领域" |
///
/// # 实现示例
///
/// ```rust
/// use echo_agent::skills::Skill;
/// use echo_agent::tools::{Tool, ToolResult, ToolParameters};
///
/// struct MySkill;
///
/// impl Skill for MySkill {
///     fn name(&self) -> &str { "my_skill" }
///     fn description(&self) -> &str { "这是一个示例技能" }
///     fn tools(&self) -> Vec<Box<dyn Tool>> { vec![] }
///     fn system_prompt_injection(&self) -> Option<String> {
///         Some("当需要XXX时，使用 my_tool 工具。".to_string())
///     }
/// }
/// ```
pub trait Skill: Send + Sync {
    /// Skill 唯一标识名（建议小写下划线，如 "calculator"）
    fn name(&self) -> &str;

    /// 人类可读的功能描述（展示给开发者）
    fn description(&self) -> &str;

    /// 此 Skill 提供的工具集合
    ///
    /// 每次调用都应返回新的 Tool 实例（因为 Box<dyn Tool> 无法 Clone）。
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// 注入到 Agent 系统提示词末尾的指引片段（可选）
    ///
    /// 告诉 LLM 这组工具的用途、何时使用以及使用约定。
    /// 该文本会在 `agent.add_skill()` 时追加到 `AgentConfig::system_prompt`。
    fn system_prompt_injection(&self) -> Option<String> {
        None
    }
}

// ── SkillInfo ─────────────────────────────────────────────────────────────────

/// 已注册 Skill 的元数据快照（用于查询/展示，不持有原 Skill 对象）
#[derive(Debug, Clone)]
pub struct SkillInfo {
    /// Skill 标识名
    pub name: String,
    /// 功能描述
    pub description: String,
    /// 该 Skill 提供的工具名称列表
    pub tool_names: Vec<String>,
    /// 是否有系统提示词注入
    pub has_prompt_injection: bool,
}

// ── SkillManager ──────────────────────────────────────────────────────────────

/// Skill 管理器
///
/// 跟踪已向 Agent 注册的所有 Skill，提供查询和统计能力。
/// 注意：SkillManager 本身不执行 Tool 注册和 Prompt 注入
/// （这些操作在 `ReactAgent::add_skill()` 中完成）。
pub struct SkillManager {
    skills: HashMap<String, SkillInfo>,
}

impl SkillManager {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// 记录一个已被 Agent 安装的 Skill
    pub(crate) fn record(&mut self, info: SkillInfo) {
        self.skills.insert(info.name.clone(), info);
    }

    /// 查询是否已安装某 Skill
    pub fn is_installed(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }

    /// 获取已安装的 Skill 数量
    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// 列出所有已安装 Skill 的元数据
    pub fn list(&self) -> Vec<&SkillInfo> {
        let mut infos: Vec<&SkillInfo> = self.skills.values().collect();
        infos.sort_by_key(|i| &i.name);
        infos
    }

    /// 获取某个 Skill 的元数据
    pub fn get(&self, name: &str) -> Option<&SkillInfo> {
        self.skills.get(name)
    }
}

impl Default for SkillManager {
    fn default() -> Self {
        Self::new()
    }
}
