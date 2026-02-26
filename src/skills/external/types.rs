use serde::{Deserialize, Serialize};

// ── SKILL.md Frontmatter 数据结构 ─────────────────────────────────────────────

/// SKILL.md YAML Frontmatter 的完整定义
///
/// # SKILL.md 示例
///
/// ```yaml
/// ---
/// name: code_review
/// version: "1.0.0"
/// description: "专业代码审查能力"
/// author: "team"
/// tags: [code, review, quality]
/// instructions: |
///   ## 代码审查能力
///   当被要求审查代码时，使用以下流程...
/// resources:
///   - name: checklist
///     path: checklist.md
///     description: "审查清单"
///   - name: style_guide
///     path: style_guide.md
///     description: "代码风格规范"
///     load_on_startup: true
/// ---
/// ```
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SkillMeta {
    /// 技能唯一标识（小写下划线，全局唯一）
    pub name: String,

    /// 语义化版本号（如 "1.0.0"）
    pub version: Option<String>,

    /// 技能功能的简短描述（1-2 句话，注入 system prompt 时作为摘要）
    pub description: String,

    /// 作者信息
    pub author: Option<String>,

    /// 分类标签，用于搜索和过滤
    pub tags: Option<Vec<String>>,

    /// 注入到 Agent system prompt 的指引文本
    ///
    /// 告诉 LLM 这个 Skill 的能力边界、使用时机和行为约定。
    /// 支持 YAML 多行字符串（`|`），内容会原样追加到 system prompt。
    /// 若为 None，则只注入 name + description 作为摘要。
    pub instructions: Option<String>,

    /// 本 Skill 引用的外部资源文件（懒加载）
    ///
    /// 这些文件存放在与 SKILL.md 相同的目录下，
    /// LLM 可通过 `load_skill_resource` 工具按需加载。
    pub resources: Option<Vec<ResourceRef>>,
}

impl SkillMeta {
    /// 生成注入到 system prompt 的文本块
    pub fn to_prompt_block(&self) -> String {
        let version_str = self
            .version
            .as_ref()
            .map(|v| format!(" v{}", v))
            .unwrap_or_default();

        let tags_str = self
            .tags
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| format!(" [{}]", t.join(", ")))
            .unwrap_or_default();

        let mut block = format!(
            "\n\n## Skill: {}{}{}\n**描述**: {}",
            self.name, version_str, tags_str, self.description
        );

        if let Some(instructions) = &self.instructions {
            block.push('\n');
            block.push_str(instructions.trim());
        }

        // 若有资源，告知 LLM 可以按需加载
        if let Some(resources) = &self.resources {
            if !resources.is_empty() {
                block.push_str("\n\n**可用参考资源**（需要时调用 `load_skill_resource` 加载）：");
                for res in resources {
                    let desc = res.description.as_deref().unwrap_or("");
                    block.push_str(&format!("\n- `{}/{}`: {}", self.name, res.name, desc));
                }
            }
        }

        block
    }

    /// 获取需要在启动时立即加载的资源
    pub fn startup_resources(&self) -> Vec<&ResourceRef> {
        self.resources
            .as_ref()
            .map(|rs| {
                rs.iter()
                    .filter(|r| r.load_on_startup.unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── ResourceRef ───────────────────────────────────────────────────────────────

/// 技能目录中一个可引用的资源文件
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResourceRef {
    /// 资源唯一名称（在本技能内唯一，作为加载时的 key）
    pub name: String,

    /// 相对于技能目录（SKILL.md 所在目录）的文件路径
    pub path: String,

    /// 资源用途描述（注入 system prompt，让 LLM 知道何时使用）
    pub description: Option<String>,

    /// 是否在 Agent 启动时立即加载并注入 system prompt（默认 false）
    ///
    /// 适合较小的配置文件或核心规范文档。
    /// 大文件建议保持默认 false，由 LLM 按需调用 `load_skill_resource` 获取。
    pub load_on_startup: Option<bool>,
}

// ── LoadedSkill ───────────────────────────────────────────────────────────────

/// 已成功加载的技能（包含元数据和技能目录路径）
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    /// 解析后的 frontmatter 元数据
    pub meta: SkillMeta,
    /// SKILL.md 所在的目录（用于后续资源解析）
    pub skill_dir: std::path::PathBuf,
}
