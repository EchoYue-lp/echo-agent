#![cfg(all(feature = "subagent", feature = "testing"))]

//! 在真实 readonly ReAct factory 和 dispatcher 之间核对 Run/attempt/guidance。
//! 这里只提供受控模型响应，不代替 EKO durable revision、GUI 或 SSH owner 验收。

use echo_agent::agent::{Agent, CancellationToken, ReactAgentBuilder};
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use echo_agent::subagent::{
    BackgroundSubagentHandle, DispatchRequest, ExecutionMode, FnAgentFactory, SubagentAccessMode,
    SubagentAttemptIdentity, SubagentDefinition, SubagentEvent, SubagentExecutor,
    SubagentExecutorConfig, SubagentRegistry, SubagentStatus,
};
use echo_agent::tasks::TaskClaim;
use echo_agent::testing::MockLlmClient;
use echo_agent::tools::ExternalRunContext;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

const SCOPE: &str = "same-conversation-control";
const TASK: &str = "fixed-plan-task";

#[derive(Default)]
struct Observations {
    factories: AtomicUsize,
    requests: Mutex<Vec<Vec<String>>>,
}

struct HeldModel {
    inner: MockLlmClient,
    observations: Arc<Observations>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl HeldModel {
    async fn observe_and_wait(&self, request: &ChatRequest) -> Result<()> {
        self.observations
            .requests
            .lock()
            .map_err(|error| ReactError::Other(error.to_string()))?
            .push(
                request
                    .messages
                    .iter()
                    .filter_map(|message| message.text_content())
                    .collect(),
            );
        self.entered.add_permits(1);
        self.release
            .acquire()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?
            .forget();
        Ok(())
    }
}

impl LlmClient for HeldModel {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            self.observe_and_wait(&request).await?;
            self.inner.chat(request).await
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            self.observe_and_wait(&request).await?;
            self.inner.chat_stream(request).await
        })
    }

    fn model_name(&self) -> &str {
        "controlled-model"
    }
}

fn identity(run: &str) -> Result<SubagentAttemptIdentity> {
    // 两个 Run 复用同 task/revision/attempt/claim 文本；execution 仅由原 TaskClaim 投影。
    let claim = TaskClaim {
        claim_id: "fixed-claim".into(),
        revision: 1,
        attempt: 1,
        spec_hash: "fixed-spec".into(),
    };
    SubagentAttemptIdentity::for_runtime_scope(
        SCOPE,
        run,
        TASK,
        claim.execution_id(run, TASK),
        claim.attempt,
    )
    .map_err(|error| ReactError::Other(error.to_string()))
}

fn request(run: &str, mode: ExecutionMode) -> DispatchRequest {
    DispatchRequest {
        agent_name: "readonly-scope-probe".into(),
        task: format!("inspect without mutation; original-request:{run}"),
        mode_override: Some(mode),
        cancel: CancellationToken::new(),
        parent_agent: "shared-primary".into(),
        parent_context: None,
        delegation_policy: DispatchRequest::policy_from_depth(0),
        runtime_context: Some(ExternalRunContext {
            conversation_id: Some("same-conversation".into()),
            run_id: Some(run.into()),
            ..Default::default()
        }),
        message: None,
        prompt_payload: None,
        prompt_context: None,
        constraints: Vec::new(),
        background: false,
    }
}

async fn harness() -> Result<(
    Arc<SubagentRegistry>,
    SubagentExecutor,
    Arc<Observations>,
    Arc<Semaphore>,
    Arc<Semaphore>,
)> {
    let registry = Arc::new(SubagentRegistry::new());
    let observations = Arc::new(Observations::default());
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let mut definition = SubagentDefinition::new("readonly-scope-probe", "只读身份探针");
    definition.access_mode = SubagentAccessMode::ReadOnly;
    registry
        .register_factory(
            definition,
            Arc::new(FnAgentFactory::new({
                let observations = observations.clone();
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    observations.factories.fetch_add(1, Ordering::SeqCst);
                    let client = Arc::new(HeldModel {
                        inner: MockLlmClient::new().with_response("## Summary\n只读检查完成"),
                        observations: observations.clone(),
                        entered: entered.clone(),
                        release: release.clone(),
                    });
                    Box::pin(async move {
                        let agent = ReactAgentBuilder::new()
                            .name("readonly-scope-probe")
                            .model("controlled-model")
                            .readonly_tools()
                            .llm_client(client)
                            .build()?;
                        Ok(Box::new(agent) as Box<dyn Agent>)
                    })
                }
            })),
        )
        .await;
    let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
    Ok((registry, executor, observations, entered, release))
}

