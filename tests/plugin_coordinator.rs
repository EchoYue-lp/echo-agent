use echo_agent::agent::ReactAgentBuilder;
use echo_agent::plugin::{
    AGENT_PLUGIN_SCHEMA_V1, InstallSource, PluginCoordinator, PluginCoordinatorError,
    PluginIntegrator, PluginLifecycle, PluginOperationKind, PluginOperationPhase,
    PluginOperationReceipt, PluginRegistry, PluginRuntimeStatus, PluginScope,
};
use echo_agent::skills::hooks::{HookEvent, HookResult};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn create_plugin(
    sources: &Path,
    name: &str,
    skill_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    create_plugin_with_dependencies(sources, name, skill_name, serde_json::json!([]))
}

fn create_plugin_with_dependencies(
    sources: &Path,
    name: &str,
    skill_name: &str,
    dependencies: serde_json::Value,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = sources.join(name);
    std::fs::create_dir_all(root.join(format!("skills/{skill_name}")))?;
    std::fs::create_dir_all(root.join("hooks"))?;
    std::fs::write(
        root.join(format!("skills/{skill_name}/SKILL.md")),
        format!("---\nname: {skill_name}\ndescription: {name} skill\n---\nUse {name}.\n"),
    )?;
    std::fs::write(root.join("hooks/hooks.yaml"), "{}\n")?;
    std::fs::write(
        root.join("plugin.json"),
        serde_json::to_vec(&serde_json::json!({
            "$schema": AGENT_PLUGIN_SCHEMA_V1,
            "name": name,
            "version": "1.0.0",
            "description": format!("{name} test plugin"),
            "defaultEnabled": true,
            "dependencies": dependencies
        }))?,
    )?;
    Ok(root)
}

fn registry(root: &Path) -> PluginRegistry {
    PluginRegistry::with_paths(
        root.join("registry.json"),
        root.join("data"),
        Some(root.to_path_buf()),
    )
}

#[derive(Default)]
struct LifecycleCounts {
    init: AtomicUsize,
    activate: AtomicUsize,
    deactivate: AtomicUsize,
    shutdown: AtomicUsize,
    fail_deactivate: AtomicBool,
    fail_activate_once: AtomicBool,
}

struct CountingLifecycle(Arc<LifecycleCounts>);

struct OrderedLifecycle {
    plugin_id: String,
    events: Arc<Mutex<Vec<String>>>,
}

impl OrderedLifecycle {
    fn record(&self, phase: &str) {
        if let Ok(mut events) = self.events.lock() {
            events.push(format!("{phase}:{}", self.plugin_id));
        }
    }
}

impl PluginLifecycle for OrderedLifecycle {
    fn init(&self) -> Result<(), String> {
        self.record("init");
        Ok(())
    }

    fn activate(&self) -> Result<(), String> {
        self.record("activate");
        Ok(())
    }

    fn deactivate(&self) -> Result<(), String> {
        self.record("deactivate");
        Ok(())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.record("shutdown");
        Ok(())
    }
}

impl PluginLifecycle for CountingLifecycle {
    fn init(&self) -> Result<(), String> {
        self.0.init.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn activate(&self) -> Result<(), String> {
        self.0.activate.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_activate_once.swap(false, Ordering::SeqCst) {
            Err("injected coordinator activation failure".to_string())
        } else {
            Ok(())
        }
    }

    fn deactivate(&self) -> Result<(), String> {
        self.0.deactivate.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_deactivate.load(Ordering::SeqCst) {
            Err("injected coordinator deactivation failure".to_string())
        } else {
            Ok(())
        }
    }

    fn shutdown(&self) -> Result<(), String> {
        self.0.shutdown.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn coordinator_is_available_from_the_public_plugin_facade() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let registry = PluginRegistry::with_paths(
        temporary.path().join("registry.json"),
        temporary.path().join("data"),
        Some(temporary.path().to_path_buf()),
    );
    let coordinator = PluginCoordinator::new(registry, PluginIntegrator::new());
    assert!(coordinator.last_receipt().is_none());
    Ok(())
}

#[tokio::test]
async fn lifecycle_operations_converge_in_order_and_restart_from_durable_intent()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let first = create_plugin(&sources, "plugin-a", "skill-a")?;
    let second = create_plugin(&sources, "plugin-b", "skill-b")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(first), PluginScope::Local)?;
    plugin_registry.install(&InstallSource::Local(second), PluginScope::Local)?;

