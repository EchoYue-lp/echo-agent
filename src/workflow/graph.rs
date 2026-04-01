//! 图工作流引擎
//!
//! 将 Agent 执行建模为**有向图**：
//!
//! - **节点（Node）**：执行单元（Agent / 函数 / 路由器）
//! - **边（Edge）**：节点间的转移逻辑（固定 / 条件 / 并行 fan-out + fan-in）
//! - **状态（SharedState）**：节点间共享的 KV store + 消息历史
//!
//! ## 与 LangGraph 对标
//!
//! | LangGraph | echo-agent workflow |
//! |-----------|---------------------|
//! | `StateGraph` | [`Graph`] |
//! | `add_node()` | [`GraphBuilder::add_node()`] |
//! | `add_edge()` | [`GraphBuilder::add_edge()`] |
//! | `add_conditional_edges()` | [`GraphBuilder::add_conditional_edge()`] |
//! | `END` | [`Graph::END`] |
//! | `compile()` | [`GraphBuilder::build()`] |
//! | `invoke()` | [`Graph::run()`] |
//!
//! ## 示例
//!
//! ```rust,no_run
//! use echo_agent::workflow::{GraphBuilder, SharedState};
//! use echo_agent::prelude::*;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let graph = GraphBuilder::new("my_workflow")
//!     .add_function_node("start", |state| Box::pin(async move {
//!         state.set("greeting", "Hello, World!");
//!         Ok(())
//!     }))
//!     .add_function_node("end", |state| Box::pin(async move {
//!         let msg: String = state.get("greeting").unwrap_or_default();
//!         println!("{msg}");
//!         Ok(())
//!     }))
//!     .set_entry("start")
//!     .add_edge("start", "end")
//!     .set_finish("end")
//!     .build()?;
//!
//! let state = SharedState::new();
//! let result = graph.run(state).await?;
//! # Ok(())
//! # }
//! ```

use super::node::Node;
use super::state::SharedState;
use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

// ── Edge ────────────────────────────────────────────────────────────────────

/// 边的路由逻辑
pub(crate) enum EdgeKind {
    /// 固定转移：A → B
    Fixed(String),
    /// 条件转移：根据 state 返回下一个节点名
    Conditional(Box<dyn ConditionFn>),
    /// 并行 fan-out：同时进入多个节点，所有完成后 merge 回 then 节点
    Parallel {
        targets: Vec<String>,
        then: String,
    },
}

/// 条件函数 trait（object-safe）
pub(crate) trait ConditionFn: Send + Sync {
    fn evaluate<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, String>;
}

struct CondWrapper<F>(F);

impl<F> ConditionFn for CondWrapper<F>
where
    F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync,
{
    fn evaluate<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, String> {
        (self.0)(state)
    }
}

/// 从节点出发的所有边
pub(crate) struct Edge {
    pub from: String,
    pub kind: EdgeKind,
}

// ── GraphBuilder ────────────────────────────────────────────────────────────

/// 图工作流构建器
///
/// 通过链式调用添加节点和边，最后 `build()` 生成不可变的 [`Graph`]。
pub struct GraphBuilder {
    name: String,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    entry_node: Option<String>,
    finish_nodes: Vec<String>,
}

