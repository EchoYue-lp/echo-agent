//! Deterministic checks at the actual dispatch/factory boundary, without a model service.

use super::{
    AgentFactory, DispatchRequest, ExecutionMode, FnAgentFactory, SubagentBuilder,
    SubagentDefinition, SubagentExecutor, SubagentExecutorConfig, SubagentHookContext,
    SubagentHookRegistry, SubagentHooks, SubagentRegistration, SubagentRegistrationAdmission,
    SubagentRegistry, SubagentResult, SubagentRetryDecision, SubagentStatus, TeamConfig, TeamSpec,
    TeamStrategy,
};
use crate::error::{ReactError, Result};
use crate::testing::MockAgent;
use echo_core::agent::{Agent, CancellationToken};
use echo_core::tools::{ExternalRunContext, InvocationResourceGuard};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

fn failed(message: impl Into<String>) -> ReactError {
    ReactError::Other(message.into())
}

fn request(mode: ExecutionMode) -> DispatchRequest {
    DispatchRequest {
        agent_name: "role".to_string(),
        task: "observe the selected executable".to_string(),
        mode_override: Some(mode),
        cancel: CancellationToken::new(),
        parent_agent: "parent".to_string(),
        parent_context: None,
        delegation_policy: DispatchRequest::policy_from_depth(0),
        runtime_context: Some(ExternalRunContext {
            run_id: Some("bound-run".to_string()),
            resource_guards: vec![InvocationResourceGuard::new_identified((), "actual-lease")],
            ..ExternalRunContext::default()
        }),
        message: None,
        prompt_payload: None,
        prompt_context: None,
        constraints: Vec::new(),
        background: false,
    }
}

fn counting_factory(count: Arc<AtomicUsize>, response: &'static str) -> Arc<dyn AgentFactory> {
    Arc::new(FnAgentFactory::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(Box::new(MockAgent::new("role").with_response(response)) as Box<dyn Agent>)
        })
    }))
}

async fn acquire(signal: &Semaphore) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), signal.acquire())
        .await
        .map_err(|error| failed(error.to_string()))?
        .map_err(|error| failed(error.to_string()))?
        .forget();
    Ok(())
}

struct HeldAdmission {
    reached: Semaphore,
    resume: Semaphore,
    descriptions: Mutex<Vec<String>>,
}