    let first_counts = Arc::new(LifecycleCounts::default());
    let second_counts = Arc::new(LifecycleCounts::default());
    let events = Arc::new(Mutex::new(Vec::<(HookEvent, String)>::new()));
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    coordinator.register_lifecycle(
        "plugin-a",
        Arc::new(CountingLifecycle(Arc::clone(&first_counts))),
    )?;
    coordinator.register_lifecycle(
        "plugin-b",
        Arc::new(CountingLifecycle(Arc::clone(&second_counts))),
    )?;
    let mut agent = ReactAgentBuilder::new().model("coordinator-test").build()?;
    let event_sink = Arc::clone(&events);
    agent.hook_registry().write().await.set_programmatic_hook(
        "record-plugin-lifecycle",
        &[HookEvent::PluginLoaded, HookEvent::PluginDisabled],
        Arc::new(move |context| {
            let event_sink = Arc::clone(&event_sink);
            Box::pin(async move {
                if let (Some(plugin_id), Ok(mut events)) = (context.matcher, event_sink.lock()) {
                    events.push((context.event, plugin_id));
                }
                HookResult::default()
            })
        }),
    );

    let first_receipt = coordinator.reconcile(&mut agent).await?;
    assert_eq!(first_receipt.status(), PluginRuntimeStatus::Converged);
    assert_eq!(first_receipt.phase(), PluginOperationPhase::Committed);
    assert_eq!(
        first_receipt.loaded_event_attempts(),
        ["plugin-a", "plugin-b"]
    );
    let first_generation = first_receipt
        .actual_generation()
        .ok_or_else(|| std::io::Error::other("first generation missing"))?;
    assert_eq!(first_counts.activate.load(Ordering::SeqCst), 1);
    assert_eq!(second_counts.activate.load(Ordering::SeqCst), 1);

    let unchanged = coordinator.reconcile(&mut agent).await?;
    assert_eq!(unchanged.actual_generation(), Some(first_generation));
    assert!(unchanged.loaded_event_attempts().is_empty());
    assert_eq!(first_counts.deactivate.load(Ordering::SeqCst), 0);
    assert_eq!(second_counts.deactivate.load(Ordering::SeqCst), 0);

    let reload = coordinator.reload(&mut agent).await?;
    assert!(
        reload
            .actual_generation()
            .is_some_and(|value| value > first_generation)
    );
    assert_eq!(reload.disabled_event_attempts(), ["plugin-b", "plugin-a"]);
    assert_eq!(reload.loaded_event_attempts(), ["plugin-a", "plugin-b"]);

    let disabled = coordinator.disable(&mut agent, "plugin-a").await?;
    assert_eq!(disabled.disabled_event_attempts(), ["plugin-a"]);
    assert_eq!(disabled.loaded_event_attempts(), ["plugin-b"]);
    assert!(
        !coordinator
            .registry()
            .get("plugin-a")
            .ok_or_else(|| std::io::Error::other("plugin-a missing after disable"))?
            .enabled
    );

    let enabled = coordinator.enable(&mut agent, "plugin-a").await?;
    assert_eq!(enabled.loaded_event_attempts(), ["plugin-a", "plugin-b"]);
    assert!(
        coordinator
            .registry()
            .get("plugin-a")
            .ok_or_else(|| std::io::Error::other("plugin-a missing after enable"))?
            .enabled
    );

    let uninstalled = coordinator.uninstall(&mut agent, "plugin-a", false).await?;
    assert_eq!(uninstalled.disabled_event_attempts(), ["plugin-a"]);
    assert!(coordinator.registry().get("plugin-a").is_none());
    assert_eq!(first_counts.shutdown.load(Ordering::SeqCst), 1);

    let desired_revision = coordinator.registry().revision();
    let shutdown = coordinator.shutdown(&mut agent).await?;
    assert_eq!(shutdown.desired_revision(), desired_revision);
    assert_eq!(shutdown.disabled_event_attempts(), ["plugin-b"]);
    assert_eq!(second_counts.shutdown.load(Ordering::SeqCst), 1);

