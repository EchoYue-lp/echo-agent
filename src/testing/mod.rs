//! 测试基础设施
//!
//! 提供在不依赖真实 LLM / 外部服务的情况下测试 echo-agent 各组件的工具集。
//!
//! | 类型 | 用途 |
//! |------|------|
//! | [`MockLlmClient`] | 替代真实 LLM，用于测试 `SummaryCompressor` 等内部依赖 `LlmClient` 的组件 |
//! | [`MockTool`] | 替代真实工具，用于测试 Agent 的工具调用 / 错误处理行为 |
//! | [`MockAgent`] | 替代真实 SubAgent，用于测试多 Agent 编排逻辑 |
//! | [`FailingMockAgent`] | 总是返回错误，用于测试编排的容错路径 |
//!
//! # 设计原则
//!
//! - **零网络请求**：所有 Mock 都完全在内存中运行
//! - **可脚本化**：通过 `with_response()` / `with_error()` 精确控制返回值
//! - **可观测**：通过 `call_count()` / `last_args()` 等方法检查调用情况
//! - **线程安全**：内部使用 `Arc<Mutex<_>>`，可安全地在多任务测试中共享
//!
//! # 使用示例
//!
//! ## 测试压缩器（`MockLlmClient`）
//!
//! ```rust,no_run
//! use echo_agent::testing::MockLlmClient;
//! use echo_agent::compression::{ContextManager, CompressionInput};
//! use echo_agent::compression::compressor::{SummaryCompressor, DefaultSummaryPrompt};
//! use echo_agent::llm::types::Message;
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mock_llm = Arc::new(
//!     MockLlmClient::new().with_response("这是 LLM 生成的摘要内容")
//! );
//!
//! let compressor = SummaryCompressor::new(mock_llm.clone(), DefaultSummaryPrompt, 2);
//! let input = CompressionInput {
//!     messages: vec![
//!         Message::user("消息1".to_string()),
//!         Message::assistant("回复1".to_string()),
//!         Message::user("消息2".to_string()),
//!         Message::assistant("回复2".to_string()),
//!         Message::user("消息3".to_string()),
//!     ],
//!     token_limit: 100,
//!     current_query: None,
//! };
//!
//! let output = compressor.compress(input).await?;
//! assert!(!output.messages.is_empty());
//! assert_eq!(mock_llm.call_count(), 1);  // 确认 LLM 被调用了一次
//! # Ok(())
//! # }
//! ```
//!
//! ## 测试工具行为（`MockTool`）
//!
//! ```rust
//! use echo_agent::testing::MockTool;
//! use echo_agent::tools::Tool;
//! use std::collections::HashMap;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let tool = MockTool::new("weather")
//!     .with_response(r#"{"city":"Beijing","temp":25}"#)
//!     .with_failure("API 服务不可用");
//!
//! let r1 = tool.execute(HashMap::new()).await.unwrap();
//! assert!(r1.success);
//!
//! let r2 = tool.execute(HashMap::new()).await.unwrap();
//! assert!(!r2.success);
//!
//! assert_eq!(tool.call_count(), 2);
//! # }
//! ```
//!
//! ## 测试多 Agent 编排（`MockAgent`）
//!
//! ```rust
//! use echo_agent::testing::MockAgent;
//! use echo_agent::agent::Agent;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let mut agent = MockAgent::new("math_agent")
//!     .with_response("结果是 42");
//!
//! let answer = agent.execute("6 * 7 = ?").await.unwrap();
//! assert_eq!(answer, "结果是 42");
//! assert_eq!(agent.calls()[0], "6 * 7 = ?");
//! # }
//! ```

mod mock_agent;
mod mock_llm;
mod mock_tool;

pub use mock_agent::{FailingMockAgent, MockAgent};
pub use mock_llm::MockLlmClient;
pub use mock_tool::MockTool;
