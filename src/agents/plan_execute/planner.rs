//! Planner — 负责将用户任务分解为结构化执行计划

use super::types::Plan;
use super::types::{PlanOutput, PlanStep, PlanStepOutput, plan_output_schema};
use crate::error::Result;
use crate::llm::types::Message;
use crate::llm::{self, LlmConfig, ResponseFormat};
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Planner trait — 接收任务描述，返回执行计划
pub trait Planner: Send + Sync {
    /// 根据任务描述生成执行计划
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// ── LlmPlanner ───────────────────────────────────────────────────────────────

/// 规划输出模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlannerOutputMode {
    /// 使用 JSON Schema 强制结构化输出（推荐，可靠性最高）
    #[default]
    JsonSchema,
    /// 使用 JsonObject + 自由文本解析（兼容性更好）
    JsonText,
}

/// 基于 LLM 的 Planner：调用大模型来分解任务
pub struct LlmPlanner {
    model: String,
    client: Arc<Client>,
    llm_config: Option<LlmConfig>,
    system_prompt: String,
    output_mode: PlannerOutputMode,
}

impl LlmPlanner {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            client: Arc::new(Client::new()),
            llm_config: None,
            system_prompt: Self::default_system_prompt().to_string(),
            output_mode: PlannerOutputMode::JsonSchema,
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

    /// 设置输出模式
    pub fn with_output_mode(mut self, mode: PlannerOutputMode) -> Self {
        self.output_mode = mode;
        self
    }

    fn default_system_prompt() -> &'static str {
        "你是一个任务规划专家。给定一个任务，你需要将其分解为具体可执行的步骤。\n\n\
        规则：\n\
        1. 每个步骤必须是明确、可执行的\n\
        2. 步骤之间要有逻辑顺序\n\
        3. 每个步骤应该只做一件事\n\
        4. 步骤描述应该简洁但充分\n\
        5. 如果某个步骤依赖另一个步骤的结果，在 dependencies 中填写被依赖步骤的描述关键词\n\
        6. 互相独立的步骤不要设置依赖关系\n\n\
        请严格按 JSON Schema 返回结构化数据。"
    }

    /// 尝试将 LLM 原始响应解析为 PlanOutput
    fn parse_structured_output(content: &str) -> Result<PlanOutput> {
        // 1. 直接解析
        if let Ok(output) = serde_json::from_str::<PlanOutput>(content) {
            return Ok(output);
        }

        // 2. 从 markdown code block 提取
        let json_str = crate::utils::json_parse::extract_json_from_markdown(content);
        if let Ok(output) = serde_json::from_str::<PlanOutput>(&json_str) {
            return Ok(output);
        }

        // 3. 尝试修复常见问题
        Self::try_auto_fix(&json_str)
    }

    /// 尝试自动修复常见 JSON 问题
    fn try_auto_fix(json_str: &str) -> Result<PlanOutput> {
        let trimmed = json_str.trim();

        // 修复：裸数组 → 包装为 {"steps": [...]}
        if trimmed.starts_with('[') {
            let wrapped = format!("{{\"steps\": {}}}", trimmed);
            let fixed = crate::utils::json_parse::clean_json(&wrapped);
            if let Ok(output) = serde_json::from_str::<PlanOutput>(&fixed) {
                info!("Auto-fix: wrapped bare array into PlanOutput");
                return Ok(output);
            }
        }

        // 修复：尾部逗号、引号等
        let fixed = crate::utils::json_parse::clean_json(trimmed);

        match serde_json::from_str::<PlanOutput>(&fixed) {
            Ok(output) => {
                info!("Auto-fix succeeded for malformed LLM plan output");
                Ok(output)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse plan output even after auto-fix");
                // 最后兜底：构造单步计划
                Ok(PlanOutput {
                    steps: vec![PlanStepOutput {
                        description: trimmed.to_string(),
                        dependencies: vec![],
                        expected_output: None,
                    }],
                })
            }
        }
    }

    /// 解析旧格式（兼容 JsonText 模式）
    fn parse_steps_legacy(response: &str) -> Vec<PlanStep> {
        let json_str = crate::utils::json_parse::extract_json_from_markdown(response);

        #[derive(serde::Deserialize)]
        struct StepJson {
            description: String,
        }

        if let Ok(steps) = serde_json::from_str::<Vec<StepJson>>(&json_str) {
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
            .filter(|line| !line.starts_with("```") && !line.starts_with('#'))
            .map(|line| {
                let cleaned = line.trim_start_matches(|c: char| {
                    c.is_ascii_digit() || c == '.' || c == '-' || c == ' '
                });
                PlanStep::new(if cleaned.is_empty() { line } else { cleaned })
            })
            .collect()
    }

    /// 将 PlanOutput 转换为 Plan，解析依赖关系
    fn resolve_plan_output(output: PlanOutput, goal: &str) -> Plan {
        // 构建描述→索引的映射（用于模糊匹配依赖）
        let desc_to_idx: Vec<(String, usize)> = output
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.description.clone(), i))
            .collect();

        let steps: Vec<PlanStep> = output
            .steps
            .into_iter()
            .map(|step_output| {
                // 解析依赖：尝试模糊匹配到步骤索引
                let deps: Vec<String> = step_output
                    .dependencies
                    .iter()
                    .filter_map(|dep_desc| {
                        // 精确匹配优先
                        let exact = desc_to_idx.iter().find(|(d, _)| d == dep_desc);
                        if let Some((_, idx)) = exact {
                            return Some(format!("step_{}", idx));
                        }
                        // 模糊匹配：包含关键词
                        let fuzzy = desc_to_idx
                            .iter()
                            .find(|(d, _)| d.contains(dep_desc) || dep_desc.contains(d));
                        fuzzy.map(|(_, idx)| format!("step_{}", idx))
                    })
                    .collect();

                let mut step = PlanStep::new(step_output.description);
                if !deps.is_empty() {
                    step = step.with_dependencies(deps);
                }
                if let Some(eo) = step_output.expected_output {
                    step = step.with_expected_output(eo);
                }
                step
            })
            .collect();

        Plan::new(steps).with_goal(goal)
    }
}