    let persisted_registry = registry(temporary.path());
    let mut restarted = PluginCoordinator::new(persisted_registry, PluginIntegrator::new());
    let mut restarted_agent = ReactAgentBuilder::new()
        .model("coordinator-restart")
        .build()?;
    let restarted_receipt = restarted.startup(&mut restarted_agent).await?;
    assert_eq!(restarted_receipt.loaded_event_attempts(), ["plugin-b"]);
    assert!(
        restarted
            .registry()
            .get("plugin-b")
            .is_some_and(|entry| entry.enabled)
    );

    let events = events
        .lock()
        .map_err(|_| std::io::Error::other("event sink lock poisoned"))?;
    assert!(events.contains(&(HookEvent::PluginDisabled, "plugin-a".to_string())));
    assert!(events.contains(&(HookEvent::PluginLoaded, "plugin-b".to_string())));
    Ok(())
}

#[tokio::test]
async fn callback_debt_keeps_desired_committed_and_blocks_later_operations_until_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let counts = Arc::new(LifecycleCounts::default());
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    coordinator.register_lifecycle("plugin-a", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
    let mut agent = ReactAgentBuilder::new().model("coordinator-debt").build()?;
    coordinator.reconcile(&mut agent).await?;
    counts.fail_deactivate.store(true, Ordering::SeqCst);

    let failure = coordinator
        .disable(&mut agent, "plugin-a")
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("deactivation failure unexpectedly converged"))?;
    let pending = failure
        .receipt()
        .ok_or_else(|| std::io::Error::other("pending receipt missing"))?;
    assert_eq!(pending.status(), PluginRuntimeStatus::ActualPending);
    assert_eq!(pending.phase(), PluginOperationPhase::CallbackWithdrawal);
    assert!(
        !coordinator
            .registry()
            .get("plugin-a")
            .ok_or_else(|| std::io::Error::other("plugin-a missing"))?
            .enabled
    );
    assert!(matches!(
        coordinator.reload(&mut agent).await,
        Err(PluginCoordinatorError::OperationPending(_))
    ));

    counts.fail_deactivate.store(false, Ordering::SeqCst);
    let retried = coordinator.retry(&mut agent).await?;
    assert_eq!(retried.status(), PluginRuntimeStatus::Converged);
    assert_eq!(
        retried.kind(),
        &PluginOperationKind::Disable {
            plugin_id: "plugin-a".to_string()
        }
    );
    assert!(coordinator.pending_receipt().is_none());
    Ok(())
}

