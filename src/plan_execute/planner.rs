//! Planner — 负责将用户任务分解为结构化执行计划

use super::types::{Plan, PlanStep};
use crate::error::Result;
use crate::llm::{self, LlmConfig};
use crate::llm::types::Message;
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info};

/// Planner trait — 接收任务描述，返回执行计划
pub trait Planner: Send + Sync {
    /// 根据任务描述生成执行计划
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// ── LlmPlanner ───────────────────────────────────────────────────────────────

/// 基于 LLM 的 Planner：调用大模型来分解任务
pub struct LlmPlanner {
    model: String,
    client: Arc<Client>,
    llm_config: Option<LlmConfig>,
    system_prompt: String,
}

impl LlmPlanner {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            client: Arc::new(Client::new()),
            llm_config: None,
            system_prompt: Self::default_system_prompt().to_string(),
        }
    }

    /// 使用自定义 LLM 配置
    pub fn with_llm_config(mut self, config: LlmConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    /// 自定义系统提示词
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    fn default_system_prompt() -> &'static str {
        "你是一个任务规划专家。给定一个任务，你需要将其分解为具体可执行的步骤。\n\n\
        规则：\n\
        1. 每个步骤必须是明确、可执行的\n\
        2. 步骤之间要有逻辑顺序\n\
        3. 每个步骤应该只做一件事\n\
        4. 步骤描述应该简洁但充分\n\n\
        请以 JSON 数组格式返回步骤列表，每个元素包含 description 字段。\n\
        示例：\n\
        ```json\n\
        [{\"description\": \"分析当前代码结构\"}, {\"description\": \"识别性能瓶颈\"}, {\"description\": \"实施优化方案\"}]\n\
        ```\n\
        只返回 JSON 数组，不要返回其他内容。"
    }

    /// 从 LLM 响应中解析步骤列表
    fn parse_steps(response: &str) -> Vec<PlanStep> {
        // 尝试从 markdown code block 中提取 JSON
        let json_str = if let Some(start) = response.find("```json") {
            let content = &response[start + 7..];
            if let Some(end) = content.find("```") {
                content[..end].trim()
            } else {
                response.trim()
            }
        } else if let Some(start) = response.find("```") {
            let content = &response[start + 3..];
            if let Some(end) = content.find("```") {
                content[..end].trim()
            } else {
                response.trim()
            }
        } else {
            response.trim()
        };

        // 尝试解析为 JSON 数组
        #[derive(serde::Deserialize)]
        struct StepJson {
            description: String,
        }

        if let Ok(steps) = serde_json::from_str::<Vec<StepJson>>(json_str) {
            return steps
                .into_iter()
                .map(|s| PlanStep::new(s.description))
                .collect();
        }

        // 回退：按行拆分
        response
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .filter(|line| {
                // 过滤 markdown / code fence / 空行
                !line.starts_with("```") && !line.starts_with('#')
            })
            .map(|line| {
                // 去除序号前缀
                let cleaned = line
                    .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == ' ');
                PlanStep::new(if cleaned.is_empty() { line } else { cleaned })
            })
            .collect()
    }
}

impl Planner for LlmPlanner {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            info!(model = %self.model, "📐 LlmPlanner: 生成执行计划");

            let messages = vec![
                Message::system(self.system_prompt.clone()),
                Message::user(format!("请为以下任务制定执行计划：\n\n{}", task)),
            ];

            let response = llm::chat(
                self.client.clone(),
                &self.model,
                messages,
                Some(0.3), // 低温度以获得更稳定的规划
                Some(4096u32),
                Some(false),
                None,
                None,
                None,
            )
            .await?;

            let content = response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default();

            debug!(response = %content, "LlmPlanner 原始响应");

            let steps = Self::parse_steps(&content);

            if steps.is_empty() {
                // 至少创建一个步骤
                Ok(Plan::new(vec![PlanStep::new(task)]).with_goal(task))
            } else {
                Ok(Plan::new(steps).with_goal(task))
            }
        })
    }
}

// ── StaticPlanner ────────────────────────────────────────────────────────────

/// 静态 Planner：使用预定义的步骤列表（适用于测试或固定工作流）
pub struct StaticPlanner {
    steps: Vec<String>,
}

impl StaticPlanner {
    pub fn new(steps: Vec<impl Into<String>>) -> Self {
        Self {
            steps: steps.into_iter().map(|s| s.into()).collect(),
        }
    }
}

impl Planner for StaticPlanner {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            let steps = self.steps.iter().map(|s| PlanStep::new(s)).collect();
            Ok(Plan::new(steps).with_goal(task))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_steps_json() {
        let response = r#"```json
[{"description": "步骤一"}, {"description": "步骤二"}]
```"#;
        let steps = LlmPlanner::parse_steps(response);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].description, "步骤一");
        assert_eq!(steps[1].description, "步骤二");
    }

    #[test]
    fn test_parse_steps_plain_json() {
        let response = r#"[{"description": "分析代码"}, {"description": "优化性能"}]"#;
        let steps = LlmPlanner::parse_steps(response);
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn test_parse_steps_fallback() {
        let response = "1. 第一步\n2. 第二步\n3. 第三步";
        let steps = LlmPlanner::parse_steps(response);
        assert_eq!(steps.len(), 3);
    }

    #[tokio::test]
    async fn test_static_planner() {
        let planner = StaticPlanner::new(vec!["步骤A", "步骤B", "步骤C"]);
        let plan = planner.plan("测试任务").await.unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "步骤A");
        assert_eq!(plan.goal.as_deref(), Some("测试任务"));
    }
}