impl GraphBuilder {
    /// 创建空的图构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            entry_node: None,
            finish_nodes: Vec::new(),
        }
    }

    // ── 添加节点 ────────────────────────────────────────────────────────

    /// 添加 Agent 节点
    ///
    /// `input_key`: state 中读取 prompt 的 key
    /// `output_key`: 执行结果写入 state 的 key
    pub fn add_agent_node(
        mut self,
        name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent(&name, agent, input_key, output_key),
        );
        self
    }

    /// 添加共享 Agent 节点（Arc<Mutex<Box<dyn Agent>>>）
    pub fn add_shared_agent_node(
        mut self,
        name: impl Into<String>,
        agent: Arc<Mutex<Box<dyn Agent>>>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent_shared(&name, agent, input_key, output_key),
        );
        self
    }

    /// 添加函数节点
    pub fn add_function_node<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        let name = name.into();
        self.nodes
            .insert(name.clone(), Node::function(&name, f));
        self
    }

    /// 添加路由节点（不执行逻辑，仅用于条件分支的汇聚点）
    pub fn add_router_node(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.nodes
            .insert(name.clone(), Node::passthrough(&name));
        self
    }

    // ── 添加边 ──────────────────────────────────────────────────────────

    /// 添加固定边：from → to
    pub fn add_edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Fixed(to.into()),
        });
        self
    }

    /// 添加条件边：from → f(state) 返回目标节点名
    ///
    /// 条件函数返回的字符串必须是已注册的节点名或 `"__end__"`。
    pub fn add_conditional_edge<F>(mut self, from: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync + 'static,
    {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Conditional(Box::new(CondWrapper(f))),
        });
        self
    }

    /// 添加并行边：from → [targets...] → then
    ///
    /// `from` 完成后，`targets` 中的所有节点**并行执行**，全部完成后进入 `then`。
    /// 并行节点的 state 修改会在 then 节点之前 merge。
    pub fn add_parallel_edge(
        mut self,
        from: impl Into<String>,
        targets: Vec<String>,
        then: impl Into<String>,
    ) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Parallel {
                targets,
                then: then.into(),
            },
        });
        self
    }

    // ── 入口和终点 ──────────────────────────────────────────────────────

    /// 设置入口节点
    pub fn set_entry(mut self, name: impl Into<String>) -> Self {
        self.entry_node = Some(name.into());
        self
    }

    /// 设置结束节点（到达此节点后图执行完毕，可设置多个）
    pub fn set_finish(mut self, name: impl Into<String>) -> Self {
        self.finish_nodes.push(name.into());
        self
    }

    /// 构建不可变的 Graph
    pub fn build(self) -> Result<Graph> {
        let entry = self.entry_node.ok_or_else(|| {
            ReactError::Agent(AgentError::InitializationFailed(
                "Graph must have an entry node (call set_entry())".to_string(),
            ))
        })?;

        if !self.nodes.contains_key(&entry) {
            return Err(ReactError::Agent(AgentError::InitializationFailed(
                format!("Entry node '{}' not found in graph", entry),
            )));
        }

        // 校验所有边引用的节点都存在
        for edge in &self.edges {
            if !self.nodes.contains_key(&edge.from) {
                return Err(ReactError::Agent(AgentError::InitializationFailed(
                    format!("Edge from unknown node '{}'", edge.from),
                )));
            }
            match &edge.kind {
                EdgeKind::Fixed(to) if to != Graph::END => {
                    if !self.nodes.contains_key(to) {
                        return Err(ReactError::Agent(AgentError::InitializationFailed(
                            format!("Edge to unknown node '{}'", to),
                        )));
                    }
                }
                EdgeKind::Parallel { targets, then } => {
                    for t in targets {
                        if !self.nodes.contains_key(t) {
                            return Err(ReactError::Agent(AgentError::InitializationFailed(
                                format!("Parallel target node '{}' not found", t),
                            )));
                        }
                    }
                    if then != Graph::END && !self.nodes.contains_key(then) {
                        return Err(ReactError::Agent(AgentError::InitializationFailed(
                            format!("Parallel 'then' node '{}' not found", then),
                        )));
                    }
                }
                _ => {}
            }
        }

        // 构建邻接表
        let mut edge_map: HashMap<String, Vec<Edge>> = HashMap::new();
        for edge in self.edges {
            edge_map.entry(edge.from.clone()).or_default().push(edge);
        }

        Ok(Graph {
            name: self.name,
            nodes: self.nodes,
            edges: edge_map,
            entry,
            finish_nodes: self.finish_nodes,
            max_steps: 100,
        })
    }
}

// ── Graph ───────────────────────────────────────────────────────────────────

/// 编译后的图工作流（不可变）
///
/// 通过 [`GraphBuilder`] 构建，调用 [`run()`](Graph::run) 执行。
pub struct Graph {
    /// 图名称
    pub name: String,
    /// 节点注册表
    nodes: HashMap<String, Node>,
    /// 邻接表：from → [Edge]
    edges: HashMap<String, Vec<Edge>>,
    /// 入口节点
    entry: String,
    /// 结束节点列表
    finish_nodes: Vec<String>,
    /// 最大执行步数（防止无限循环）
    max_steps: usize,
}

/// 图执行结果
#[derive(Debug)]
pub struct GraphResult {
    /// 最终状态
    pub state: SharedState,
    /// 执行路径（节点名序列）
    pub path: Vec<String>,
    /// 总步数
    pub steps: usize,
}

impl Graph {
    /// 终止标记节点名
    pub const END: &'static str = "__end__";

    /// 设置最大执行步数
    pub fn set_max_steps(&mut self, max: usize) {
        self.max_steps = max;
    }

