//! 图工作流节点
//!
//! 每个节点是图中的一个执行单元，可以是：
//! - **Agent 节点**：调用 `Agent::execute()` 并将结果写入 state
//! - **函数节点**：任意 `async fn(SharedState) -> Result<()>`
//! - **Router 节点**：纯路由（不执行，仅做条件分支）

use super::state::SharedState;
use crate::agent::Agent;
use crate::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── NodeAction ──────────────────────────────────────────────────────────────

/// 节点执行逻辑的类型安全封装
pub(crate) enum NodeAction {
    /// Agent 执行：从 state 中读取 input_key 作为 prompt，输出写入 output_key
    Agent {
        agent: Arc<Mutex<Box<dyn Agent>>>,
        input_key: String,
        output_key: String,
    },
    /// 自定义异步函数
    Function(Box<dyn NodeFn>),
    /// 空操作（用于 router 节点）
    Passthrough,
}

/// 自定义节点函数 trait（object-safe）
pub(crate) trait NodeFn: Send + Sync {
    fn call<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, Result<()>>;
}

/// 用闭包实现 NodeFn
struct FnWrapper<F>(F);

impl<F> NodeFn for FnWrapper<F>
where
    F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync,
{
    fn call<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, Result<()>> {
        (self.0)(state)
    }
}

// ── Node ────────────────────────────────────────────────────────────────────

/// 图中的节点定义
#[allow(dead_code)]
pub(crate) struct Node {
    /// 节点唯一名称
    pub name: String,
    /// 执行逻辑
    pub action: NodeAction,
}

impl Node {
    /// 创建 Agent 节点
    pub fn agent(
        name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            action: NodeAction::Agent {
                agent: Arc::new(Mutex::new(Box::new(agent))),
                input_key: input_key.into(),
                output_key: output_key.into(),
            },
        }
    }

    /// 创建 Agent 节点（已封装为 Arc<Mutex<Box<dyn Agent>>>）
    pub fn agent_shared(
        name: impl Into<String>,
        agent: Arc<Mutex<Box<dyn Agent>>>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            action: NodeAction::Agent {
                agent,
                input_key: input_key.into(),
                output_key: output_key.into(),
            },
        }
    }

    /// 创建函数节点
    pub fn function<F>(name: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            action: NodeAction::Function(Box::new(FnWrapper(f))),
        }
    }

    /// 创建 passthrough（路由）节点
    pub fn passthrough(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: NodeAction::Passthrough,
        }
    }

    /// 执行节点
    pub async fn execute(&self, state: &SharedState) -> Result<()> {
        match &self.action {
            NodeAction::Agent {
                agent,
                input_key,
                output_key,
            } => {
                let input = state
                    .get::<String>(input_key)
                    .unwrap_or_else(|| String::new());

                let mut agent = agent.lock().await;
                let output = agent.execute(&input).await?;

                state.set(output_key, &output);
                // 同时追加到消息历史
                state.push_message(crate::llm::types::Message::assistant(output));
                Ok(())
            }
            NodeAction::Function(f) => f.call(state).await,
            NodeAction::Passthrough => Ok(()),
        }
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_function_node() {
        let node = Node::function("double", |state: &SharedState| {
            Box::pin(async move {
                let x: i64 = state.get("input").unwrap_or(0);
                state.set("output", x * 2);
                Ok(())
            })
        });

        let state = SharedState::new();
        state.set("input", 21i64);
        node.execute(&state).await.unwrap();
        assert_eq!(state.get::<i64>("output"), Some(42));
    }

    #[tokio::test]
    async fn test_passthrough_node() {
        let node = Node::passthrough("noop");
        let state = SharedState::new();
        state.set("x", 1);
        node.execute(&state).await.unwrap();
        assert_eq!(state.get::<i64>("x"), Some(1)); // 不变
    }

    #[tokio::test]
    async fn test_agent_node_with_mock() {
        use crate::testing::MockAgent;

        let mock = MockAgent::new("test-agent").with_response("mock answer");
        let node = Node::agent("test-agent", mock, "task", "result");

        let state = SharedState::new();
        state.set("task", "hello");
        node.execute(&state).await.unwrap();
        assert_eq!(
            state.get::<String>("result"),
            Some("mock answer".to_string())
        );
    }
}
