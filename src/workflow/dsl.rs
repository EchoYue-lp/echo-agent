//! 高层声明式 DSL — StateGraph
//!
//! 提供 LangGraph 风格的链式 API，用于声明式构建多 Agent 拓扑。
//!
//! # 示例
//!
//! ```rust,ignore
//! use echo_agent::workflow::dsl::StateGraph;
//! use echo_agent::prelude::*;
//!
//! let mut sg = StateGraph::new("research_flow");
//! sg.add_react_node("researcher", |b| {
//!         b.model("qwen3-max").system_prompt("研究员").enable_tools()
//!     })
//!     .with_input("topic").with_output("research");
//! sg.add_react_node("writer", |b| {
//!         b.model("qwen3-max").system_prompt("作者")
//!     })
//!     .with_input("research").with_output("draft");
//! sg.add_function_node("checker", |state| Box::pin(async move {
//!         let draft: String = state.get("draft").unwrap_or_default();
//!         state.set("review", if draft.len() > 100 { "pass" } else { "fail" });
//!         Ok(())
//!     }));
//! sg.add_conditional_edge("checker", |state| Box::pin(async {
//!         let r: String = state.get("review").unwrap_or_default();
//!         if r == "pass" { "done" } else { "writer" }.to_string()
//!     }));
//! sg.add_react_node("done", |b| {
//!         b.model("qwen3-max").system_prompt("润色")
//!     })
//!     .with_input("draft").with_output("final");
//! sg.entry("researcher").finish("done");
//! let graph = sg.compile()?;
//! ```

use crate::agents::react::builder::ReactAgentBuilder;
use crate::error::Result;
use crate::workflow::graph::{Graph, GraphBuilder};
use crate::workflow::state::SharedState;
use futures::future::BoxFuture;

// ── Pending node definition ──────────────────────────────────────────────

type AgentConfigFn = Box<dyn FnOnce(ReactAgentBuilder) -> ReactAgentBuilder>;
type FunctionNodeFn =
    Box<dyn for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync>;
type ConditionFn = Box<dyn for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync>;

enum PendingNode {
    Agent {
        name: String,
        config_fn: AgentConfigFn,
        input_key: String,
        output_key: String,
    },
    Function {
        name: String,
        f: FunctionNodeFn,
    },
    Router {
        name: String,
    },
}

enum PendingEdge {
    Fixed {
        from: String,
        to: String,
    },
    Conditional {
        from: String,
        f: ConditionFn,
    },
    Parallel {
        from: String,
        targets: Vec<String>,
        then: String,
    },
}

// ── StateGraph ───────────────────────────────────────────────────────────

/// 声明式 Agent 拓扑构建器。
///
/// 通过链式 API 定义节点、边和状态字段，最终 `compile()` 编译为 `Graph`。
///
/// 所有 `add_*` 和 `with_*` 方法返回 `&mut Self`，支持链式调用。
pub struct StateGraph {
    name: String,
    pending_nodes: Vec<PendingNode>,
    edges: Vec<PendingEdge>,
    entry: Option<String>,
    finish_nodes: Vec<String>,
}