    /// 执行图工作流
    ///
    /// 从 entry 节点开始，按边的路由逻辑依次执行节点，直到到达 finish 节点或 `__end__`。
    pub async fn run(&self, state: SharedState) -> Result<GraphResult> {
        let mut current = self.entry.clone();
        let mut path = Vec::new();
        let mut step_count = 0;

        info!(graph = %self.name, entry = %current, "Starting graph execution");

        loop {
            // 防止无限循环
            if step_count >= self.max_steps {
                warn!(
                    graph = %self.name,
                    steps = step_count,
                    "Graph execution exceeded max steps"
                );
                return Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                    self.max_steps,
                )));
            }

            // 检查终止条件
            if current == Self::END || self.finish_nodes.contains(&current) {
                // 如果是 finish 节点（非 __end__），先执行该节点
                if current != Self::END {
                    if let Some(node) = self.nodes.get(&current) {
                        state.set_current_node(&current);
                        debug!(graph = %self.name, node = %current, "Executing finish node");
                        node.execute(&state).await?;
                        path.push(current.clone());
                        step_count += 1;
                    }
                }
                info!(
                    graph = %self.name,
                    steps = step_count,
                    path = ?path,
                    "Graph execution completed"
                );
                return Ok(GraphResult {
                    state,
                    path,
                    steps: step_count,
                });
            }

            // 执行当前节点
            let node = self.nodes.get(&current).ok_or_else(|| {
                ReactError::Agent(AgentError::InitializationFailed(format!(
                    "Node '{}' not found in graph '{}'",
                    current, self.name
                )))
            })?;

            state.set_current_node(&current);
            debug!(graph = %self.name, node = %current, step = step_count, "Executing node");
            node.execute(&state).await?;
            path.push(current.clone());
            step_count += 1;

            // 路由到下一个节点
            let next = self.resolve_next(&current, &state).await?;

            match next {
                NextStep::Single(name) => {
                    current = name;
                }
                NextStep::Parallel { targets, then } => {
                    debug!(
                        graph = %self.name,
                        targets = ?targets,
                        then = %then,
                        "Executing parallel fan-out"
                    );

                    // 并行分支执行
                    // 注：因为 Node 包含 dyn trait（非 Send + 'static），
                    // 无法直接 tokio::spawn。此处使用顺序执行保证正确性，
                    // 所有分支共享同一个 SharedState（Arc 内部可变）。
                    for target_name in &targets {
                        if let Some(target_node) = self.nodes.get(target_name) {
                            state.set_current_node(target_name);
                            debug!(graph = %self.name, node = %target_name, "Executing parallel branch");
                            target_node.execute(&state).await?;
                            path.push(target_name.clone());
                            step_count += 1;
                        }
                    }

                    current = then;
                }
                NextStep::End => {
                    info!(
                        graph = %self.name,
                        steps = step_count,
                        path = ?path,
                        "Graph execution completed (reached END)"
                    );
                    return Ok(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    });
                }
            }
        }
    }

    /// 解析下一个节点
    async fn resolve_next(&self, current: &str, state: &SharedState) -> Result<NextStep> {
        let edges = match self.edges.get(current) {
            Some(e) => e,
            None => {
                // 没有出边 → 如果是 finish 节点则结束，否则报错
                if self.finish_nodes.contains(&current.to_string()) {
                    return Ok(NextStep::End);
                }
                return Err(ReactError::Agent(AgentError::InitializationFailed(
                    format!(
                        "Node '{}' has no outgoing edges and is not a finish node",
                        current
                    ),
                )));
            }
        };

        // 取第一条匹配的边
        for edge in edges {
            match &edge.kind {
                EdgeKind::Fixed(to) => {
                    if to == Self::END {
                        return Ok(NextStep::End);
                    }
                    return Ok(NextStep::Single(to.clone()));
                }
                EdgeKind::Conditional(f) => {
                    let target = f.evaluate(state).await;
                    if target == Self::END {
                        return Ok(NextStep::End);
                    }
                    return Ok(NextStep::Single(target));
                }
                EdgeKind::Parallel { targets, then } => {
                    return Ok(NextStep::Parallel {
                        targets: targets.clone(),
                        then: then.clone(),
                    });
                }
            }
        }

        // 不应到达
        Ok(NextStep::End)
    }
}

