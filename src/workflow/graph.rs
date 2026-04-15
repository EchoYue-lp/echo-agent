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
//!         let _ = state.set("greeting", "Hello, World!");
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

use super::WorkflowEvent;
use super::checkpoint_store::{Checkpoint, CheckpointStore, InterruptType, MemoryCheckpointStore};
use super::node::Node;
use super::state::SharedState;
use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result};
use crate::human_loop::ApprovalDecision;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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
    Parallel { targets: Vec<String>, then: String },
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

// ── Interrupt Configuration ────────────────────────────────────────────────────

/// Interrupt 配置
#[derive(Debug, Clone, Default)]
pub struct InterruptConfig {
    /// 进入这些节点前暂停
    pub before: Vec<String>,
    /// 这些节点执行后暂停
    pub after: Vec<String>,
}

impl InterruptConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查节点是否需要在进入前暂停
    pub fn should_interrupt_before(&self, node_name: &str) -> bool {
        self.before.iter().any(|n| n == node_name || n == "*")
    }

    /// 检查节点是否需要在执行后暂停
    pub fn should_interrupt_after(&self, node_name: &str) -> bool {
        self.after.iter().any(|n| n == node_name || n == "*")
    }

    /// 检查是否需要任何 interrupt
    pub fn has_interrupts(&self) -> bool {
        !self.before.is_empty() || !self.after.is_empty()
    }
}

// ── Interrupt State ────────────────────────────────────────────────────────────

/// Interrupt 状态 - 执行暂停时的状态
#[derive(Debug)]
pub struct InterruptState {
    /// Checkpoint（可用于恢复）
    pub checkpoint: Checkpoint,
    /// Interrupt 类型
    pub interrupt_type: InterruptType,
    /// 暂停的节点名
    pub pending_node: String,
    /// 给用户的提示
    pub prompt: String,
}

impl InterruptState {
    /// 创建 BeforeNode interrupt
    pub fn before_node(checkpoint: Checkpoint, node_name: String) -> Self {
        let prompt = format!("节点 '{}' 执行前需要确认", node_name);
        Self {
            checkpoint,
            interrupt_type: InterruptType::BeforeNode,
            pending_node: node_name,
            prompt,
        }
    }

    /// 创建 AfterNode interrupt
    pub fn after_node(checkpoint: Checkpoint, node_name: String) -> Self {
        let prompt = format!("节点 '{}' 执行后需要确认", node_name);
        Self {
            checkpoint,
            interrupt_type: InterruptType::AfterNode,
            pending_node: node_name,
            prompt,
        }
    }

    /// 创建 ToolApproval interrupt
    pub fn tool_approval(checkpoint: Checkpoint, tool_name: String, args: Value) -> Self {
        let prompt = format!(
            "工具 '{}' 需要审批\n参数: {}",
            tool_name,
            serde_json::to_string_pretty(&args).unwrap_or_default()
        );
        Self {
            checkpoint,
            interrupt_type: InterruptType::ToolApproval,
            pending_node: tool_name,
            prompt,
        }
    }
}