#[tokio::test]
async fn dropped_event_future_is_not_attempted_again_by_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-cancel")
        .build()?;
    coordinator.reconcile(&mut agent).await?;

    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let hook_attempts = Arc::clone(&attempts);
    let hook_started = Arc::clone(&started);
    let hook_release = Arc::clone(&release);
    agent.hook_registry().write().await.set_programmatic_hook(
        "block-disabled-event",
        &[HookEvent::PluginDisabled],
        Arc::new(move |_context| {
            let hook_attempts = Arc::clone(&hook_attempts);
            let hook_started = Arc::clone(&hook_started);
            let hook_release = Arc::clone(&hook_release);
            Box::pin(async move {
                hook_attempts.fetch_add(1, Ordering::SeqCst);
                hook_started.notify_one();
                hook_release.notified().await;
                HookResult::default()
            })
        }),
    );

    let mut disable = Box::pin(coordinator.disable(&mut agent, "plugin-a"));
    tokio::select! {
        _ = started.notified() => {}
        result = &mut disable => {
            return Err(std::io::Error::other(format!(
                "disable finished before cancellation point: {result:?}"
            )).into());
        }
    }
    drop(disable);
    release.notify_waiters();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let pending = coordinator
        .pending_receipt()
        .ok_or_else(|| std::io::Error::other("cancelled operation receipt missing"))?;
    assert_eq!(pending.status(), PluginRuntimeStatus::ActualPending);
    assert_eq!(pending.phase(), PluginOperationPhase::EventEmission);
    assert_eq!(pending.disabled_event_attempts(), ["plugin-a"]);

    let retried = coordinator.retry(&mut agent).await?;
    assert_eq!(retried.status(), PluginRuntimeStatus::Converged);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    Ok(())
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn dropped_publication_future_retains_actual_pending_phase_for_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    std::fs::write(
        source.join("mcp.json"),
        serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {
                "blocked": {
                    "type": "streamable-http",
                    "url": format!("http://{address}/mcp")
                }
            }
        })
        .to_string(),
    )?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let integrator = PluginIntegrator::new();
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-publication-cancel")
        .build()?;
    let target = integrator.publication_target(&agent);
    let mut coordinator = PluginCoordinator::new(plugin_registry, integrator);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        if let Ok((_connection, _)) = listener.accept().await {
            let _ = entered_tx.send(());
            std::future::pending::<()>().await;
        }
    });
    {
        let mut reconcile = Box::pin(coordinator.reconcile(&mut agent));
        tokio::select! {
            entered = entered_rx => {
                entered.map_err(|_| std::io::Error::other("publication await was not observed"))?;
            }
            result = &mut reconcile => {
                return Err(std::io::Error::other(format!(
                    "publication finished before cancellation point: {result:?}"
                )).into());
            }
        }
    }
    server.abort();
    let _ = server.await;

    let pending = coordinator
        .pending_receipt()
        .ok_or_else(|| std::io::Error::other("cancelled publication receipt missing"))?;
    assert_eq!(pending.status(), PluginRuntimeStatus::ActualPending);
    assert_eq!(pending.phase(), PluginOperationPhase::WiringPublication);
    let cleanup = target
        .pending_cleanup_receipt()
        .await
        .ok_or_else(|| std::io::Error::other("cancelled publication cleanup receipt missing"))?;
    assert!(
        cleanup
            .components_by_plugin
            .get("plugin-a")
            .is_some_and(|owned| !owned.mcp_server_ids.is_empty())
    );
    let retried = coordinator.retry(&mut agent).await?;
    assert_eq!(retried.status(), PluginRuntimeStatus::Converged);
    assert!(retried.actual_generation().is_some());
    Ok(())
}

#[tokio::test]
async fn invalid_preparation_preserves_old_actual_and_reprepares_on_same_operation_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let counts = Arc::new(LifecycleCounts::default());
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    coordinator.register_lifecycle("plugin-a", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-invalid-preparation")
        .build()?;
    let first = coordinator.reconcile(&mut agent).await?;
    let first_generation = first
        .actual_generation()
        .ok_or_else(|| std::io::Error::other("initial generation missing"))?;
    let data_path = coordinator.registry().data_dir_for("plugin-a");
    std::fs::remove_dir_all(&data_path)?;
    std::fs::write(&data_path, "blocks plugin data directory")?;

    let first_failure = coordinator
        .reload(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("invalid preparation unexpectedly published"))?;
    let first_pending = first_failure
        .receipt()
        .ok_or_else(|| std::io::Error::other("invalid preparation receipt missing"))?;
    let operation_id = first_pending.operation_id();
    assert_eq!(first_pending.phase(), PluginOperationPhase::Preparation);
    assert_eq!(coordinator.active_generation(), Some(first_generation));
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 0);
    assert!(agent.has_skill("skill-a"));

    let repeated = coordinator
        .retry(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("persistently invalid preparation advanced"))?;
    assert_eq!(
        repeated.receipt().map(PluginOperationReceipt::operation_id),
        Some(operation_id)
    );
    assert_eq!(coordinator.active_generation(), Some(first_generation));
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 0);

    std::fs::remove_file(&data_path)?;
    std::fs::create_dir_all(&data_path)?;
    let recovered = coordinator.retry(&mut agent).await?;
    assert_eq!(recovered.operation_id(), operation_id);
    assert!(
        recovered
            .actual_generation()
            .is_some_and(|generation| generation > first_generation)
    );
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn missing_dependency_can_be_installed_on_disk_and_retried_with_same_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let local_plugins = temporary.path().join(".echo-agent/plugins.local");
    std::fs::create_dir_all(&local_plugins)?;
    create_plugin_with_dependencies(
        &local_plugins,
        "aaa-app",
        "app-skill",
        serde_json::json!([{"name":"zzz-base","version":">=1.0.0"}]),
    )?;
    let mut coordinator =
        PluginCoordinator::new(registry(temporary.path()), PluginIntegrator::new());
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-dependency-refresh")
        .build()?;

    let failure = coordinator
        .startup(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("missing dependency unexpectedly converged"))?;
    let pending = failure
        .receipt()
        .ok_or_else(|| std::io::Error::other("dependency failure receipt missing"))?;
    let operation_id = pending.operation_id();
    assert_eq!(pending.phase(), PluginOperationPhase::Preparation);
    assert!(coordinator.active_generation().is_none());

    create_plugin_with_dependencies(
        &local_plugins,
        "zzz-base",
        "base-skill",
        serde_json::json!([]),
    )?;
    let recovered = coordinator.retry(&mut agent).await?;
    assert_eq!(recovered.operation_id(), operation_id);
    assert_eq!(recovered.loaded_event_attempts(), ["zzz-base", "aaa-app"]);
    assert!(recovered.actual_generation().is_some());
    Ok(())
}