async fn join_original(handle: &BackgroundSubagentHandle, release: &Semaphore) -> Result<()> {
    let original_join = handle.join();
    futures::pin_mut!(original_join);
    let (crossed_deadline, settled) =
        match tokio::time::timeout(Duration::from_secs(5), original_join.as_mut()).await {
            Ok(settled) => (false, settled),
            Err(_) => {
                // 不丢弃已取走底层JoinHandle的原future。释放本fixture屏障后继续同一等待。
                handle.cancel();
                release.add_permits(8);
                (true, original_join.await)
            }
        };
    let consumed_once = handle.join().await.is_err();
    if crossed_deadline {
        return Err(ReactError::Other(
            "original Subagent settled only after its unchanged deadline".into(),
        ));
    }
    if !consumed_once {
        return Err(ReactError::Other(
            "background result was consumed twice".into(),
        ));
    }
    let result = settled?;
    if result.outcome.status != SubagentStatus::Completed {
        return Err(ReactError::Other(format!(
            "original source returned {:?} instead of Completed",
            result.outcome.status
        )));
    }
    Ok(())
}

#[tokio::test]
async fn shared_scope_active_runs_keep_guidance_and_reject_same_run_reentry() -> Result<()> {
    for mode in [
        ExecutionMode::Sync,
        ExecutionMode::Fork,
        ExecutionMode::Teammate,
    ] {
        let (registry, executor, observations, entered, release) = harness().await?;
        let mut events = registry.event_bus().subscribe();
        for (run, instruction) in [("run-a", "仅处理甲的指导"), ("run-b", "仅处理乙的指导")]
        {
            executor
                .queue_guidance_scoped(SCOPE, run, TASK, 1, instruction)
                .map_err(|error| ReactError::Other(error.to_string()))?;
        }
        let first = executor
            .dispatch_background_attempt(request("run-a", mode.clone()), identity("run-a")?)
            .await?;
        let second = executor
            .dispatch_background_attempt(request("run-b", mode.clone()), identity("run-b")?)
            .await;
        // 即使回归红导致第二个 Run 未准入，也先释放/结束已存在的原 owner。
        let entered_result = if second.is_ok() {
            tokio::time::timeout(Duration::from_secs(2), entered.acquire_many(2))
                .await
                .map_err(|_| ReactError::Other("two original model requests did not enter".into()))
                .and_then(|permit| permit.map_err(|error| ReactError::Other(error.to_string())))
                .map(|permit| permit.forget())
        } else {
            Err(ReactError::Other(
                "independent Run was refused before model entry".into(),
            ))
        };
        let mut duplicate = identity("run-a")?;
        duplicate.execution_id = "replacement-execution-for-same-attempt".into();
        let reentry = executor
            .dispatch_background_attempt(request("run-a", mode.clone()), duplicate.clone())
            .await;
        let rejected = reentry.is_err();
        let mut foreign_scope = identity("run-a")?;
        foreign_scope.control_scope_id = Some("foreign-control-scope".into());
        let scope_rejected = executor.request_interrupt(foreign_scope).is_err();
        let mut foreign_run = identity("run-a")?;
        foreign_run.run_id = Some("foreign-run".into());
        let run_rejected = executor.request_interrupt(foreign_run).is_err();
        let factory_count_before_release = observations.factories.load(Ordering::SeqCst);
        let active = registry.control_registry().active_snapshot(8);
        release.add_permits(8);
        let first_result = join_original(&first, &release).await;
        let second_result = match second {
            Ok(handle) => join_original(&handle, &release).await,
            Err(error) => Err(error),
        };
        if let Ok(unexpected) = reentry {
            // 负控若意外执行，也真实 join；不遗留后台 owner 来掩盖红。
            join_original(&unexpected, &release).await?;
        }
        entered_result?;
        first_result?;
        second_result?;
        assert!(
            rejected,
            "same Run/attempt cannot acquire another execution"
        );
        assert!(
            scope_rejected && run_rejected,
            "foreign scope/Run cannot interrupt the admitted execution"
        );
        assert_eq!(
            factory_count_before_release, 2,
            "refusal must precede factory/model"
        );
        assert_eq!(active.len(), 2);
        assert!(registry.control_registry().active_snapshot(8).is_empty());
        let completed_retry = executor
            .dispatch_background_attempt(request("run-a", mode), duplicate)
            .await;
        let settled_rejected = completed_retry.is_err();
        if let Ok(unexpected) = completed_retry {
            join_original(&unexpected, &release).await?;
        }
        assert!(
            settled_rejected,
            "settled same Run/attempt remains rejected"
        );
        assert_eq!(observations.factories.load(Ordering::SeqCst), 2);

        let requests = observations
            .requests
            .lock()
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(requests.len(), 2);
        for (run, instruction, foreign) in [
            ("run-a", "仅处理甲的指导", "仅处理乙的指导"),
            ("run-b", "仅处理乙的指导", "仅处理甲的指导"),
        ] {
            // marker只标识原测试输入；不参与runtime准入，不改变固定task/claim。
            let marker = format!("original-request:{run}");
            let owned = requests
                .iter()
                .filter(|messages| messages.iter().any(|text| text.contains(&marker)))
                .collect::<Vec<_>>();
            assert_eq!(owned.len(), 1, "each Run owns one original model request");
            for messages in owned {
                let text = messages.join("\n");
                assert_eq!(text.matches(instruction).count(), 1);
                assert!(!text.contains(foreign), "guidance crossed the original Run");
            }
        }
        let mut completed = Vec::new();
        let mut started = Vec::new();
        while let Ok(event) = events.try_recv() {
            match event.as_ref() {
                SubagentEvent::DispatchStarted {
                    run_id,
                    execution_id,
                    conversation_id,
                    ..
                } => {
                    assert_eq!(conversation_id.as_deref(), Some("same-conversation"));
                    started.push((run_id.clone(), execution_id.clone()));
                }
                SubagentEvent::DispatchCompleted {
                    run_id,
                    execution_id,
                    ..
                } => {
                    completed.push((run_id.clone(), execution_id.clone()));
                }
                _ => {}
            }
        }
        let mut expected = ["run-a", "run-b"]
            .into_iter()
            .map(|run| identity(run).map(|id| (Some(run.to_string()), Some(id.execution_id))))
            .collect::<Result<Vec<_>>>()?;
        started.sort();
        completed.sort();
        expected.sort();
        assert_eq!(started, expected);
        assert_eq!(completed, expected);
    }
    Ok(())
}

