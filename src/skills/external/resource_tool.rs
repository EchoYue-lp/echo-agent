use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

use super::loader::SkillLoader;

/// 技能资源懒加载工具
///
/// 当 Agent 安装了带有 `resources` 字段的外部技能后，
/// 此工具会自动注册到 ToolManager，让 LLM 可以按需加载资源文件内容。
///
/// # 工作流程
///
/// ```text
/// LLM 需要代码审查清单
///   → 调用 load_skill_resource("code_review", "checklist")
///   → SkillLoader 检查缓存（命中则直接返回）
///   → 未命中时读取 skills/code_review/checklist.md
///   → 文件内容作为工具结果返回到 LLM 上下文
/// ```
pub struct LoadSkillResourceTool {
    loader: Arc<Mutex<SkillLoader>>,
    /// 工具描述中展示的资源目录（让 LLM 知道有哪些可用资源）
    resource_catalog_desc: String,
}

impl LoadSkillResourceTool {
    pub fn new(loader: Arc<Mutex<SkillLoader>>) -> Self {
        // 在构造时快照资源目录，用于工具描述
        // （后续如果 loader 动态更新，可以重新构造此工具）
        Self {
            loader,
            resource_catalog_desc: String::new(),
        }
    }

    /// 构建时注入资源目录描述（由 load_skills_from_dir 负责调用）
    pub fn with_catalog_desc(mut self, desc: String) -> Self {
        self.resource_catalog_desc = desc;
        self
    }
}

#[async_trait]
impl Tool for LoadSkillResourceTool {
    fn name(&self) -> &str {
        "load_skill_resource"
    }

    fn description(&self) -> &str {
        "按需加载技能的参考资源文件内容（如规范文档、检查清单、模板等）。\
         当你需要更多背景信息或参考资料时调用此工具。"
    }

    fn parameters(&self) -> serde_json::Value {
        let catalog = if self.resource_catalog_desc.is_empty() {
            "（资源目录将在运行时动态提供）".to_string()
        } else {
            self.resource_catalog_desc.clone()
        };

        json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": format!(
                        "技能名称。可用资源目录：\n{}",
                        catalog
                    )
                },
                "resource_name": {
                    "type": "string",
                    "description": "资源名称（与 SKILL.md 中 resources[].name 对应）"
                }
            },
            "required": ["skill_name", "resource_name"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        let skill_name = parameters
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("skill_name".to_string()))?
            .to_string();

        let resource_name = parameters
            .get("resource_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("resource_name".to_string()))?
            .to_string();

        let mut loader = self.loader.lock().await;
        match loader.load_resource(&skill_name, &resource_name).await {
            Ok(content) => {
                let header = format!("# 资源: {}/{}\n\n", skill_name, resource_name);
                Ok(ToolResult::success(format!("{}{}", header, content)))
            }
            Err(e) => Ok(ToolResult::error(format!(
                "加载资源 '{}/{}' 失败: {}",
                skill_name, resource_name, e
            ))),
        }
    }
}
