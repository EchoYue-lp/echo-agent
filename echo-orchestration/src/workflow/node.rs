//! Graph workflow nodes
//!
//! Each node is an execution unit in the graph, which can be:
//! - **Agent node**: drains a cancellable Agent stream and writes its final answer to state
//! - **Function node**: arbitrary `async fn(SharedState) -> Result<()>`
//! - **Router node**: pure routing (no execution, conditional branching only)

use super::state::SharedState;
use echo_core::agent::{Agent, AgentEvent};
use echo_core::error::{AgentError, ReactError, Result};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// ── NodeAction ──────────────────────────────────────────────────────────────

/// Type-safe wrapper for node execution logic
pub(crate) enum NodeAction {
    /// Agent execution: reads input_key from state as prompt, writes output to output_key
    Agent {
        agent: Arc<dyn Agent>,
        input_key: String,
        output_key: String,
        /// Whether to use execute (multi-turn with tools) or chat (single turn)
        use_execute: bool,
    },
    /// Nested subgraph execution — the node itself is a complete compiled Graph
    Subgraph(Box<super::graph::Graph>),
    /// Custom async function
    Function(Box<dyn NodeFn>),
    /// No-op (used for router nodes)
    Passthrough,
}

/// Custom node function trait (object-safe)
pub(crate) trait NodeFn: Send + Sync {
    fn call<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, Result<()>>;
}

/// Implements NodeFn using a closure
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

/// Node definition in the graph
pub(crate) struct Node {
    /// Execution logic
    pub action: NodeAction,
}

impl Node {
    fn commit_agent_output(state: &SharedState, output_key: &str, output: String) -> Result<()> {
        // Use merge_overwrite to support structural data merge semantics
        // rather than simple key-level overwrite.
        state.merge_overwrite(&SharedState::from_values(
            [(
                output_key.to_string(),
                serde_json::Value::String(output.clone()),
            )]
            .into_iter()
            .collect(),
        ))?;
        state.push_message(echo_core::llm::types::Message::assistant(output))?;
        Ok(())
    }

