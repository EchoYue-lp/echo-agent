//! Adapter wrapping `Arc<dyn Agent>` as an `impl Agent` (Sprint 11).
//!
//! `TeamAgentBuilder::manager`/`subagent` consume `Box<dyn Agent>`, but
//! `SubagentRegistry::get_agent` returns `Arc<dyn Agent>` (a shared singleton
//! that may be used by multiple dispatch paths). The `Agent` trait is not
//! `Clone`, so a raw `Box::new(arc)` won't typecheck. This newtype transparently
//! delegates the 4 required trait methods to the inner `Arc`, letting a shared
//! agent be fed into the team builder.
//!
//! Only the 4 required `Agent` methods are implemented; the rest fall back to
//! their trait defaults (which is fine — the team only calls `execute`).

use echo_core::agent::Agent;
use echo_core::error::Result;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Arc;

/// Wrap a shared `Arc<dyn Agent>` so it can be boxed into APIs that consume
/// `Box<dyn Agent>` (notably `TeamAgentBuilder`).
pub struct ArcAgentBox(pub Arc<dyn Agent>);

impl Agent for ArcAgentBox {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn model_name(&self) -> &str {
        self.0.model_name()
    }
    fn system_prompt(&self) -> &str {
        self.0.system_prompt()
    }
    fn token_usage_summary(&self) -> echo_core::tokenizer::UsageSummary {
        self.0.token_usage_summary()
    }
    fn steer_input(
        &self,
        expected_turn_id: Option<&str>,
        message: echo_core::llm::types::Message,
    ) -> std::result::Result<String, echo_core::agent::AgentSteerError> {
        self.0.steer_input(expected_turn_id, message)
    }
    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        self.0.execute(task)
    }
    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<echo_core::agent::AgentEvent>>>> {
        self.0.execute_stream(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::error::ReactError;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A minimal stub agent that counts execute() calls.
    struct StubAgent {
        name: String,
        call_count: Arc<AtomicU32>,
    }
    impl Agent for StubAgent {
        fn name(&self) -> &str {
            &self.name
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
            let c = self.call_count.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok("stub-output".to_string())
            })
        }
        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<echo_core::agent::AgentEvent>>>> {
            // Minimal: wrap execute() into a single FinalAnswer event stream
            // (mirrors MockAgent's pattern in src/testing/mock_agent.rs).
            Box::pin(async move {
                let answer = self.execute(task).await?;
                let event_stream = futures::stream::once(async move {
                    Ok(echo_core::agent::AgentEvent::FinalAnswer(answer))
                });
                Ok(Box::pin(event_stream) as BoxStream<'a, Result<echo_core::agent::AgentEvent>>)
            })
        }
    }

    #[tokio::test]
    async fn arc_agent_box_delegates_execute() {
        let calls = Arc::new(AtomicU32::new(0));
        let agent: Arc<dyn Agent> = Arc::new(StubAgent {
            name: "stub".into(),
            call_count: calls.clone(),
        });
        // Wrap and run through the box — delegation must reach the inner Arc.
        let boxed: ArcAgentBox = ArcAgentBox(agent);
        let _ = Agent::execute(&boxed, "do something").await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "execute must delegate");
        assert_eq!(boxed.name(), "stub");
        assert_eq!(boxed.model_name(), "stub");
    }

    #[test]
    fn arc_agent_box_name_methods_delegate() {
        let agent: Arc<dyn Agent> = Arc::new(StubAgent {
            name: "alpha".into(),
            call_count: Arc::new(AtomicU32::new(0)),
        });
        let boxed = ArcAgentBox(agent);
        assert_eq!(boxed.name(), "alpha");
        assert_eq!(boxed.model_name(), "stub");
        assert_eq!(boxed.system_prompt(), "");
    }

    /// Compile-time guard: ArcAgentBox implements Agent (so it can be Box<dyn Agent>).
    #[allow(dead_code)]
    fn _assert_agent_impl(x: ArcAgentBox) -> Box<dyn Agent> {
        Box::new(x)
    }

    /// Compile-time guard: Arc<dyn Agent> is NOT directly Boxable as dyn Agent
    /// (this is the problem ArcAgentBox solves). Uncomment to confirm it fails:
    // fn _deny_direct(x: Arc<dyn Agent>) -> Box<dyn Agent> { Box::new(x) } // won't compile
    #[allow(dead_code)]
    fn _deny_direct(_x: Arc<dyn Agent>) -> Result<()> {
        // Documenting the constraint; the type system prevents Box::new(arc).
        Err(ReactError::Other("not boxable directly".into()))
    }
}