/// run_until_interrupt 的返回类型
#[derive(Debug)]
pub enum RunUntilInterruptResult {
    /// 执行完成
    Completed(GraphResult),
    /// 遇到 interrupt 点暂停
    Interrupted(InterruptState),
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
    /// Interrupt 配置
    interrupt_config: InterruptConfig,
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
            interrupt_config: InterruptConfig::default(),
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
        self.nodes.insert(name.clone(), Node::function(&name, f));
        self
    }

    /// 添加路由节点（不执行逻辑，仅用于条件分支的汇聚点）
    pub fn add_router_node(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.nodes.insert(name.clone(), Node::passthrough(&name));
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

    // ── Interrupt 配置 ──────────────────────────────────────────────────────

    /// 设置 interrupt_before（进入节点前暂停）
    ///
    /// 支持 "*" 通配符表示所有节点。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::workflow::GraphBuilder;
    ///
    /// let graph = GraphBuilder::new("my_flow")
    ///     .add_function_node("step1", |_| Box::pin(async { Ok(()) }))
    ///     .add_function_node("step2", |_| Box::pin(async { Ok(()) }))
    ///     .set_entry("step1")
    ///     .add_edge("step1", "step2")
    ///     .interrupt_before(vec!["step2"])  // 进入 step2 前暂停
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn interrupt_before(mut self, nodes: Vec<&str>) -> Self {
        self.interrupt_config.before = nodes.into_iter().map(String::from).collect();
        self
    }

    /// 设置 interrupt_after（节点执行后暂停）
    ///
    /// 支持 "*" 通配符表示所有节点。
    pub fn interrupt_after(mut self, nodes: Vec<&str>) -> Self {
        self.interrupt_config.after = nodes.into_iter().map(String::from).collect();
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
            interrupt_config: self.interrupt_config,
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
        })
    }

    // ── 便捷方法 ────────────────────────────────────────────────────────

    /// 快捷添加 ReactAgent 节点（默认 input="task", output="result"）
    pub fn add_react_node(self, name: impl Into<String>, agent: impl Agent + 'static) -> Self {
        self.add_agent_node(name, agent, "task", "result")
    }

    /// 快捷添加 PlanExecuteAgent 节点（默认 input="task", output="plan_result"）
    #[cfg(feature = "plan-execute")]
    pub fn add_plan_node<
        P: crate::agents::plan_execute::Planner + Send + Sync + 'static,
        E: crate::agents::plan_execute::Executor + Send + Sync + 'static,
    >(
        self,
        name: impl Into<String>,
        agent: crate::agents::plan_execute::PlanExecuteAgent<P, E>,
    ) -> Self {
        self.add_agent_node(name, agent, "task", "plan_result")
    }

    /// 快捷添加 SelfReflectionAgent 节点（默认 input="task", output="reflection_result"）
    #[cfg(feature = "self-reflection")]
    pub fn add_reflect_node<C: crate::agents::self_reflection::Critic + Send + Sync + 'static>(
        self,
        name: impl Into<String>,
        agent: crate::agents::self_reflection::SelfReflectionAgent<C>,
    ) -> Self {
        self.add_agent_node(name, agent, "task", "reflection_result")
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
    /// Interrupt 配置
    interrupt_config: InterruptConfig,
    /// Checkpoint 存储
    checkpoint_store: Arc<dyn CheckpointStore>,
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
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    debug!(graph = %self.name, node = %current, "Executing finish node");
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
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

    // ── Interrupt + Checkpoint 方法 ───────────────────────────────────────────

    /// 执行到 interrupt 点暂停
    ///
    /// 如果配置了 `interrupt_before` 或 `interrupt_after`，执行到相应节点时会暂停
    /// 并返回 `InterruptState`。可以通过 `resume()` 方法继续执行。
    ///
    /// # Returns
    ///
    /// - `RunUntilInterruptResult::Completed(GraphResult)` - 执行完成
    /// - `RunUntilInterruptResult::Interrupted(InterruptState)` - 遇到 interrupt 点暂停
    pub async fn run_until_interrupt(&self, state: SharedState) -> Result<RunUntilInterruptResult> {
        let mut current = self.entry.clone();
        let mut path = Vec::new();
        let mut step_count = 0;

        info!(graph = %self.name, entry = %current, "Starting graph execution (with interrupt)");

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

            // 检查 interrupt_before
            if self.interrupt_config.should_interrupt_before(&current) {
                debug!(graph = %self.name, node = %current, "Interrupt before node");

                let checkpoint = Checkpoint::new(
                    self.name.clone(),
                    current.clone(),
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::BeforeNode,
                );

                // 保存 checkpoint
                self.checkpoint_store.save(&checkpoint).await?;

                let interrupt_state = InterruptState::before_node(checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

            // 检查终止条件
            if current == Self::END || self.finish_nodes.contains(&current) {
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    debug!(graph = %self.name, node = %current, "Executing finish node");
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
                }
                info!(
                    graph = %self.name,
                    steps = step_count,
                    path = ?path,
                    "Graph execution completed"
                );
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state,
                    path,
                    steps: step_count,
                }));
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

            // 检查 interrupt_after
            if self.interrupt_config.should_interrupt_after(&current) {
                debug!(graph = %self.name, node = %current, "Interrupt after node");

                // 获取下一个节点
                let next = self.resolve_next(&current, &state).await?;

                let checkpoint = Checkpoint::new(
                    self.name.clone(),
                    match next {
                        NextStep::Single(ref name) => name.clone(),
                        NextStep::Parallel { ref then, .. } => then.clone(),
                        NextStep::End => "__end__".to_string(),
                    },
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::AfterNode,
                );

                self.checkpoint_store.save(&checkpoint).await?;

                let interrupt_state = InterruptState::after_node(checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

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
                    return Ok(RunUntilInterruptResult::Completed(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    }));
                }
            }
        }
    }

    /// 从 Checkpoint 恢复执行
    ///
    /// 当用户批准继续后，从保存的 checkpoint 恢复执行。
    ///
    /// 修复：遇到 interrupt 点时返回 `RunUntilInterruptResult::Interrupted` 而非 `Err`。
    pub async fn resume(
        &self,
        checkpoint: Checkpoint,
        _decision: ApprovalDecision,
    ) -> Result<RunUntilInterruptResult> {
        // 恢复状态
        let state = checkpoint.restore_state()?;
        let mut current = checkpoint.current_node;
        let mut path = checkpoint.path;
        let mut step_count = checkpoint.step_count;

        info!(
            graph = %self.name,
            checkpoint_id = %checkpoint.id,
            node = %current,
            "Resuming from checkpoint"
        );

        // 删除已使用的 checkpoint
        self.checkpoint_store.delete(&checkpoint.id).await?;

        // 继续执行
        loop {
            if step_count >= self.max_steps {
                return Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                    self.max_steps,
                )));
            }

            // 检查 interrupt_before（跳过，因为已经处理过了）
            // 如果是从 BeforeNode interrupt 恢复，需要执行当前节点

            if current == Self::END || self.finish_nodes.contains(&current) {
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
                }
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state,
                    path,
                    steps: step_count,
                }));
            }

            // 执行当前节点
            let node = self.nodes.get(&current).ok_or_else(|| {
                ReactError::Agent(AgentError::InitializationFailed(format!(
                    "Node '{}' not found",
                    current
                )))
            })?;

            state.set_current_node(&current);
            node.execute(&state).await?;
            path.push(current.clone());
            step_count += 1;

            // 检查 interrupt_after
            if self.interrupt_config.should_interrupt_after(&current) {
                let next = self.resolve_next(&current, &state).await?;

                let next_node_name = match &next {
                    NextStep::Single(name) => name.clone(),
                    NextStep::Parallel { then, .. } => then.clone(),
                    NextStep::End => "__end__".to_string(),
                };

                let new_checkpoint = Checkpoint::new(
                    self.name.clone(),
                    next_node_name,
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::AfterNode,
                );

                self.checkpoint_store.save(&new_checkpoint).await?;

                let interrupt_state = InterruptState::after_node(new_checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

            let next = self.resolve_next(&current, &state).await?;

            match next {
                NextStep::Single(name) => {
                    // 检查下一个节点的 interrupt_before
                    if self.interrupt_config.should_interrupt_before(&name) {
                        let new_checkpoint = Checkpoint::new(
                            self.name.clone(),
                            name.clone(),
                            &state,
                            path.clone(),
                            step_count,
                            InterruptType::BeforeNode,
                        );
                        self.checkpoint_store.save(&new_checkpoint).await?;

                        let interrupt_state = InterruptState::before_node(new_checkpoint, name);
                        return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
                    }
                    current = name;
                }
                NextStep::Parallel { targets, then } => {
                    for target_name in &targets {
                        if let Some(target_node) = self.nodes.get(target_name) {
                            state.set_current_node(target_name);
                            target_node.execute(&state).await?;
                            path.push(target_name.clone());
                            step_count += 1;
                        }
                    }
                    current = then;
                }
                NextStep::End => {
                    return Ok(RunUntilInterruptResult::Completed(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    }));
                }
            }
        }
    }

    /// 从 checkpoint 恢复执行，同时注入状态修改。
    ///
    /// 在恢复前将 `state_updates` 合并到 checkpoint 的状态中，
    /// 适用于需要在外部修改 workflow 状态后继续执行的场景。
    pub async fn resume_with_state(
        &self,
        checkpoint: Checkpoint,
        state_updates: std::collections::HashMap<String, Value>,
    ) -> Result<RunUntilInterruptResult> {
        // Apply state updates to the checkpoint before resuming
        let state = checkpoint.restore_state()?;
        for (key, value) in &state_updates {
            let _ = state.set(key, value.clone());
        }

        // Create a modified checkpoint with updated state
        let modified_checkpoint = Checkpoint::new(
            checkpoint.graph_name.clone(),
            checkpoint.current_node.clone(),
            &state,
            checkpoint.path.clone(),
            checkpoint.step_count,
            checkpoint.interrupt_type,
        );

        self.resume(modified_checkpoint, ApprovalDecision::Approved)
            .await
    }

    /// 加载 Checkpoint
    pub async fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        self.checkpoint_store.load(id).await
    }

    /// 列出所有 Checkpoint
    pub async fn list_checkpoints(&self) -> Result<Vec<super::checkpoint_store::CheckpointInfo>> {
        self.checkpoint_store.list().await
    }

    /// 设置自定义 Checkpoint 存储
    pub fn with_checkpoint_store(mut self, store: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoint_store = store;
        self
    }

    /// 流式执行图工作流，逐节点发出 [`WorkflowEvent`] 事件。
    ///
    /// 每个节点的 Start/End 事件都会被发出，最后发出 `Completed` 事件。
    pub async fn run_stream(
        &self,
        state: SharedState,
    ) -> Result<BoxStream<'_, Result<WorkflowEvent>>> {
        let state_clone = state.clone();
        let stream = async_stream::try_stream! {
            let mut current = self.entry.clone();
            let mut path = Vec::new();
            let mut step_count = 0usize;
            let workflow_start = Instant::now();

            loop {
                if step_count >= self.max_steps {
                    Err(ReactError::Agent(AgentError::MaxIterationsExceeded(self.max_steps)))?;
                }

                if current == Self::END || self.finish_nodes.contains(&current) {
                    if current != Self::END
                        && let Some(node) = self.nodes.get(&current)
                    {
                        state_clone.set_current_node(&current);
                        yield WorkflowEvent::NodeStart {
                            node_name: current.clone(),
                            step_index: step_count,
                        };
                        let node_start = Instant::now();
                        node.execute(&state_clone).await?;
                        yield WorkflowEvent::NodeEnd {
                            node_name: current.clone(),
                            step_index: step_count,
                            elapsed: node_start.elapsed(),
                        };
                        path.push(current.clone());
                        step_count += 1;
                    }

                    let final_result = state_clone
                        .get::<String>("result")
                        .or_else(|| state_clone.get::<String>("output"))
                        .unwrap_or_default();

                    yield WorkflowEvent::Completed {
                        result: final_result,
                        total_steps: step_count,
                        elapsed: workflow_start.elapsed(),
                    };
                    return;
                }

                let node = self.nodes.get(&current).ok_or_else(|| {
                    ReactError::Agent(AgentError::InitializationFailed(format!(
                        "Node '{}' not found in graph '{}'",
                        current, self.name
                    )))
                })?;

                state_clone.set_current_node(&current);
                yield WorkflowEvent::NodeStart {
                    node_name: current.clone(),
                    step_index: step_count,
                };
                let node_start = Instant::now();
                node.execute(&state_clone).await?;
                yield WorkflowEvent::NodeEnd {
                    node_name: current.clone(),
                    step_index: step_count,
                    elapsed: node_start.elapsed(),
                };
                path.push(current.clone());
                step_count += 1;

                let next = self.resolve_next(&current, &state_clone).await?;
                match next {
                    NextStep::Single(name) => {
                        current = name;
                    }
                    NextStep::Parallel { targets, then } => {
                        for target_name in &targets {
                            if let Some(target_node) = self.nodes.get(target_name) {
                                state_clone.set_current_node(target_name);
                                yield WorkflowEvent::NodeStart {
                                    node_name: target_name.clone(),
                                    step_index: step_count,
                                };
                                let branch_start = Instant::now();
                                target_node.execute(&state_clone).await?;
                                yield WorkflowEvent::NodeEnd {
                                    node_name: target_name.clone(),
                                    step_index: step_count,
                                    elapsed: branch_start.elapsed(),
                                };
                                path.push(target_name.clone());
                                step_count += 1;
                            }
                        }
                        current = then;
                    }
                    NextStep::End => {
                        let final_result = state_clone
                            .get::<String>("result")
                            .or_else(|| state_clone.get::<String>("output"))
                            .unwrap_or_default();
                        yield WorkflowEvent::Completed {
                            result: final_result,
                            total_steps: step_count,
                            elapsed: workflow_start.elapsed(),
                        };
                        return;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
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
        if let Some(edge) = edges.iter().next() {
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
                    let _ = state.set("x", 1i64);
                    Ok(())
                })
            })
            .add_function_node("b", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("x", x + 10);
                    Ok(())
                })
            })
            .add_function_node("c", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("x", x * 2);
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
                    let _ = state.set("result", "passed");
                    Ok(())
                })
            })
            .add_function_node("fail", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "failed");
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
        let _ = state.set("score", 80i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("result"),
            Some("passed".to_string())
        );
        assert_eq!(result.path, vec!["check", "pass"]);

        // 测试失败路径
        let state = SharedState::new();
        let _ = state.set("score", 40i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("result"),
            Some("failed".to_string())
        );
        assert_eq!(result.path, vec!["check", "fail"]);
    }

    #[tokio::test]
    async fn test_loop_graph() {
        // 模拟循环：counter 从 0 递增到 5
        let graph = GraphBuilder::new("loop")
            .add_function_node("init", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("counter", 0i64);
                    Ok(())
                })
            })
            .add_function_node("increment", |state: &SharedState| {
                Box::pin(async move {
                    let c: i64 = state.get("counter").unwrap();
                    let _ = state.set("counter", c + 1);
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
                    if c >= 5 {
                        "done".to_string()
                    } else {
                        "increment".to_string()
                    }
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
                    let _ = state.set("input", "hello");
                    Ok(())
                })
            })
            .add_function_node("upper", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    let _ = state.set("upper_result", s.to_uppercase());
                    Ok(())
                })
            })
            .add_function_node("length", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    let _ = state.set("length_result", s.len() as i64);
                    Ok(())
                })
            })
            .add_function_node("combine", |state: &SharedState| {
                Box::pin(async move {
                    let u: String = state.get("upper_result").unwrap();
                    let l: i64 = state.get("length_result").unwrap();
                    let _ = state.set("final", format!("{u} (len={l})"));
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
                    let _ = state.set("done", true);
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
                    let _ = state.set("task", "What is 2+2?");
                    Ok(())
                })
            })
            .add_agent_node("solver", mock, "task", "answer")
            .add_function_node("verify", |state: &SharedState| {
                Box::pin(async move {
                    let answer: String = state.get("answer").unwrap();
                    let _ = state.set("verified", !answer.is_empty());
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

    // ── Feature 4: Workflow 流式输出 (run_stream) ────────────────────────────

    #[tokio::test]
    async fn test_run_stream_linear() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_linear")
            .add_function_node("a", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("x", 1i64);
                    Ok(())
                })
            })
            .add_function_node("b", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("result", format!("x={}", x));
                    Ok(())
                })
            })
            .set_entry("a")
            .add_edge("a", "b")
            .set_finish("b")
            .build()
            .unwrap();

        let state = SharedState::new();
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        let node_starts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeStart { node_name, .. } => Some(node_name.clone()),
                _ => None,
            })
            .collect();
        let node_ends: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeEnd { node_name, .. } => Some(node_name.clone()),
                _ => None,
            })
            .collect();
        let completed = events
            .iter()
            .any(|e| matches!(e, WorkflowEvent::Completed { .. }));

        assert_eq!(node_starts, vec!["a", "b"]);
        assert_eq!(node_ends, vec!["a", "b"]);
        assert!(completed, "应收到 Completed 事件");
    }

    #[tokio::test]
    async fn test_run_stream_parallel() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_parallel")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("val", "ok");
                    Ok(())
                })
            })
            .add_function_node("b1", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("b1_done", true);
                    Ok(())
                })
            })
            .add_function_node("b2", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("b2_done", true);
                    Ok(())
                })
            })
            .add_function_node("merge", |state: &SharedState| {
                Box::pin(async move {
                    let b1: bool = state.get("b1_done").unwrap_or(false);
                    let b2: bool = state.get("b2_done").unwrap_or(false);
                    let _ = state.set("result", format!("b1={b1},b2={b2}"));
                    Ok(())
                })
            })
            .set_entry("start")
            .add_parallel_edge("start", vec!["b1".into(), "b2".into()], "merge")
            .set_finish("merge")
            .build()
            .unwrap();

        let state = SharedState::new();
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut node_start_names = Vec::new();
        let mut completed_result = None;

        while let Some(event) = stream.next().await {
            match event.unwrap() {
                WorkflowEvent::NodeStart { node_name, .. } => {
                    node_start_names.push(node_name);
                }
                WorkflowEvent::Completed { result, .. } => {
                    completed_result = Some(result);
                }
                _ => {}
            }
        }

        assert!(node_start_names.contains(&"start".to_string()));
        assert!(node_start_names.contains(&"b1".to_string()));
        assert!(node_start_names.contains(&"b2".to_string()));
        assert!(node_start_names.contains(&"merge".to_string()));
        assert_eq!(completed_result, Some("b1=true,b2=true".to_string()));
    }

    #[tokio::test]
    async fn test_run_stream_conditional() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_cond")
            .add_function_node("check", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .add_function_node("yes", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "took_yes_path");
                    Ok(())
                })
            })
            .add_function_node("no", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "took_no_path");
                    Ok(())
                })
            })
            .set_entry("check")
            .add_conditional_edge("check", |state: &SharedState| {
                Box::pin(async move {
                    let flag: bool = state.get("flag").unwrap_or(false);
                    if flag {
                        "yes".to_string()
                    } else {
                        "no".to_string()
                    }
                })
            })
            .set_finish("yes")
            .set_finish("no")
            .build()
            .unwrap();

        let state = SharedState::new();
        let _ = state.set("flag", true);
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut visited = Vec::new();
        while let Some(event) = stream.next().await {
            if let WorkflowEvent::NodeStart { node_name, .. } = event.unwrap() {
                visited.push(node_name);
            }
        }

        assert_eq!(visited, vec!["check", "yes"]);
    }
}
