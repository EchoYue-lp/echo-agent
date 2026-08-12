//! Testing infrastructure
//!
//! Provides a set of utilities for testing echo-agent components without
//! depending on real LLMs / external services.
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`MockLlmClient`] | Replaces a real LLM, used for testing `SummaryCompressor` and other components that depend on `LlmClient` |
//! | [`MockTool`] | Replaces a real tool, used for testing Agent tool call / error handling behavior |
//! | [`MockAgent`] | Replaces a real SubAgent, used for testing multi-Agent orchestration logic |
//! | [`FailingMockAgent`] | Always returns an error, used for testing orchestration fault-tolerance paths |
//!
//! # Design Principles
//!
//! - **Zero network requests**: All mocks run entirely in memory
//! - **Scriptable**: Precisely control return values via `with_response()` / `with_error()`
//! - **Observable**: Inspect invocations via `call_count()` / `last_args()` and similar methods
//! - **Thread-safe**: Uses `Arc<Mutex<_>>` internally, safe to share across multi-task tests
//!
//! # Usage Examples
//!
//! ## Testing a compressor (`MockLlmClient`)
//!
//! ```rust,no_run
//! use echo_agent::testing::MockLlmClient;
//! use echo_agent::compression::{ContextManager, CompressionInput, ContextCompressor};
//! use echo_agent::compression::compressor::SummaryCompressor;
//! use echo_agent::llm::types::Message;
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mock_llm = Arc::new(
//!     MockLlmClient::new().with_response("This is LLM-generated summary content")
//! );
//!
//! let compressor = SummaryCompressor::new(mock_llm.clone(), 2);
//! let input = CompressionInput {
//!     messages: vec![
//!         Message::user("message1".to_string()),
//!         Message::assistant("reply1".to_string()),
//!         Message::user("message2".to_string()),
//!         Message::assistant("reply2".to_string()),
//!         Message::user("message3".to_string()),
//!     ],
//!     token_limit: 100,
//!     current_query: None,
//!     focus_instructions: None,
//! };
//!
//! let output = compressor.compress(input).await?;
//! assert!(!output.messages.is_empty());
//! assert_eq!(mock_llm.call_count(), 1);  // Confirm LLM was called once
//! # Ok(())
//! # }
//! ```
//!
//! ## Testing tool behavior (`MockTool`)
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
//!     .with_failure("API service unavailable");
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
//! ## Testing multi-Agent orchestration (`MockAgent`)
//!
//! ```rust
//! use echo_agent::testing::MockAgent;
//! use echo_agent::agent::Agent;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let mut agent = MockAgent::new("math_agent")
//!     .with_response("The result is 42");
//!
//! let answer = agent.execute("6 * 7 = ?").await.unwrap();
//! assert_eq!(answer, "The result is 42");
//! assert_eq!(agent.calls()[0], "6 * 7 = ?");
//! # }
//! ```

mod mock_agent;
mod mock_embedder;
mod mock_llm;
mod mock_tool;

pub use mock_agent::{FailingMockAgent, MockAgent};
pub use mock_embedder::MockEmbedder;
pub use mock_llm::{MockLlmClient, StreamChunk};
pub use mock_tool::MockTool;
