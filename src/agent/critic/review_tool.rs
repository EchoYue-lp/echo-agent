//! ReviewTool — 让 Agent 自我审查的工具
//!
//! 遵循业界模式（Hermes/Claude Code）：反思是工具级能力，不是独立 Agent 类型。
//! Agent 可以在 ReAct 循环中调用此工具来评估自己的输出质量。

use crate::agent::critic::{Critic, LlmCritic};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// ReviewTool — 评估 Agent 输出质量的工具
///
/// Agent 可以在推理过程中调用此工具来：
/// 1. 评估自己的回答质量
/// 2. 获取结构化反馈（分数、建议）
/// 3. 根据反馈决定是否继续优化
///
/// # Example
///
/// ```rust,ignore
/// use echo_agent::agent::critic::{LlmCritic, ReviewTool};
///
/// let critic = LlmCritic::new(llm_client).with_pass_threshold(8.0);
/// let review_tool = ReviewTool::new(Arc::new(critic));
///
/// // Agent 在 ReAct 循环中可以调用：
/// // review(task="写一个排序算法", output="我的排序代码...")
/// // 返回：{"score": 8.5, "passed": true, "feedback": "...", "suggestions": []}
/// ```
pub struct ReviewTool {
    critic: Arc<dyn Critic>,
}

impl ReviewTool {
    /// 创建 ReviewTool，使用提供的 Critic 进行评估
    pub fn new(critic: Arc<dyn Critic>) -> Self {
        Self { critic }
    }

    /// 便捷方法：使用 LlmCritic 创建 ReviewTool
    pub fn with_llm(client: Arc<dyn crate::llm::LlmClient>) -> Self {
        Self::new(Arc::new(LlmCritic::new(client)))
    }

    /// 便捷方法：使用 LlmCritic 并指定 pass_threshold
    pub fn with_llm_threshold(client: Arc<dyn crate::llm::LlmClient>, threshold: f64) -> Self {
        let critic = LlmCritic::new(client).with_pass_threshold(threshold);
        Self::new(Arc::new(critic))
    }
}

impl Tool for ReviewTool {
    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        "Evaluate the quality of your own output. Use this tool when you want to self-critique \
         before finalizing your answer. Returns a structured critique with score, pass/fail status, \
         and actionable feedback."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The original task or question"
                },
                "output": {
                    "type": "string",
                    "description": "Your output/answer to be reviewed"
                },
                "context": {
                    "type": "string",
                    "description": "Additional context (optional)"
                }
            },
            "required": ["task", "output"]
        })
    }

    fn execute<'a>(
        &'a self,
        parameters: HashMap<String, Value>,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let task = parameters
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task".to_string()))?;

            let output = parameters
                .get("output")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("output".to_string()))?;

            let context = parameters
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // 调用 Critic 进行评估
            let critique = self.critic.critique(task, output, context).await?;

            // 构建结构化响应
            let result = json!({
                "score": critique.score,
                "passed": critique.passed,
                "feedback": critique.feedback,
                "suggestions": critique.suggestions
            });

            Ok(ToolResult::success(result.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::critic::StaticCritic;

    #[tokio::test]
    async fn test_review_tool_with_static_critic() {
        // 使用 StaticCritic 进行测试（总是返回固定分数）
        let critic = StaticCritic::new(8.5, true, "Good response");
        let tool = ReviewTool::new(Arc::new(critic));

        let mut args = HashMap::new();
        args.insert("task".to_string(), json!("写一个快速排序"));
        args.insert("output".to_string(), json!("fn quicksort..."));

        let result = tool.execute(args).await.unwrap();
        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed["score"], 8.5);
        assert_eq!(parsed["passed"], true);
    }

    #[tokio::test]
    async fn test_review_tool_missing_params() {
        let critic = StaticCritic::new(5.0, false, "Needs improvement");
        let tool = ReviewTool::new(Arc::new(critic));

        // 缺少 output 参数
        let mut args = HashMap::new();
        args.insert("task".to_string(), json!("some task"));

        let result = tool.execute(args).await;
        assert!(result.is_err());
    }
}