#[tokio::test]
async fn restricted_scope_retry_preserves_the_registry_discovery_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let project_plugins = temporary.path().join(".echo-agent/plugins");
    let local_plugins = temporary.path().join(".echo-agent/plugins.local");
    std::fs::create_dir_all(&project_plugins)?;
    std::fs::create_dir_all(&local_plugins)?;
    create_plugin_with_dependencies(
        &project_plugins,
        "aaa-app",
        "app-skill",
        serde_json::json!([{"name":"zzz-base","version":">=1.0.0"}]),
    )?;
    create_plugin_with_dependencies(
        &local_plugins,
        "zzz-base",
        "local-base-skill",
        serde_json::json!([]),
    )?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.scan_scopes(&[PluginScope::Project])?;
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-restricted-scope")
        .build()?;

    let first_failure = coordinator
        .reconcile(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("restricted view imported local dependency"))?;
    let operation_id = first_failure
        .receipt()
        .map(PluginOperationReceipt::operation_id)
        .ok_or_else(|| std::io::Error::other("restricted-view receipt missing"))?;
    assert!(coordinator.registry().get("zzz-base").is_none());
    let repeated = coordinator
        .retry(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("retry widened restricted scope"))?;
    assert_eq!(
        repeated.receipt().map(PluginOperationReceipt::operation_id),
        Some(operation_id)
    );
    assert!(coordinator.registry().get("zzz-base").is_none());

    create_plugin_with_dependencies(
        &project_plugins,
        "zzz-base",
        "project-base-skill",
        serde_json::json!([]),
    )?;
    let recovered = coordinator.retry(&mut agent).await?;
    assert_eq!(recovered.operation_id(), operation_id);
    assert_eq!(recovered.loaded_event_attempts(), ["zzz-base", "aaa-app"]);
    assert!(agent.has_skill("project-base-skill"));
    assert!(!agent.has_skill("local-base-skill"));
    Ok(())
}