    /// Create an Agent node (defaults to execute, i.e., multi-turn with tools)
    pub fn agent(
        _name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent: Arc::new(agent),
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute: true,
            },
        }
    }

    /// Create an Agent node (configurable execute/chat mode)
    pub fn agent_with_mode(
        _name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent: Arc::new(agent),
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute,
            },
        }
    }

    /// Create an Agent node (pre-wrapped as Arc<dyn Agent>)
    pub fn agent_shared(
        _name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent,
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute: true,
            },
        }
    }

    /// Create an Agent node (pre-wrapped + configurable execute/chat)
    pub fn agent_shared_with_mode(
        _name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent,
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute,
            },
        }
    }

    /// Create a function node
    pub fn function<F>(_name: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        Self {
            action: NodeAction::Function(Box::new(FnWrapper(f))),
        }
    }

    /// Create a passthrough (router) node
    pub fn passthrough(_name: impl Into<String>) -> Self {
        Self {
            action: NodeAction::Passthrough,
        }
    }

    /// Create a subgraph node — embeds a compiled Graph as a node.
    ///
    /// The subgraph shares the parent's `SharedState` and executes within
    /// the parent's step count limit.
    pub fn subgraph(_name: impl Into<String>, graph: super::graph::Graph) -> Self {
        Self {
            action: NodeAction::Subgraph(Box::new(graph)),
        }
    }

    /// Execute through the cancellable Agent stream and expose token deltas to Graph.
    pub(crate) fn execute_stream<'a>(
        &'a self,
        state: &'a SharedState,
        cancel: CancellationToken,
        control: Arc<super::graph::GraphExecutionControl>,
        node_name: &'a str,
        execution_id: &'a str,
    ) -> BoxStream<'a, Result<String>> {
        let stream = async_stream::try_stream! {
            match &self.action {
                NodeAction::Agent { agent, input_key, output_key, use_execute } => {
                    let input = state.get::<String>(input_key).unwrap_or_default();
                    let run_id = control.run_id().ok_or_else(|| ReactError::Other("Workflow invocation has no bound run identity".into()))?;
                    let invocation = echo_core::agent::AgentInvocationContext {
                        runtime: Some(echo_core::tools::ExternalRunContext {
                            run_id: Some(run_id.clone()),
                            turn_id: Some(control.operation_id.clone()),
                            execution_id: Some(execution_id.to_string()),
                            cancel: Some(Arc::new(cancel.clone())),
                            ..Default::default()
                        }),
                        cancel: Some(cancel.clone()),
                        ..Default::default()
                    };
                    let raw = if *use_execute {
                        agent.execute_stream_message_with_invocation_context(echo_core::llm::types::Message::user(input), cancel.clone(), invocation).await
                    } else {
                        agent.chat_stream_message_with_invocation_context(echo_core::llm::types::Message::user(input), cancel.clone(), invocation).await
                    };
                    let mut events = match raw {
                        Ok(events) => events.fuse(),
                        Err(error @ ReactError::ExecutionNotSettled { .. }) => { Err(error)?; return; }
                        Err(error) => { control.record_error(Some(node_name), error); return; }
                    };
                    let mut output = echo_core::agent::AgentOutputProjection::default();
                    let mut observation = None;
                    while let Some(event) = events.next().await {
                        match event {
                            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                                // A foreign observation cannot be overwritten by
                                // matching local progress on this source.
                                let foreign_pending = observation.as_ref().is_some_and(|error: &ReactError| !matches!(error, ReactError::ExecutionNotSettled { run_id: observed_run, execution_id: Some(observed_execution) } if observed_run == &run_id && observed_execution == execution_id));
                                if !foreign_pending { observation = Some(error); }
                            }
                            Err(error) => control.record_error(Some(node_name), error),
                            Ok(event) => {
                                match event {
                                    AgentEvent::AssistantMessage { block } if !control.has_error() => {
                                        match output.observe_block(&block) {
                                            Ok(Some(delta)) if delta.block.phase != echo_core::agent::AssistantPhase::Reasoning && !delta.text.is_empty() => yield delta.text,
                                            Ok(_) => {},
                                            Err(error) => control.record_error(Some(node_name), error),
                                        }
                                    },
                                    AgentEvent::ExecutionStatus(status) => {
                                        // 先核原native身份/序号，再解除匹配unknown；被拒的合成/foreign终态无权释放owner。
                                        match output.observe_status(&status) {
                                            Ok(true) if matches!(status.status, echo_core::agent::ExecutionStatusKind::Settled | echo_core::agent::ExecutionStatusKind::Failed | echo_core::agent::ExecutionStatusKind::Cancelled) => {
                                                if observation.as_ref().is_some_and(|error| matches!(error,
                                                    ReactError::ExecutionNotSettled { run_id: observed_run, execution_id: Some(observed_execution) }
                                                        if observed_run == &run_id && observed_execution == &status.execution_id)) { observation = None; }
                                                // 原生取消/失败先入既有首因，尾部cleanup不能改写；原流仍读至EOF。
                                                if let Some(error) = output.terminal_failure() { control.record_error(Some(node_name), error); }
                                            },
                                            Ok(_) => {},
                                            Err(error) => control.record_error(Some(node_name), error),
                                        }
                                    },
                                    AgentEvent::FinalAnswer(answer) if !control.has_error() => {
                                        if let Err(error)=output.confirm_answer(&answer) {control.record_error(Some(node_name),error);}
                                    },
                                    AgentEvent::Token(_) | AgentEvent::ThinkStart | AgentEvent::ThinkEnd { .. } => control.record_error(Some(node_name), ReactError::Other("Workflow source emitted a retired assistant projection".into())),
                                    AgentEvent::Cancelled => control.record_error(Some(node_name), ReactError::Agent(Box::new(AgentError::Cancelled(format!("Workflow Agent node '{}' cancelled", agent.name()))))),
                                    AgentEvent::Error { message,failure,.. } => control.record_error(Some(node_name),match failure.terminal_kind {
                                        echo_core::error::AgentTerminalKind::Cancelled=>AgentError::Cancelled(message).into(),
                                        echo_core::error::AgentTerminalKind::TimedOut=>AgentError::Timeout(message).into(),
                                        echo_core::error::AgentTerminalKind::PermissionDenied=>AgentError::PermissionDenied(message).into(),
                                        echo_core::error::AgentTerminalKind::Failed=>ReactError::Other(message),
                                    }),
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some(error) = observation { Err(error)?; }
                    if !control.has_error() {
                        // 必须等原stream物理EOF；block complete/最终确认均不提前提交state。
                        match output.finish() {
                            Ok(answer) => {
                                match output.take_final_presentation() {
                                    Ok(Some(safe_final)) => yield safe_final,
                                    Ok(None) => {},
                                    Err(error) => {control.record_error(Some(node_name),error);return;},
                                }
                                if let Err(error)=Self::commit_agent_output(state,output_key,answer) {control.record_error(Some(node_name),error);}
                            },
                            Err(error) => control.record_error(Some(node_name),error),
                        }
                    }
                }
                NodeAction::Subgraph(subgraph) => {
                    match subgraph.run_nested(state.clone(), control.clone()).await? {
                        super::graph::GraphLoopOutcome::Settled(result) => match *result {
                            super::graph::RunUntilInterruptResult::Completed(result) => { state.deep_merge(&result.state)?; }
                            _ => control.record_error(Some(node_name), ReactError::Other("Nested Graph returned an unexpected interrupt".into())),
                        },
                        super::graph::GraphLoopOutcome::CauseRecorded => {}
                    }
                }
                NodeAction::Function(f) => {
                    match f.call(state).await {
                        Err(error @ ReactError::ExecutionNotSettled { .. }) => Err(error)?,
                        Err(error) => control.record_error(Some(node_name), error),
                        Ok(()) => {}
                    }
                }
                NodeAction::Passthrough => {}
            }
        };
        Box::pin(stream)
    }
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn drain_node(node: &Node, state: &SharedState) -> Result<()> {
        let control =
            super::super::graph::GraphExecutionControl::new(Some(uuid::Uuid::new_v4().to_string()));
        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut stream = node.execute_stream(
            state,
            CancellationToken::new(),
            control,
            "test-node",
            &execution_id,
        );
        while let Some(token) = stream.next().await {
            let _ = token?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_function_node() -> Result<()> {
        let node = Node::function("double", |state: &SharedState| {
            Box::pin(async move {
                let x: i64 = state.get("input").unwrap_or(0);
                let _ = state.set("output", x * 2);
                Ok(())
            })
        });

        let state = SharedState::new();
        state.set("input", 21i64)?;
        drain_node(&node, &state).await?;
        assert_eq!(state.get::<i64>("output"), Some(42));
        Ok(())
    }

    #[tokio::test]
    async fn test_passthrough_node() -> Result<()> {
        let node = Node::passthrough("noop");
        let state = SharedState::new();
        state.set("x", 1)?;
        drain_node(&node, &state).await?;
        assert_eq!(state.get::<i64>("x"), Some(1)); // unchanged
        Ok(())
    }
}

#[cfg(test)]
mod canonical_consumer_tests {
    use super::*;
    use echo_core::agent::{
        AgentInvocationContext, AgentPhase, AssistantMessageBlock, AssistantPhase, ExecutionStatus,
        ExecutionStatusKind,
    };
    use echo_core::llm::types::Message;

    struct NativeNodeAgent {
        text_calls: Arc<std::sync::atomic::AtomicUsize>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        tail_error: bool,
        foreign_terminal: bool,
        safe_final_only: bool,
        unsettled: Option<&'static str>,
    }
    impl Agent for NativeNodeAgent {
        fn name(&self) -> &str {
            "native-node"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _input: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Err(ReactError::Other("no text fallback".into())) })
        }
        fn execute_stream<'a>(
            &'a self,
            _input: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.text_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Err(ReactError::Other("no text fallback".into())) })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                assert_eq!(message.role, echo_core::llm::types::Role::User);
                assert_eq!(message.content.as_text().as_deref(), Some("原输入"));
                let runtime = invocation
                    .runtime
                    .ok_or_else(|| ReactError::Other("runtime missing".into()))?;
                let execution = runtime
                    .execution_id
                    .ok_or_else(|| ReactError::Other("execution missing".into()))?;
                assert!(runtime.run_id.is_some() && runtime.turn_id.is_some());
                let status = move |kind, sequence| {
                    AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: false,
                        execution_id: execution.clone(),
                        request_id: Some("request".into()),
                        sequence,
                        phase: AgentPhase::Thinking,
                        status: kind,
                        reason: None,
                    })
                };
                let entered = self.entered.clone();
                let release = self.release.clone();
                let tail_error = self.tail_error;
                let foreign = self.foreign_terminal;
                let safe_final_only = self.safe_final_only;
                let unsettled = self.unsettled;
                let stream = async_stream::stream! {
                    yield Ok(status(ExecutionStatusKind::Started,0));
                    if matches!(unsettled,Some("cancelled"|"failed")) {
                        let mut terminal=status(if unsettled==Some("cancelled") {ExecutionStatusKind::Cancelled} else {ExecutionStatusKind::Failed},1);
                        if let AgentEvent::ExecutionStatus(status)=&mut terminal {status.reason=Some("native-terminal-reason".into());}
                        yield Ok(terminal);entered.notify_one();release.notified().await;
                        if tail_error {yield Err(ReactError::Other("native cleanup failed".into()));}
                        return;
                    }
                    let block=|phase,text:&str|AgentEvent::AssistantMessage {block:AssistantMessageBlock {provider_source:None,block_id:format!("{phase:?}"),request_id:"request".into(),sequence:0,phase,delta:text.into(),complete:true,provider:None,reasoning:None}};
                    yield Ok(block(AssistantPhase::Reasoning,"不作普通输出的推理"));
                    let delta=block(AssistantPhase::Unknown,"进度");yield Ok(delta.clone());yield Ok(delta);
                    if !safe_final_only {yield Ok(block(AssistantPhase::FinalAnswer,"答案"));}
                    if let Some(kind)=unsettled.filter(|kind|*kind!="duplicate") {yield Err(ReactError::ExecutionNotSettled {run_id:"run".into(),execution_id:Some(if kind=="foreign" {"foreign"} else {"execution"}.into())});}
                    yield Ok(AgentEvent::FinalAnswer(if safe_final_only {"Guard安全答案"} else {"答案"}.into()));
                    if foreign {
                        yield Ok(AgentEvent::ExecutionStatus(ExecutionStatus {adapter_generated:false,execution_id:"foreign".into(),request_id:None,sequence:1,phase:AgentPhase::Terminal,status:ExecutionStatusKind::Settled,reason:None}));
                    } else {
                        let mut terminal=status(ExecutionStatusKind::Settled,1);
                        if let AgentEvent::ExecutionStatus(status)=&mut terminal {
                            if unsettled==Some("synthetic") {status.adapter_generated=true;}
                            if unsettled==Some("foreign") {status.execution_id="foreign".into();}
                        }
                        yield Ok(terminal.clone());
                        if unsettled==Some("duplicate") {
                            yield Err(ReactError::ExecutionNotSettled {run_id:"run".into(),execution_id:Some("execution".into())});
                            yield Ok(terminal);
                        }
                    }
                    entered.notify_one();release.notified().await;
                    if tail_error {yield Err(ReactError::Other("native cleanup failed".into()));}
                };
                Ok(Box::pin(stream) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.execute_stream_message_with_invocation_context(message, cancel, invocation)
        }
    }
    #[tokio::test]
    async fn canonical_workflow_preserves_native_first_cause_through_cleanup() -> Result<()> {
        for unsettled in [Some("cancelled"), Some("failed"), None] {
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let graph = super::super::GraphBuilder::new("native-first-cause")
                .add_agent_node(
                    "node",
                    NativeNodeAgent {
                        text_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                        entered: entered.clone(),
                        release: release.clone(),
                        tail_error: true,
                        foreign_terminal: false,
                        safe_final_only: false,
                        unsettled,
                    },
                    "input",
                    "output",
                )
                .set_entry("node")
                .set_finish("node")
                .build()?;
            let state = SharedState::new();
            state.set("input", "原输入".to_string())?;
            let inspect = state.clone();
            let mut running = tokio::spawn(async move { graph.run(state).await });
            tokio::select! {
                arrived = tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()) => {
                    arrived.map_err(|_| ReactError::Other("native first-cause EOF gate not reached".into()))?;
                },
                early = &mut running => {
                    return match early.map_err(|error| ReactError::Other(error.to_string()))? {
                        Err(error) => Err(error),
                        Ok(_) => Err(ReactError::Other("Graph returned before source EOF".into())),
                    };
                },
            }
            assert!(!running.is_finished());
            assert!(inspect.get::<String>("output").is_none());
            release.notify_one();
            let result = running
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?;
            match unsettled {
                Some("cancelled") => assert!(matches!(result, Err(ReactError::Agent(error))
                    if matches!(error.as_ref(), AgentError::Cancelled(reason) if reason == "native-terminal-reason"))),
                Some("failed") => assert!(matches!(result, Err(ReactError::Other(reason))
                    if reason == "native-terminal-reason")),
                _ => assert!(matches!(result, Err(ReactError::Other(reason))
                    if reason == "native cleanup failed")),
            }
            assert!(inspect.get::<String>("output").is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn canonical_workflow_node_merges_deltas_and_commits_only_after_physical_eof()
    -> Result<()> {
        for (tail_error, foreign_terminal, use_execute, safe_final_only, unsettled) in [
            (false, false, true, false, None),
            (false, false, false, false, None),
            (true, false, true, false, None),
            (false, true, true, false, None),
            (false, false, true, true, None),
            (false, false, true, false, Some("synthetic")),
            (false, false, true, false, Some("foreign")),
            (false, false, true, false, Some("valid")),
            (false, false, true, false, Some("duplicate")),
            (false, false, true, false, Some("cancelled")),
            (false, false, true, false, Some("failed")),
        ] {
            let text_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let node = Node::agent_with_mode(
                "node",
                NativeNodeAgent {
                    text_calls: text_calls.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                    tail_error,
                    foreign_terminal,
                    safe_final_only,
                    unsettled,
                },
                "input",
                "output",
                use_execute,
            );
            let state = SharedState::new();
            state.set("input", "原输入".to_string())?;
            let inspect = state.clone();
            let control = super::super::graph::GraphExecutionControl::new(Some("run".into()));
            let observer = control.clone();
            let mut running = tokio::spawn(async move {
                node.execute_stream(
                    &state,
                    CancellationToken::new(),
                    control,
                    "node",
                    "execution",
                )
                .collect::<Vec<_>>()
                .await
            });
            tokio::select! {
                arrived=tokio::time::timeout(std::time::Duration::from_secs(2),entered.notified())=>{
                    arrived.map_err(|_|ReactError::Other("native source EOF gate not reached".into()))?;
                },
                early=&mut running=>{
                    let items=early.map_err(|error|ReactError::Other(error.to_string()))?;
                    if text_calls.load(std::sync::atomic::Ordering::SeqCst)>0 {
                        return Err(ReactError::Other("canonical Workflow input selected retired text fallback".into()));
                    }
                    items.into_iter().collect::<Result<Vec<_>>>()?;
                    return Err(ReactError::Other("Workflow source returned before EOF gate".into()));
                },
            }
            assert!(!running.is_finished());
            assert!(inspect.get::<String>("output").is_none());
            release.notify_one();
            let raw = running
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?;
            if matches!(unsettled, Some("synthetic" | "foreign" | "duplicate")) {
                assert!(raw.iter().any(|item|matches!(item,Err(ReactError::ExecutionNotSettled {run_id,..}) if run_id=="run")));
                assert!(inspect.get::<String>("output").is_none());
                continue;
            }
            let tokens = raw.into_iter().collect::<Result<Vec<_>>>()?;
            assert_eq!(
                tokens.concat(),
                if matches!(unsettled, Some("cancelled" | "failed")) {
                    ""
                } else if safe_final_only {
                    "进度Guard安全答案"
                } else {
                    "进度答案"
                }
            );
            if tail_error || foreign_terminal || matches!(unsettled, Some("cancelled" | "failed")) {
                assert!(observer.has_error());
                assert!(inspect.get::<String>("output").is_none());
            } else {
                assert!(!observer.has_error());
                assert_eq!(
                    inspect.get::<String>("output"),
                    Some(
                        if safe_final_only {
                            "Guard安全答案"
                        } else {
                            "答案"
                        }
                        .into()
                    )
                );
            }
        }
        Ok(())
    }
}
