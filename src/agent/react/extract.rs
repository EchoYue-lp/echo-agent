//! ReactAgent 结构化提取
//!
//! 提供两类结构化输出能力：
//!
//! - **一次性提取** (`extract_json` / `extract`)：不走 ReAct 循环，直接向 LLM 提取
//! - **完整执行提取** (`execute_typed`)：走完整 ReAct 循环，需配合 `output_type` 使用

use super::ReactAgent;
use crate::agent::Agent;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use crate::llm::{ResponseFormat, chat};

impl ReactAgent {
    /// 一次性结构化 JSON 提取，不走 ReAct 循环。
    ///
    /// 直接向 LLM 发一次请求，要求按 `schema` 返回 JSON，
    /// 返回解析后的 [`serde_json::Value`]。
    ///
    /// 适合"提取 / 分类 / 格式转换"等不需要工具调用的场景。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// use echo_agent::prelude::*;
    /// use serde_json::json;
    ///
    /// # let config = AgentConfig::new("qwen3-max", "extractor", "你是一个信息提取助手");
    /// # let agent = ReactAgent::new(config);
    /// let result = agent.extract_json(
    ///     "张三，28岁",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("{}", result["name"]);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_json(
        &self,
        prompt: &str,
        schema: ResponseFormat,
    ) -> Result<serde_json::Value> {
        let mut messages = vec![
            Message::system(self.config.system_prompt.clone()),
            Message::user(prompt.to_string()),
        ];

        let max_retries = self.config.llm_max_retries;
        let retry_delay = std::time::Duration::from_millis(self.config.llm_retry_delay_ms);

        for attempt in 0..=max_retries {
            let response = chat(
                self.client.clone(),
                &self.config.model_name,
                &messages,
                Some(0.0),
                Some(4096),
                Some(false),
                None,
                None,
                Some(schema.clone()),
            )
            .await?;

            let text = response
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content.as_text())
                .ok_or_else(|| ReactError::Other("LLM 返回空内容".to_string()))?;

            match serde_json::from_str(&text) {
                Ok(value) => return Ok(value),
                Err(e) if attempt < max_retries => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        error = %e,
                        "JSON 解析失败，将错误反馈给 LLM 重试"
                    );
                    // Feed the error back to LLM for self-correction
                    let correction = format!(
                        "Your previous response was not valid JSON.\n\
                         Parse error: {e}\n\
                         Raw response:\n{text}\n\n\
                         Please provide a valid JSON response that strictly matches the required schema."
                    );
                    messages.push(Message::assistant(text));
                    messages.push(Message::user(correction));
                    tokio::time::sleep(retry_delay).await;
                }
                Err(e) => {
                    return Err(ReactError::Other(format!(
                        "JSON 解析失败（已重试 {max_retries} 次）: {e}\n原始响应: {text}"
                    )));
                }
            }
        }

        unreachable!()
    }

    /// 一次性结构化提取，自动将 JSON 结果反序列化为指定类型 `T`。
    ///
    /// 与 [`extract_json`](Self::extract_json) 相同，但额外执行 `serde` 反序列化。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::llm::ResponseFormat;
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Person { name: String, age: u32 }
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// # use echo_agent::prelude::*;
    /// # let config = AgentConfig::new("qwen3-max", "extractor", "你是一个提取助手");
    /// # let agent = ReactAgent::new(config);
    /// let person: Person = agent.extract(
    ///     "张三，28岁",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("姓名: {}, 年龄: {}", person.name, person.age);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract<T>(&self, prompt: &str, schema: ResponseFormat) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self.extract_json(prompt, schema).await?;
        serde_json::from_value(value).map_err(|e| ReactError::Other(format!("反序列化失败: {e}")))
    }

    /// 完整 ReAct 执行后将结果反序列化为类型 `T`
    ///
    /// 需要在构建 Agent 时通过 [`ReactAgentBuilder::output_type`] 声明输出类型，
    /// 框架会自动设置 `response_format` 引导 LLM 返回匹配的 JSON。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, JsonSchema)]
    /// struct Analysis { summary: String, score: f64 }
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// let mut agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .system_prompt("你是一个分析助手，请以 JSON 格式返回分析结果")
    ///     .output_type::<Analysis>()
    ///     .build()?;
    ///
    /// let result: Analysis = agent.execute_typed("分析 Rust 语言的优缺点").await?;
    /// println!("评分: {}", result.score);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_typed<T>(&mut self, task: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let raw = self.execute(task).await?;
        serde_json::from_str(&raw)
            .map_err(|e| ReactError::Other(format!("结构化输出反序列化失败: {e}\n原始响应: {raw}")))
    }
}