#[tokio::test]
async fn failed_registry_refresh_preserves_old_actual_until_same_operation_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let counts = Arc::new(LifecycleCounts::default());
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    coordinator.register_lifecycle("plugin-a", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-refresh-failure")
        .build()?;
    let first = coordinator.reconcile(&mut agent).await?;
    let first_generation = first
        .actual_generation()
        .ok_or_else(|| std::io::Error::other("initial generation missing"))?;
    let project_plugins = temporary.path().join(".echo-agent/plugins");
    std::fs::create_dir_all(&project_plugins)?;
    let duplicate = create_plugin(&project_plugins, "plugin-a", "duplicate-skill")?;

    let failure =
        coordinator.reload(&mut agent).await.err().ok_or_else(|| {
            std::io::Error::other("duplicate registry refresh unexpectedly passed")
        })?;
    let pending = failure
        .receipt()
        .ok_or_else(|| std::io::Error::other("refresh failure receipt missing"))?;
    let operation_id = pending.operation_id();
    assert_eq!(pending.phase(), PluginOperationPhase::Preparation);
    assert_eq!(coordinator.active_generation(), Some(first_generation));
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 0);
    assert!(agent.has_skill("skill-a"));
    assert!(coordinator.registry().get("plugin-a").is_some());

    std::fs::remove_dir_all(duplicate)?;
    let recovered = coordinator.retry(&mut agent).await?;
    assert_eq!(recovered.operation_id(), operation_id);
    assert!(
        recovered
            .actual_generation()
            .is_some_and(|generation| generation > first_generation)
    );
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn activation_failure_is_cleaned_by_lifecycle_authority_before_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let counts = Arc::new(LifecycleCounts::default());
    counts.fail_activate_once.store(true, Ordering::SeqCst);
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    coordinator.register_lifecycle("plugin-a", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-activate-retry")
        .build()?;

    let failure = coordinator
        .reconcile(&mut agent)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("activation failure unexpectedly converged"))?;
    assert_eq!(
        failure.receipt().map(|receipt| receipt.phase()),
        Some(PluginOperationPhase::CallbackActivation)
    );
    let retried = coordinator.retry(&mut agent).await?;
    assert_eq!(retried.status(), PluginRuntimeStatus::Converged);
    assert_eq!(counts.activate.load(Ordering::SeqCst), 2);
    assert_eq!(counts.deactivate.load(Ordering::SeqCst), 1);
    assert_eq!(counts.shutdown.load(Ordering::SeqCst), 1);
    assert_eq!(counts.init.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn dependency_topology_orders_callbacks_and_lifecycle_events()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let base =
        create_plugin_with_dependencies(&sources, "zzz-base", "base-skill", serde_json::json!([]))?;
    let dependent = create_plugin_with_dependencies(
        &sources,
        "aaa-app",
        "app-skill",
        serde_json::json!([{"name":"zzz-base","version":">=1.0.0"}]),
    )?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(base), PluginScope::Local)?;
    plugin_registry.install(&InstallSource::Local(dependent), PluginScope::Local)?;
    let callback_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hook_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    for plugin_id in ["zzz-base", "aaa-app"] {
        coordinator.register_lifecycle(
            plugin_id,
            Arc::new(OrderedLifecycle {
                plugin_id: plugin_id.to_string(),
                events: Arc::clone(&callback_events),
            }),
        )?;
    }
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-topology")
        .build()?;
    let hook_sink = Arc::clone(&hook_events);
    agent.hook_registry().write().await.set_programmatic_hook(
        "record-topology",
        &[HookEvent::PluginLoaded, HookEvent::PluginDisabled],
        Arc::new(move |context| {
            let hook_sink = Arc::clone(&hook_sink);
            Box::pin(async move {
                if let (Some(plugin_id), Ok(mut events)) = (context.matcher, hook_sink.lock()) {
                    events.push(format!("{}:{plugin_id}", context.event.as_str()));
                }
                HookResult::default()
            })
        }),
    );

    coordinator.reconcile(&mut agent).await?;
    assert_eq!(
        *callback_events
            .lock()
            .map_err(|_| std::io::Error::other("callback order lock poisoned"))?,
        [
            "init:zzz-base",
            "activate:zzz-base",
            "init:aaa-app",
            "activate:aaa-app"
        ]
    );
    assert_eq!(
        *hook_events
            .lock()
            .map_err(|_| std::io::Error::other("hook order lock poisoned"))?,
        ["PluginLoaded:zzz-base", "PluginLoaded:aaa-app"]
    );
    callback_events
        .lock()
        .map_err(|_| std::io::Error::other("callback order lock poisoned"))?
        .clear();
    hook_events
        .lock()
        .map_err(|_| std::io::Error::other("hook order lock poisoned"))?
        .clear();

    let receipt = coordinator.reload(&mut agent).await?;
    assert_eq!(receipt.disabled_event_attempts(), ["aaa-app", "zzz-base"]);
    assert_eq!(receipt.loaded_event_attempts(), ["zzz-base", "aaa-app"]);
    assert_eq!(
        *callback_events
            .lock()
            .map_err(|_| std::io::Error::other("callback order lock poisoned"))?,
        [
            "deactivate:aaa-app",
            "deactivate:zzz-base",
            "activate:zzz-base",
            "activate:aaa-app"
        ]
    );
    assert_eq!(
        *hook_events
            .lock()
            .map_err(|_| std::io::Error::other("hook order lock poisoned"))?,
        [
            "PluginDisabled:aaa-app",
            "PluginDisabled:zzz-base",
            "PluginLoaded:zzz-base",
            "PluginLoaded:aaa-app"
        ]
    );

    callback_events
        .lock()
        .map_err(|_| std::io::Error::other("callback order lock poisoned"))?
        .clear();
    hook_events
        .lock()
        .map_err(|_| std::io::Error::other("hook order lock poisoned"))?
        .clear();
    coordinator.shutdown(&mut agent).await?;
    assert_eq!(
        *callback_events
            .lock()
            .map_err(|_| std::io::Error::other("callback order lock poisoned"))?,
        [
            "deactivate:aaa-app",
            "deactivate:zzz-base",
            "shutdown:aaa-app",
            "shutdown:zzz-base"
        ]
    );
    assert_eq!(
        *hook_events
            .lock()
            .map_err(|_| std::io::Error::other("hook order lock poisoned"))?,
        ["PluginDisabled:aaa-app", "PluginDisabled:zzz-base"]
    );
    Ok(())
}