impl Planner for LlmPlanner {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            info!(model = %self.model, mode = ?self.output_mode, "LlmPlanner: generating plan");

            let messages = vec![
                Message::system(self.system_prompt.clone()),
                Message::user(format!("请为以下任务制定执行计划：\n\n{}", task)),
            ];

            let response_format = match self.output_mode {
                PlannerOutputMode::JsonSchema => Some(ResponseFormat::json_schema(
                    "plan_output",
                    plan_output_schema(),
                )),
                PlannerOutputMode::JsonText => None,
            };

            let response = llm::chat(
                self.client.clone(),
                &self.model,
                &messages,
                Some(0.3),
                Some(4096u32),
                Some(false),
                None,
                None,
                response_format,
            )
            .await?;

            let content = response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default();

            debug!(response = %content, "LlmPlanner raw response");

            let plan = match self.output_mode {
                PlannerOutputMode::JsonSchema => {
                    let output = Self::parse_structured_output(&content)?;
                    Self::resolve_plan_output(output, task)
                }
                PlannerOutputMode::JsonText => {
                    let steps = Self::parse_steps_legacy(&content);
                    if steps.is_empty() {
                        Plan::new(vec![PlanStep::new(task)]).with_goal(task)
                    } else {
                        Plan::new(steps).with_goal(task)
                    }
                }
            };

            info!(
                steps = plan.steps.len(),
                "Plan generated with {} steps",
                plan.steps.len()
            );

            Ok(plan)
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
            let steps = self.steps.iter().map(PlanStep::new).collect();
            Ok(Plan::new(steps).with_goal(task))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_structured_output_json() {
        let response = r#"{"steps":[{"description":"分析代码结构","dependencies":[],"expected_output":"代码结构报告"},{"description":"识别性能瓶颈","dependencies":["分析代码结构"]}]}"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
        assert_eq!(output.steps[0].description, "分析代码结构");
        assert_eq!(output.steps[1].dependencies, vec!["分析代码结构"]);
    }

    #[test]
    fn test_parse_structured_output_markdown() {
        let response = r#"```json
{"steps":[{"description":"步骤一"},{"description":"步骤二"}]}
```"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
    }

    #[test]
    fn test_auto_fix_array_wrapping() {
        let response = r#"[{"description":"步骤A"},{"description":"步骤B"}]"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
    }

    #[test]
    fn test_auto_fix_trailing_comma() {
        let response = r#"{"steps":[{"description":"步骤A",}]}"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 1);
    }

    #[test]
    fn test_auto_fix_fallback() {
        let response = "无法解析的文本";
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 1); // 兜底为单步计划
    }

    #[test]
    fn test_resolve_plan_output_with_deps() {
        let output = PlanOutput {
            steps: vec![
                PlanStepOutput {
                    description: "分析代码".into(),
                    dependencies: vec![],
                    expected_output: None,
                },
                PlanStepOutput {
                    description: "优化性能".into(),
                    dependencies: vec!["分析代码".into()],
                    expected_output: Some("优化报告".into()),
                },
            ],
        };
        let plan = LlmPlanner::resolve_plan_output(output, "test");
        assert_eq!(plan.steps.len(), 2);
        assert!(plan.steps[0].dependencies.is_empty());
        assert_eq!(plan.steps[1].dependencies, vec!["step_0"]);
        assert_eq!(plan.steps[1].expected_output, Some("优化报告".to_string()));
    }

    #[test]
    fn test_parse_steps_legacy_json() {
        let response = r#"```json
[{"description": "步骤一"}, {"description": "步骤二"}]
```"#;
        let steps = LlmPlanner::parse_steps_legacy(response);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].description, "步骤一");
        assert_eq!(steps[1].description, "步骤二");
    }

    #[test]
    fn test_parse_steps_legacy_plain_json() {
        let response = r#"[{"description": "分析代码"}, {"description": "优化性能"}]"#;
        let steps = LlmPlanner::parse_steps_legacy(response);
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn test_parse_steps_legacy_fallback() {
        let response = "1. 第一步\n2. 第二步\n3. 第三步";
        let steps = LlmPlanner::parse_steps_legacy(response);
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

    #[test]
    fn test_plan_output_schema_valid() {
        let schema = plan_output_schema();
        assert!(schema.is_object());
        assert!(schema["properties"]["steps"].is_object());
    }
}
