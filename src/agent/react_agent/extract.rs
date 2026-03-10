//! ReactAgent 结构化提取
//!
//! 提供一次性 JSON 提取方法，不经过 ReAct 循环。

use super::ReactAgent;
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
        let messages = vec![
            Message::system(self.config.system_prompt.clone()),
            Message::user(prompt.to_string()),
        ];

        let response = chat(
            self.client.clone(),
            &self.config.model_name,
            messages,
            Some(0.0),
            Some(4096),
            Some(false),
            None,
            None,
            Some(schema),
        )
        .await?;

        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| ReactError::Other("LLM 返回空内容".to_string()))?;

        serde_json::from_str(&text)
            .map_err(|e| ReactError::Other(format!("JSON 解析失败: {e}\n原始响应: {text}")))
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
}