#[tokio::test]
async fn converged_receipt_is_bound_to_the_original_agent_target()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    let mut first_agent = ReactAgentBuilder::new()
        .model("coordinator-agent-a")
        .build()?;
    let first = coordinator.reconcile(&mut first_agent).await?;
    let mut second_agent = ReactAgentBuilder::new()
        .model("coordinator-agent-b")
        .build()?;
    let revision = coordinator.registry().revision();

    assert!(matches!(
        coordinator.disable(&mut second_agent, "plugin-a").await,
        Err(PluginCoordinatorError::WrongAgentTarget { .. })
    ));
    assert_eq!(coordinator.registry().revision(), revision);
    assert!(
        coordinator
            .registry()
            .get("plugin-a")
            .is_some_and(|entry| entry.enabled)
    );
    assert!(matches!(
        coordinator.reconcile(&mut second_agent).await,
        Err(PluginCoordinatorError::WrongAgentTarget { .. })
    ));
    assert!(coordinator.pending_receipt().is_none());

    let unchanged = coordinator.reconcile(&mut first_agent).await?;
    assert_eq!(unchanged.actual_generation(), first.actual_generation());
    Ok(())
}

#[tokio::test]
async fn registering_callbacks_invalidates_a_converged_noop()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    std::fs::create_dir_all(&sources)?;
    let source = create_plugin(&sources, "plugin-a", "skill-a")?;
    let mut plugin_registry = registry(temporary.path());
    plugin_registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let mut coordinator = PluginCoordinator::new(plugin_registry, PluginIntegrator::new());
    let mut agent = ReactAgentBuilder::new()
        .model("coordinator-register")
        .build()?;
    let initial = coordinator.reconcile(&mut agent).await?;
    let counts = Arc::new(LifecycleCounts::default());

    coordinator.register_lifecycle("plugin-a", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
    let reconciled = coordinator.reconcile(&mut agent).await?;
    assert!(reconciled.actual_generation().is_some_and(|generation| {
        initial
            .actual_generation()
            .is_some_and(|initial_generation| generation > initial_generation)
    }));
    assert_eq!(counts.init.load(Ordering::SeqCst), 1);
    assert_eq!(counts.activate.load(Ordering::SeqCst), 1);

    let unchanged = coordinator.reconcile(&mut agent).await?;
    assert_eq!(
        unchanged.actual_generation(),
        reconciled.actual_generation()
    );
    assert_eq!(counts.activate.load(Ordering::SeqCst), 1);
    Ok(())
}

#[cfg(feature = "mcp")]
#[test]
fn coordinator_reuses_direct_and_plugin_qualified_same_name_mcp_identity() {
    use echo_agent::mcp::McpServerId;

    let direct = McpServerId::direct("shared");
    let first = McpServerId::plugin("plugin-a", "shared");
    let second = McpServerId::plugin("plugin-b", "shared");
    assert_ne!(direct, first);
    assert_ne!(first, second);
    assert_ne!(direct.selector(), first.selector());
    assert_ne!(first.selector(), second.selector());
}
