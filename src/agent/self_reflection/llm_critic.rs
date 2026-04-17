//! LLM 驱动的 Critic 实现

use crate::agent::self_reflection::critic::Critic;
use crate::agent::self_reflection::types::{Critique, CritiqueOutput, critique_output_schema};
use crate::error::{ReactError, Result};
use crate::llm;
use crate::llm::ResponseFormat;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// LLM 驱动的评估器
///
/// 使用大模型评估 Agent 输出的质量，返回结构化的 `Critique`。
/// 复用 `LlmPlanner` 的模式：LLM 调用 + 结构化 JSON 输出 + 自动修复。
pub struct LlmCritic {
    model: String,
    client: Arc<Client>,
    system_prompt: String,
    pass_threshold: f64,
}

impl LlmCritic {
    /// 创建 LLM 评估器
    ///
    /// # 参数
    /// * `model` - LLM 模型标识符，用于质量评估
    ///
    /// # 默认配置
    /// * 系统提示：多维度质量评估专家（准确性、完整性、清晰度、实用性）
    /// * 通过阈值：7.0（评分 ≥ 7.0 视为通过）
    /// * HTTP 客户端：新建的 `reqwest::Client`
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            client: Arc::new(Client::new()),
            system_prompt: Self::default_system_prompt().to_string(),
            pass_threshold: 7.0,
        }
    }

    /// 自定义系统提示词
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// 设置通过阈值（0.0 - 10.0）
    pub fn with_pass_threshold(mut self, threshold: f64) -> Self {
        self.pass_threshold = threshold;
        self
    }

    fn default_system_prompt() -> &'static str {
        "你是一个严格的质量评估专家。你需要评估给定回答的质量。\n\n\
        评估维度：\n\
        1. 准确性：事实是否正确\n\
        2. 完整性：是否覆盖了所有要点\n\
        3. 清晰度：表述是否清楚易懂\n\
        4. 实用性：是否提供了有价值的信息\n\n\
        评分标准：\n\
        - 9.0-10.0: 优秀，几乎无可挑剔\n\
        - 7.0-8.9: 良好，基本正确但有小瑕疵\n\
        - 5.0-6.9: 一般，有明显不足\n\
        - 0.0-4.9: 较差，存在严重错误\n\n\
        请严格按 JSON Schema 返回结构化数据。"
    }

    /// 解析 LLM 响应为 CritiqueOutput
    fn parse_critique_output(content: &str) -> Result<CritiqueOutput> {
        // 1. 直接解析
        if let Ok(output) = serde_json::from_str::<CritiqueOutput>(content) {
            return Ok(output);
        }

        // 2. 从 markdown code block 提取
        let json_str = crate::utils::json_parse::extract_json_from_markdown(content);
        if let Ok(output) = serde_json::from_str::<CritiqueOutput>(&json_str) {
            return Ok(output);
        }

        // 3. 自动修复
        Self::try_auto_fix(&json_str)
    }

    fn try_auto_fix(json_str: &str) -> Result<CritiqueOutput> {
        let fixed = crate::utils::json_parse::clean_json(json_str);

        match serde_json::from_str::<CritiqueOutput>(&fixed) {
            Ok(output) => {
                info!("Auto-fix succeeded for LLM critique output");
                Ok(output)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse critique output");
                // 兜底：构造默认的未通过评估
                Ok(CritiqueOutput {
                    score: 5.0,
                    passed: false,
                    feedback: json_str.trim().to_string(),
                    suggestions: vec![],
                })
            }
        }
    }
}

impl Critic for LlmCritic {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            info!(model = %self.model, "LlmCritic: evaluating answer");

            let user_content = if context.is_empty() {
                format!("原始任务:\n{}\n\n待评估的回答:\n{}", task, answer)
            } else {
                format!(
                    "原始任务:\n{}\n\n待评估的回答:\n{}\n\n附加上下文:\n{}",
                    task, answer, context
                )
            };

            let messages = vec![
                Message::system(self.system_prompt.clone()),
                Message::user(user_content),
            ];

            let response_format = Some(ResponseFormat::json_schema(
                "critique_output",
                critique_output_schema(),
            ));

            let response = llm::chat(
                self.client.clone(),
                &self.model,
                &messages,
                Some(0.3),
                Some(2048u32),
                Some(false),
                None,
                None,
                response_format,
            )
            .await
            .map_err(|e| ReactError::Other(format!("LLM critique call failed: {}", e)))?;

            let content = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_text())
                .unwrap_or_default();

            debug!(response = %content, "LlmCritic raw response");

            let output = Self::parse_critique_output(&content)?;
            let mut critique: Critique = output.into();

            // 用阈值覆盖 LLM 的 passed 判定
            critique.passed = critique.score >= self.pass_threshold;

            info!(
                score = critique.score,
                passed = critique.passed,
                "LlmCritic: evaluation complete"
            );

            Ok(critique)
        })
    }

    fn name(&self) -> &str {
        "llm_critic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_critique_output_json() {
        let json = r#"{"score": 8.5, "passed": true, "feedback": "回答准确", "suggestions": ["可以更详细"]}"#;
        let output = LlmCritic::parse_critique_output(json).unwrap();
        assert_eq!(output.score, 8.5);
        assert!(output.passed);
    }

    #[test]
    fn test_parse_critique_output_markdown() {
        let response = r#"```json
{"score": 6.0, "passed": false, "feedback": "不够完整", "suggestions": ["增加示例"]}
```"#;
        let output = LlmCritic::parse_critique_output(response).unwrap();
        assert_eq!(output.score, 6.0);
        assert!(!output.passed);
    }

    #[test]
    fn test_parse_critique_auto_fix() {
        let json = r#"{"score": 7.0, "passed": true, "feedback": "良好",}"#;
        let output = LlmCritic::parse_critique_output(json).unwrap();
        assert_eq!(output.score, 7.0);
    }

    #[test]
    fn test_parse_critique_fallback() {
        let text = "无法解析的文本";
        let output = LlmCritic::parse_critique_output(text).unwrap();
        assert!(!output.passed); // 兜底：未通过
        assert_eq!(output.score, 5.0);
    }
}
