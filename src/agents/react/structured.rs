//! Structured Output 支持
//!
//! 提供 `StructuredAgent<T>` 包装器，在 Agent execute 后自动将文本输出解析为类型 `T`。
//!
//! # 使用方式
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::agents::react::structured::StructuredAgent;
//! use serde::Deserialize;
//!
//! #[derive(Debug, Deserialize)]
//! struct Sentiment { label: String, score: f64 }
//!
//! # async fn run() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::simple("qwen3-max", "你是情感分析助手，始终以 JSON 格式输出")?;
//! let mut sa = StructuredAgent::<Sentiment>::new(agent);
//! let result = sa.execute("I love Rust!").await?;
//! println!("{}: {}", result.label, result.score);
//! # Ok(())
//! # }
//! ```

use crate::agent::Agent;
use crate::error::{ReactError, Result};

use super::ReactAgent;

/// 将 Agent 的文本输出自动解析为类型 `T` 的包装器
///
/// 要求 Agent 输出合法 JSON 文本（可以通过 system prompt 约束，
/// 或配合 `ResponseFormat::JsonSchema` 使用）。
pub struct StructuredAgent<T> {
    inner: ReactAgent,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> StructuredAgent<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    /// 包装一个已有的 ReactAgent
    pub fn new(agent: ReactAgent) -> Self {
        Self {
            inner: agent,
            _phantom: std::marker::PhantomData,
        }
    }

    /// 获取内部 Agent 的引用
    pub fn inner(&self) -> &ReactAgent {
        &self.inner
    }

    /// 获取内部 Agent 的可变引用
    pub fn inner_mut(&mut self) -> &mut ReactAgent {
        &mut self.inner
    }

    /// 执行任务并将结果解析为类型 `T`
    pub async fn execute(&mut self, task: &str) -> Result<T> {
        let text = self.inner.execute(task).await?;
        parse_json_output(&text)
    }

    /// 聊天并将结果解析为类型 `T`
    pub async fn chat(&mut self, message: &str) -> Result<T> {
        let text = self.inner.chat(message).await?;
        parse_json_output(&text)
    }
}

/// 从 LLM 输出文本中提取并解析 JSON
///
/// 支持两种格式：
/// 1. 纯 JSON 字符串
/// 2. Markdown 代码块包裹的 JSON（```json ... ```）
fn parse_json_output<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    let trimmed = text.trim();

    // 尝试直接解析
    if let Ok(v) = serde_json::from_str::<T>(trimmed) {
        return Ok(v);
    }

    // 尝试提取 markdown 代码块中的 JSON
    if let Some(json_str) = extract_json_from_markdown(trimmed) {
        if let Ok(v) = serde_json::from_str::<T>(json_str) {
            return Ok(v);
        }
    }

    Err(ReactError::Other(format!(
        "无法将 LLM 输出解析为目标类型。原始输出:\n{text}"
    )))
}

/// 从 markdown 代码块中提取 JSON 内容
fn extract_json_from_markdown(text: &str) -> Option<&str> {
    // 匹配 ```json\n...\n``` 或 ```\n...\n```
    let start = if let Some(pos) = text.find("```json") {
        pos + 7
    } else if let Some(pos) = text.find("```") {
        pos + 3
    } else {
        return None;
    };

    let remaining = &text[start..];
    // 跳过换行符
    let content_start = remaining.find('\n').map(|p| p + 1).unwrap_or(0);
    let content = &remaining[content_start..];

    // 找到结束的 ```
    content.find("```").map(|end| content[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    #[test]
    fn test_parse_json_direct() {
        let result: Person = parse_json_output(r#"{"name": "Alice", "age": 30}"#).unwrap();
        assert_eq!(
            result,
            Person {
                name: "Alice".to_string(),
                age: 30
            }
        );
    }

    #[test]
    fn test_parse_json_with_whitespace() {
        let result: Person = parse_json_output("  \n{\"name\": \"Bob\", \"age\": 25}\n  ").unwrap();
        assert_eq!(result.name, "Bob");
    }

    #[test]
    fn test_parse_json_from_markdown() {
        let text = r#"Here is the result:
```json
{"name": "Charlie", "age": 35}
```
"#;
        let result: Person = parse_json_output(text).unwrap();
        assert_eq!(result.name, "Charlie");
        assert_eq!(result.age, 35);
    }

    #[test]
    fn test_parse_json_failure() {
        let result = parse_json_output::<Person>("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let text = "```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_from_markdown(text), Some("{\"a\": 1}"));
    }

    #[test]
    fn test_extract_json_no_markdown() {
        assert_eq!(extract_json_from_markdown("{\"a\": 1}"), None);
    }
}