impl StateGraph {
    /// 创建一个新的 StateGraph
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pending_nodes: Vec::new(),
            edges: Vec::new(),
            entry: None,
            finish_nodes: Vec::new(),
        }
    }

    /// 添加一个 ReactAgent 节点。
    ///
    /// `configure` 闭包接收 `ReactAgentBuilder`，返回配置后的 builder。
    /// 默认 input="task", output="result"。
    pub fn add_react_node<F>(&mut self, name: impl Into<String>, configure: F) -> &mut Self
    where
        F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder + 'static,
    {
        self.pending_nodes.push(PendingNode::Agent {
            name: name.into(),
            config_fn: Box::new(configure),
            input_key: "task".to_string(),
            output_key: "result".to_string(),
        });
        self
    }

    /// 设置最后一个 agent 节点的 input key
    pub fn with_input(&mut self, key: impl Into<String>) -> &mut Self {
        if let Some(PendingNode::Agent { input_key, .. }) = self.pending_nodes.last_mut() {
            *input_key = key.into();
        }
        self
    }

    /// 设置最后一个 agent 节点的 output key
    pub fn with_output(&mut self, key: impl Into<String>) -> &mut Self {
        if let Some(PendingNode::Agent { output_key, .. }) = self.pending_nodes.last_mut() {
            *output_key = key.into();
        }
        self
    }

    /// 添加函数节点
    pub fn add_function_node<F>(&mut self, name: impl Into<String>, f: F) -> &mut Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        self.pending_nodes.push(PendingNode::Function {
            name: name.into(),
            f: Box::new(f),
        });
        self
    }

    /// 添加路由节点
    pub fn add_router(&mut self, name: impl Into<String>) -> &mut Self {
        self.pending_nodes
            .push(PendingNode::Router { name: name.into() });
        self
    }

    /// 添加固定边：from → to
    pub fn add_edge(&mut self, from: impl Into<String>, to: impl Into<String>) -> &mut Self {
        self.edges.push(PendingEdge::Fixed {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// 添加条件边
    pub fn add_conditional_edge<F>(&mut self, from: impl Into<String>, f: F) -> &mut Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync + 'static,
    {
        self.edges.push(PendingEdge::Conditional {
            from: from.into(),
            f: Box::new(f),
        });
        self
    }

    /// 添加并行边：from → [targets...] → then
    pub fn add_parallel_edge(
        &mut self,
        from: impl Into<String>,
        targets: Vec<String>,
        then: impl Into<String>,
    ) -> &mut Self {
        self.edges.push(PendingEdge::Parallel {
            from: from.into(),
            targets,
            then: then.into(),
        });
        self
    }

    /// 设置入口节点
    pub fn entry(&mut self, name: impl Into<String>) -> &mut Self {
        self.entry = Some(name.into());
        self
    }

    /// 添加结束节点
    pub fn finish(&mut self, name: impl Into<String>) -> &mut Self {
        self.finish_nodes.push(name.into());
        self
    }

    /// 编译为 Graph
    ///
    /// 遍历所有 pending nodes，构建 agents，添加边和入口/终点，最终 build。
    pub fn compile(self) -> Result<Graph> {
        let mut builder = GraphBuilder::new(&self.name);

        // Build all pending nodes
        for node in self.pending_nodes {
            match node {
                PendingNode::Agent {
                    name,
                    config_fn,
                    input_key,
                    output_key,
                } => {
                    let base = ReactAgentBuilder::new();
                    let configured = config_fn(base);
                    let agent = configured.build()?;
                    builder = builder.add_agent_node(&name, agent, &input_key, &output_key);
                }
                PendingNode::Function { name, f } => {
                    builder = builder.add_function_node(&name, f);
                }
                PendingNode::Router { name } => {
                    builder = builder.add_router_node(&name);
                }
            }
        }

        // Add all edges
        for edge in self.edges {
            match edge {
                PendingEdge::Fixed { from, to } => {
                    builder = builder.add_edge(&from, &to);
                }
                PendingEdge::Conditional { from, f } => {
                    builder = builder.add_conditional_edge(&from, f);
                }
                PendingEdge::Parallel {
                    from,
                    targets,
                    then,
                } => {
                    builder = builder.add_parallel_edge(&from, targets, &then);
                }
            }
        }

        // Set entry and finish
        if let Some(entry) = self.entry {
            builder = builder.set_entry(&entry);
        }
        for finish in self.finish_nodes {
            builder = builder.set_finish(&finish);
        }

        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_state_graph_function_nodes() {
        let mut sg = StateGraph::new("test_flow");
        sg.add_function_node("step1", |state| {
            Box::pin(async move {
                let _ = state.set("x", "hello");
                Ok(())
            })
        })
        .add_function_node("step2", |state| {
            Box::pin(async move {
                let x: String = state.get("x").unwrap_or_default();
                let _ = state.set("y", format!("{} world", x));
                Ok(())
            })
        })
        .add_edge("step1", "step2")
        .add_edge("step2", "__end__")
        .entry("step1")
        .finish("step2");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        let y: String = result.state.get("y").unwrap_or_default();
        assert_eq!(y, "hello world");
    }

    #[tokio::test]
    async fn test_state_graph_run() {
        let mut sg = StateGraph::new("hello_flow");
        sg.add_function_node("greet", |state| {
            Box::pin(async move {
                let _ = state.set("msg", "hello");
                Ok(())
            })
        })
        .add_function_node("shout", |state| {
            Box::pin(async move {
                let msg: String = state.get("msg").unwrap_or_default();
                let _ = state.set("msg", format!("{}!", msg.to_uppercase()));
                Ok(())
            })
        })
        .add_edge("greet", "shout")
        .add_edge("shout", "__end__")
        .entry("greet")
        .finish("shout");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        let msg: String = result.state.get("msg").unwrap_or_default();
        assert_eq!(msg, "HELLO!");
    }

    #[tokio::test]
    async fn test_state_graph_conditional() {
        let mut sg = StateGraph::new("cond_flow");
        sg.add_function_node("check", |state| {
            Box::pin(async move {
                let _ = state.set("status", "ok");
                Ok(())
            })
        })
        .add_function_node("pass", |state| {
            Box::pin(async move {
                let _ = state.set("result", "passed");
                Ok(())
            })
        })
        .add_function_node("fail", |state| {
            Box::pin(async move {
                let _ = state.set("result", "failed");
                Ok(())
            })
        })
        .add_conditional_edge("check", |state| {
            Box::pin(async {
                let s: String = state.get("status").unwrap_or_default();
                if s == "ok" { "pass" } else { "fail" }.to_string()
            })
        })
        .add_edge("pass", "__end__")
        .add_edge("fail", "__end__")
        .entry("check")
        .finish("pass")
        .finish("fail");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.steps, 2); // check + pass
    }
}