impl HeldAdmission {
    fn new() -> Self {
        Self {
            reached: Semaphore::new(0),
            resume: Semaphore::new(0),
            descriptions: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl SubagentRegistrationAdmission for HeldAdmission {
    async fn admit(
        &self,
        registration: &SubagentRegistration,
        request: &DispatchRequest,
    ) -> Result<()> {
        let runtime = request
            .runtime_context
            .as_ref()
            .ok_or_else(|| failed("missing runtime"))?;
        if runtime.run_id.as_deref() != Some("bound-run")
            || !runtime
                .resource_guards
                .iter()
                .any(|guard| guard.matches_identity(&"actual-lease"))
        {
            return Err(failed(
                "policy lost actual invocation identity/resource guard",
            ));
        }
        self.descriptions
            .lock()
            .map_err(|error| failed(error.to_string()))?
            .push(registration.definition().description.clone());
        self.reached.add_permits(1);
        acquire(&self.resume).await
    }
}

#[derive(Clone, Copy)]
enum Entry {
    Normal,
    Background,
    TeammateHandle,
}

async fn execute(
    executor: &SubagentExecutor,
    request: DispatchRequest,
    entry: Entry,
) -> Result<SubagentResult> {
    match entry {
        Entry::Normal => executor.dispatch(request).await,
        Entry::Background => executor.dispatch_background(request).await?.join().await,
        Entry::TeammateHandle => executor.dispatch_teammate(request).await?.join().await,
    }
}

#[tokio::test]
async fn policy_wait_keeps_definition_factory_and_record_together_across_replace_or_remove()
-> Result<()> {
    for entry in [Entry::Normal, Entry::Background, Entry::TeammateHandle] {
        for mode in [
            ExecutionMode::Sync,
            ExecutionMode::Fork,
            ExecutionMode::Teammate,
        ] {
            for replace in [false, true] {
                let registry = Arc::new(SubagentRegistry::new());
                let old_count = Arc::new(AtomicUsize::new(0));
                let new_count = Arc::new(AtomicUsize::new(0));
                registry
                    .register_factory(
                        SubagentDefinition::new("role", "original definition"),
                        counting_factory(old_count.clone(), "original executable"),
                    )
                    .await;
                let selected = registry
                    .resolve_registration("role")
                    .await
                    .ok_or_else(|| failed("missing original record"))?;
                let policy = Arc::new(HeldAdmission::new());
                registry
                    .set_registration_admission(Some(policy.clone()))
                    .await;
                let executor = Arc::new(SubagentExecutor::new(
                    registry.clone(),
                    SubagentExecutorConfig::default(),
                ));
                let running = tokio::spawn({
                    let executor = executor.clone();
                    let mode = mode.clone();
                    async move { execute(&executor, request(mode), entry).await }
                });
                acquire(&policy.reached).await?;
                assert_eq!(
                    old_count.load(Ordering::SeqCst),
                    0,
                    "factory ran before policy"
                );
                // Finishing this mutation while the callback is blocked also
                // proves that dispatch holds no global registry lock over await.
                tokio::time::timeout(Duration::from_secs(3), async {
                    registry.remove("role").await;
                    if replace {
                        registry
                            .register_factory(
                                SubagentDefinition::new("role", "replacement definition"),
                                counting_factory(new_count.clone(), "replacement executable"),
                            )
                            .await;
                    }
                })
                .await
                .map_err(|error| failed(error.to_string()))?;
                if let Some(current) = registry.resolve_registration("role").await {
                    assert!(!selected.is_same_registration(&current));
                }
                policy.resume.add_permits(1);
                let result = running.await.map_err(|error| failed(error.to_string()))??;
                assert_eq!(result.output, "original executable");
                assert_eq!(old_count.load(Ordering::SeqCst), 1);
                assert_eq!(new_count.load(Ordering::SeqCst), 0);
                assert_eq!(
                    *policy
                        .descriptions
                        .lock()
                        .map_err(|error| failed(error.to_string()))?,
                    vec!["original definition"]
                );
                if replace {
                    // A fresh old result must not become the new lazy cache.
                    assert!(
                        !registry
                            .get("role")
                            .await
                            .ok_or_else(|| failed("missing replacement"))?
                            .has_instance
                    );
                    let _ = registry
                        .get_agent("role")
                        .await
                        .ok_or_else(|| failed("replacement cache failed"))?;
                    assert_eq!(new_count.load(Ordering::SeqCst), 1);
                } else {
                    assert!(!registry.contains("role").await);
                }
            }
        }
    }
    Ok(())
}

struct RejectAdmission(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl SubagentRegistrationAdmission for RejectAdmission {
    async fn admit(&self, registration: &SubagentRegistration, _: &DispatchRequest) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(failed(format!(
            "host refused actual {}",
            registration.definition().description
        )))
    }
}

#[tokio::test]
async fn refusal_is_returned_before_lazy_fresh_or_background_factory_side_effects() -> Result<()> {
    for entry in [Entry::Normal, Entry::Background, Entry::TeammateHandle] {
        for mode in [
            ExecutionMode::Sync,
            ExecutionMode::Fork,
            ExecutionMode::Teammate,
        ] {
            let registry = Arc::new(SubagentRegistry::new());
            let count = Arc::new(AtomicUsize::new(0));
            let admitted = Arc::new(AtomicUsize::new(0));
            registry
                .register_factory(
                    SubagentDefinition::new("role", "blocked record"),
                    counting_factory(count.clone(), "must not run"),
                )
                .await;
            registry
                .set_registration_admission(Some(Arc::new(RejectAdmission(admitted.clone()))))
                .await;
            let executor =
                SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
            let error = execute(&executor, request(mode), entry)
                .await
                .err()
                .ok_or_else(|| failed("refused dispatch succeeded"))?;
            assert!(
                error
                    .to_string()
                    .contains("host refused actual blocked record")
            );
            assert_eq!(admitted.load(Ordering::SeqCst), 1);
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(
                !registry
                    .get("role")
                    .await
                    .ok_or_else(|| failed("lost blocked record"))?
                    .has_instance
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn registered_entry_rejects_foreign_registry_even_with_same_name_and_revision() -> Result<()>
{
    let first = SubagentRegistry::new();
    let second = Arc::new(SubagentRegistry::new());
    let count = Arc::new(AtomicUsize::new(0));
    first
        .register_factory(
            SubagentDefinition::new("role", "foreign"),
            counting_factory(count.clone(), "must not run"),
        )
        .await;
    second
        .register(
            SubagentDefinition::new("role", "local"),
            Box::new(MockAgent::new("role")),
        )
        .await;
    let selected = first
        .resolve_registration("role")
        .await
        .ok_or_else(|| failed("missing foreign"))?;
    let local = second
        .resolve_registration("role")
        .await
        .ok_or_else(|| failed("missing local"))?;
    assert_eq!(selected.revision(), local.revision());
    assert!(!selected.is_same_registration(&local));
    let executor = SubagentExecutor::new(second, SubagentExecutorConfig::default());
    for background in [false, true] {
        let error = if background {
            executor
                .dispatch_background_registered(request(ExecutionMode::Sync), selected.clone())
                .await
                .err()
        } else {
            executor
                .dispatch_registered(request(ExecutionMode::Sync), selected.clone())
                .await
                .err()
        }
        .ok_or_else(|| failed("foreign record was accepted"))?;
        assert!(error.to_string().contains("registration does not belong"));
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn cancellation_during_policy_wait_never_constructs_the_selected_agent() -> Result<()> {
    for entry in [Entry::Normal, Entry::Background, Entry::TeammateHandle] {
        let registry = Arc::new(SubagentRegistry::new());
        let count = Arc::new(AtomicUsize::new(0));
        registry
            .register_factory(
                SubagentDefinition::new("role", "original"),
                counting_factory(count.clone(), "must not run"),
            )
            .await;
        let policy = Arc::new(HeldAdmission::new());
        registry
            .set_registration_admission(Some(policy.clone()))
            .await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig::default(),
        ));
        let req = request(ExecutionMode::Sync);
        let cancel = req.cancel.clone();
        let running = tokio::spawn(async move { execute(&executor, req, entry).await });
        acquire(&policy.reached).await?;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .map_err(|error| failed(error.to_string()))?
            .map_err(|error| failed(error.to_string()))?;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn replacement_while_original_factory_awaits_does_not_publish_into_new_record() -> Result<()>
{
    let registry = Arc::new(SubagentRegistry::new());
    let reached = Arc::new(Semaphore::new(0));
    let resume = Arc::new(Semaphore::new(0));
    let old = Arc::new(FnAgentFactory::new({
        let reached = reached.clone();
        let resume = resume.clone();
        move || {
            let reached = reached.clone();
            let resume = resume.clone();
            Box::pin(async move {
                reached.add_permits(1);
                acquire(&resume).await?;
                Ok(Box::new(MockAgent::new("role").with_response("old factory")) as Box<dyn Agent>)
            })
        }
    }));
    registry
        .register_factory(SubagentDefinition::new("role", "old definition"), old)
        .await;
    let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
    let running =
        tokio::spawn(async move { executor.dispatch(request(ExecutionMode::Sync)).await });
    acquire(&reached).await?;
    let new_count = Arc::new(AtomicUsize::new(0));
    registry
        .register_factory(
            SubagentDefinition::new("role", "new definition"),
            counting_factory(new_count.clone(), "new factory"),
        )
        .await;
    resume.add_permits(1);
    assert_eq!(
        running
            .await
            .map_err(|error| failed(error.to_string()))??
            .output,
        "old factory"
    );
    assert!(
        !registry
            .get("role")
            .await
            .ok_or_else(|| failed("missing new registration"))?
            .has_instance
    );
    let _ = registry
        .get_agent("role")
        .await
        .ok_or_else(|| failed("new cache failed"))?;
    assert_eq!(new_count.load(Ordering::SeqCst), 1);
    Ok(())
}

struct RetargetAfterFailure {
    registry: Arc<SubagentRegistry>,
    next_count: Arc<AtomicUsize>,
    delegate: bool,
}

#[async_trait::async_trait]
impl SubagentHooks for RetargetAfterFailure {
    async fn on_failure(&self, _: &SubagentHookContext, _: &str) -> SubagentRetryDecision {
        let target = if self.delegate { "alternative" } else { "role" };
        self.registry
            .register_factory(
                SubagentDefinition::new(target, "replacement must be checked"),
                counting_factory(self.next_count.clone(), "must not run"),
            )
            .await;
        if self.delegate {
            SubagentRetryDecision::Delegate {
                alternative_agent: target.to_string(),
            }
        } else {
            SubagentRetryDecision::Retry { delay_secs: 0 }
        }
    }
}

struct FirstRecordOnly(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl SubagentRegistrationAdmission for FirstRecordOnly {
    async fn admit(&self, registration: &SubagentRegistration, _: &DispatchRequest) -> Result<()> {
        let description = registration.definition().description.clone();
        self.0
            .lock()
            .map_err(|error| failed(error.to_string()))?
            .push(description.clone());
        if description == "first allowed" {
            Ok(())
        } else {
            Err(failed("replacement not admitted"))
        }
    }
}

#[tokio::test]
async fn retry_and_retarget_admit_the_new_actual_record_before_its_factory() -> Result<()> {
    for delegate in [false, true] {
        let registry = Arc::new(SubagentRegistry::new());
        registry
            .register_factory(
                SubagentDefinition::new("role", "first allowed"),
                Arc::new(FnAgentFactory::new(|| {
                    Box::pin(async { Err(failed("first factory failed")) })
                })),
            )
            .await;
        let policy = Arc::new(FirstRecordOnly(Mutex::new(Vec::new())));
        registry
            .set_registration_admission(Some(policy.clone()))
            .await;
        let next_count = Arc::new(AtomicUsize::new(0));
        let mut hooks = SubagentHookRegistry::new();
        hooks.register(Arc::new(RetargetAfterFailure {
            registry: registry.clone(),
            next_count: next_count.clone(),
            delegate,
        }));
        let executor =
            SubagentExecutor::with_hooks(registry, SubagentExecutorConfig::default(), hooks);
        let error = executor
            .dispatch(request(ExecutionMode::Sync))
            .await
            .err()
            .ok_or_else(|| failed("new record escaped admission"))?;
        assert!(error.to_string().contains("replacement not admitted"));
        assert_eq!(next_count.load(Ordering::SeqCst), 0);
        assert_eq!(
            *policy.0.lock().map_err(|error| failed(error.to_string()))?,
            vec!["first allowed", "replacement must be checked"]
        );
    }
    Ok(())
}

#[tokio::test]
async fn team_member_is_checked_by_the_same_policy_without_constructing_a_refused_member()
-> Result<()> {
    let registry = Arc::new(SubagentRegistry::new());
    let count = Arc::new(AtomicUsize::new(0));
    registry
        .register_factory(
            SubagentDefinition::new("role", "blocked member"),
            counting_factory(count.clone(), "must not run"),
        )
        .await;
    let team = SubagentBuilder::new("team")
        .description("first allowed")
        .team(TeamSpec {
            strategy: TeamStrategy::Pipeline(vec!["role".to_string()]),
            manager: String::new(),
            subagents: Vec::new(),
            config: TeamConfig::default(),
        })
        .build();
    registry
        .register(team, Box::new(MockAgent::new("team")))
        .await;
    let policy = Arc::new(FirstRecordOnly(Mutex::new(Vec::new())));
    registry
        .set_registration_admission(Some(policy.clone()))
        .await;
    let executor = SubagentExecutor::new(registry, SubagentExecutorConfig::default());
    let mut req = request(ExecutionMode::Team);
    req.agent_name = "team".to_string();
    let result = executor.dispatch(req).await;
    assert!(match result {
        Ok(result) => result.outcome.status != SubagentStatus::Completed,
        Err(_) => true,
    });
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        *policy.0.lock().map_err(|error| failed(error.to_string()))?,
        vec!["first allowed", "blocked member"]
    );
    Ok(())
}

#[tokio::test]
async fn no_policy_preserves_general_dynamic_registration_and_fresh_dispatch() -> Result<()> {
    let registry = Arc::new(SubagentRegistry::new());
    let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
    for response in ["first", "replacement"] {
        let count = Arc::new(AtomicUsize::new(0));
        registry
            .register_factory(
                SubagentDefinition::new("role", "dynamic General role"),
                counting_factory(count.clone(), response),
            )
            .await;
        for _ in 0..2 {
            assert_eq!(
                executor
                    .dispatch(request(ExecutionMode::Sync))
                    .await?
                    .output,
                response
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
    Ok(())
}

#[tokio::test]
async fn refused_controlled_background_settles_the_original_reservation_without_a_factory()
-> Result<()> {
    let registry = Arc::new(SubagentRegistry::new());
    let count = Arc::new(AtomicUsize::new(0));
    let admitted = Arc::new(AtomicUsize::new(0));
    registry
        .register_factory(
            SubagentDefinition::new("role", "blocked exact attempt"),
            counting_factory(count.clone(), "must not run"),
        )
        .await;
    registry
        .set_registration_admission(Some(Arc::new(RejectAdmission(admitted.clone()))))
        .await;
    let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
    let identity =
        super::SubagentAttemptIdentity::for_run("bound-run", "task", "exact-execution", 1)
            .map_err(|error| failed(error.to_string()))?;
    let error = executor
        .dispatch_background_attempt(request(ExecutionMode::Sync), identity)
        .await
        .err()
        .ok_or_else(|| failed("refused controlled background returned a handle"))?;
    assert!(error.to_string().contains("host refused actual"));
    assert_eq!(admitted.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(registry.control_registry().active_snapshot(16).is_empty());
    Ok(())
}

#[tokio::test]
async fn actual_agent_tool_keeps_the_admitted_record_for_foreground_and_background() -> Result<()> {
    use crate::tools::Tool;
    use crate::tools::builtin::agent_dispatch::AgentDispatchTool;

    for background in [false, true] {
        let registry = Arc::new(SubagentRegistry::new());
        let old_count = Arc::new(AtomicUsize::new(0));
        let new_count = Arc::new(AtomicUsize::new(0));
        registry
            .register_factory(
                SubagentDefinition::new("role", "tool original"),
                counting_factory(old_count.clone(), "original executable"),
            )
            .await;
        let policy = Arc::new(HeldAdmission::new());
        registry
            .set_registration_admission(Some(policy.clone()))
            .await;
        let executor = Arc::new(SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig::default(),
        ));
        let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
        let mut events = registry.event_bus().subscribe();
        let running = tokio::spawn(async move {
            let parameters = [
                ("agent_name".to_string(), serde_json::json!("role")),
                (
                    "task".to_string(),
                    serde_json::json!("use the admitted role"),
                ),
                ("background".to_string(), serde_json::json!(background)),
            ]
            .into_iter()
            .collect();
            tool.execute_with_context(
                parameters,
                &echo_core::tools::ToolContext {
                    run_id: Some("bound-run".to_string()),
                    resource_guards: vec![InvocationResourceGuard::new_identified(
                        (),
                        "actual-lease",
                    )],
                    ..echo_core::tools::ToolContext::default()
                },
            )
            .await
        });
        acquire(&policy.reached).await?;
        registry
            .register_factory(
                SubagentDefinition::new("role", "tool replacement"),
                counting_factory(new_count.clone(), "must not run"),
            )
            .await;
        policy.resume.add_permits(1);
        let result = running.await.map_err(|error| failed(error.to_string()))??;
        assert!(
            result.success,
            "actual agent_tool rejected its original record: {}",
            result.output
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = events.recv().await.map_err(|error| failed(error.to_string()))?;
                if matches!(event.as_ref(), super::SubagentEvent::DispatchCompleted { agent, .. } if agent == "role") {
                    return Ok::<(), ReactError>(());
                }
            }
        }).await.map_err(|error| failed(error.to_string()))??;
        assert_eq!(old_count.load(Ordering::SeqCst), 1);
        assert_eq!(new_count.load(Ordering::SeqCst), 0);
        assert_eq!(
            *policy
                .descriptions
                .lock()
                .map_err(|error| failed(error.to_string()))?,
            vec!["tool original"]
        );
    }
    Ok(())
}