#[tokio::test]
async fn mismatched_execution_owner_preserves_guidance_without_constructing_a_factory() -> Result<()>
{
    let (_registry, executor, observations, _entered, release) = harness().await?;
    executor
        .queue_guidance_scoped(SCOPE, "run-a", TASK, 1, "原 Run 的一次指导")
        .map_err(|error| ReactError::Other(error.to_string()))?;
    // 只验证现有execution绑定合同；durable Run/revision仍由原应用权威校验。
    let mut wrong_execution = request("run-a", ExecutionMode::Sync);
    if let Some(runtime) = wrong_execution.runtime_context.as_mut() {
        runtime.execution_id = Some("foreign-execution-owner".into());
    }
    // 即使负控意外准入，也先真实结束原owner，再报告拒绝断言。
    let refused = executor
        .dispatch_background_attempt(wrong_execution, identity("run-a")?)
        .await;
    let rejected = refused.is_err();
    let unexpected_cleanup = match refused {
        Ok(handle) => {
            release.add_permits(8);
            join_original(&handle, &release).await
        }
        Err(_) => Ok(()),
    };
    assert!(
        rejected,
        "mismatched execution owner was admitted; original source cleanup: {unexpected_cleanup:?}"
    );
    unexpected_cleanup?;
    assert_eq!(observations.factories.load(Ordering::SeqCst), 0);
    assert!(
        observations
            .requests
            .lock()
            .map_err(|error| ReactError::Other(error.to_string()))?
            .is_empty()
    );
    release.add_permits(1);
    let result = executor
        .dispatch_attempt(request("run-a", ExecutionMode::Sync), identity("run-a")?)
        .await?;
    assert_eq!(result.outcome.status, SubagentStatus::Completed);
    assert_eq!(observations.factories.load(Ordering::SeqCst), 1);
    let requests = observations
        .requests
        .lock()
        .map_err(|error| ReactError::Other(error.to_string()))?;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests
            .iter()
            .flatten()
            .map(|text| text.matches("原 Run 的一次指导").count())
            .sum::<usize>(),
        1
    );
    Ok(())
}