/// 内部路由结果
enum NextStep {
    Single(String),
    Parallel { targets: Vec<String>, then: String },
    End,
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_linear_graph() {
        let graph = GraphBuilder::new("linear")
            .add_function_node("a", |state: &SharedState| {
                Box::pin(async move {
                    state.set("x", 1i64);
                    Ok(())
                })
            })
            .add_function_node("b", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    state.set("x", x + 10);
                    Ok(())
                })
            })
            .add_function_node("c", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    state.set("x", x * 2);
                    Ok(())
                })
            })
            .set_entry("a")
            .add_edge("a", "b")
            .add_edge("b", "c")
            .set_finish("c")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();

        assert_eq!(result.state.get::<i64>("x"), Some(22)); // (1+10)*2
        assert_eq!(result.path, vec!["a", "b", "c"]);
        assert_eq!(result.steps, 3);
    }

    #[tokio::test]
    async fn test_conditional_graph() {
        let graph = GraphBuilder::new("conditional")
            .add_function_node("check", |_state: &SharedState| {
                Box::pin(async move {
                    // score 由外部设置
                    Ok(())
                })
            })
            .add_function_node("pass", |state: &SharedState| {
                Box::pin(async move {
                    state.set("result", "passed");
                    Ok(())
                })
            })
            .add_function_node("fail", |state: &SharedState| {
                Box::pin(async move {
                    state.set("result", "failed");
                    Ok(())
                })
            })
            .set_entry("check")
            .add_conditional_edge("check", |state: &SharedState| {
                Box::pin(async move {
                    let score: i64 = state.get("score").unwrap_or(0);
                    if score >= 60 {
                        "pass".to_string()
                    } else {
                        "fail".to_string()
                    }
                })
            })
            .set_finish("pass")
            .set_finish("fail")
            .build()
            .unwrap();

        // 测试通过路径
        let state = SharedState::new();
        state.set("score", 80i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<String>("result"), Some("passed".to_string()));
        assert_eq!(result.path, vec!["check", "pass"]);

        // 测试失败路径
        let state = SharedState::new();
        state.set("score", 40i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<String>("result"), Some("failed".to_string()));
        assert_eq!(result.path, vec!["check", "fail"]);
    }

    #[tokio::test]
    async fn test_loop_graph() {
        // 模拟循环：counter 从 0 递增到 5
        let graph = GraphBuilder::new("loop")
            .add_function_node("init", |state: &SharedState| {
                Box::pin(async move {
                    state.set("counter", 0i64);
                    Ok(())
                })
            })
            .add_function_node("increment", |state: &SharedState| {
                Box::pin(async move {
                    let c: i64 = state.get("counter").unwrap();
                    state.set("counter", c + 1);
                    Ok(())
                })
            })
            .add_function_node("done", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .set_entry("init")
            .add_edge("init", "increment")
            .add_conditional_edge("increment", |state: &SharedState| {
                Box::pin(async move {
                    let c: i64 = state.get("counter").unwrap_or(0);
                    if c >= 5 { "done".to_string() } else { "increment".to_string() }
                })
            })
            .set_finish("done")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<i64>("counter"), Some(5));
        // init + 5*increment + done = 7 steps
        assert_eq!(result.steps, 7);
    }

    #[tokio::test]
    async fn test_parallel_graph() {
        let graph = GraphBuilder::new("parallel")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    state.set("input", "hello");
                    Ok(())
                })
            })
            .add_function_node("upper", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    state.set("upper_result", s.to_uppercase());
                    Ok(())
                })
            })
            .add_function_node("length", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    state.set("length_result", s.len() as i64);
                    Ok(())
                })
            })
            .add_function_node("combine", |state: &SharedState| {
                Box::pin(async move {
                    let u: String = state.get("upper_result").unwrap();
                    let l: i64 = state.get("length_result").unwrap();
                    state.set("final", format!("{u} (len={l})"));
                    Ok(())
                })
            })
            .set_entry("start")
            .add_parallel_edge(
                "start",
                vec!["upper".to_string(), "length".to_string()],
                "combine",
            )
            .set_finish("combine")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("final"),
            Some("HELLO (len=5)".to_string())
        );
    }

    #[tokio::test]
    async fn test_end_edge() {
        let graph = GraphBuilder::new("end_test")
            .add_function_node("only", |state: &SharedState| {
                Box::pin(async move {
                    state.set("done", true);
                    Ok(())
                })
            })
            .set_entry("only")
            .add_edge("only", "__end__")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<bool>("done"), Some(true));
        assert_eq!(result.path, vec!["only"]);
    }

    #[tokio::test]
    async fn test_max_steps_exceeded() {
        let mut graph = GraphBuilder::new("infinite")
            .add_function_node("loop_node", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .set_entry("loop_node")
            .add_edge("loop_node", "loop_node") // 无限循环
            .build()
            .unwrap();

        graph.set_max_steps(10);

        let state = SharedState::new();
        let result = graph.run(state).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_missing_entry() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_unknown_entry() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_unknown_edge_target() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("a")
            .add_edge("a", "nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_agent_node_in_graph() {
        use crate::testing::MockAgent;

        let mock = MockAgent::new("solver").with_response("graph agent output");

        let graph = GraphBuilder::new("agent_graph")
            .add_function_node("prepare", |state: &SharedState| {
                Box::pin(async move {
                    state.set("task", "What is 2+2?");
                    Ok(())
                })
            })
            .add_agent_node("solver", mock, "task", "answer")
            .add_function_node("verify", |state: &SharedState| {
                Box::pin(async move {
                    let answer: String = state.get("answer").unwrap();
                    state.set("verified", !answer.is_empty());
                    Ok(())
                })
            })
            .set_entry("prepare")
            .add_edge("prepare", "solver")
            .add_edge("solver", "verify")
            .set_finish("verify")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("answer"),
            Some("graph agent output".to_string())
        );
        assert_eq!(result.state.get::<bool>("verified"), Some(true));
        assert_eq!(result.path, vec!["prepare", "solver", "verify"]);
    }
}
