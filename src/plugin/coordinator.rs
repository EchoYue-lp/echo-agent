//! Serialized host-level coordination for plugin desired and actual state.

use super::{
    PluginIntegrator, PluginLifecycle, PluginLifecycleManager, PluginPublicationTarget,
    PluginRegistry, PluginWiringError, PluginWiringResult, PreparedPluginSet,
};
use crate::agent::react::ReactAgent;
use crate::skills::hooks::{HookContext, HookEvent, HookRegistry};
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

/// The host operation represented by a coordinator receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOperationKind {
    Startup,
    Reconcile,
    Enable { plugin_id: String },
    Reload,
    Disable { plugin_id: String },
    Uninstall { plugin_id: String, keep_data: bool },
    Shutdown,
}

/// The next phase that must settle before an operation can advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginOperationPhase {
    DesiredCommitted,
    Preparation,
    CallbackWithdrawal,
    WiringWithdrawal,
    WiringPublication,
    CallbackActivation,
    EventEmission,
    Committed,
}

/// Whether the runtime has converged to the durable registry intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRuntimeStatus {
    ActualPending,
    Converged,
}

/// Stable in-process receipt for one serialized plugin transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOperationReceipt {
    operation_id: u64,
    kind: PluginOperationKind,
    desired_revision: u64,
    phase: PluginOperationPhase,
    status: PluginRuntimeStatus,
    actual_generation: Option<u64>,
    disabled_event_attempts: Vec<String>,
    loaded_event_attempts: Vec<String>,
    last_error: Option<String>,
}

impl PluginOperationReceipt {
    pub fn operation_id(&self) -> u64 {
        self.operation_id
    }

    pub fn kind(&self) -> &PluginOperationKind {
        &self.kind
    }

    pub fn desired_revision(&self) -> u64 {
        self.desired_revision
    }

    pub fn phase(&self) -> PluginOperationPhase {
        self.phase
    }

    pub fn status(&self) -> PluginRuntimeStatus {
        self.status
    }

    pub fn actual_generation(&self) -> Option<u64> {
        self.actual_generation
    }

    /// PluginDisabled attempts already issued by this operation.
    pub fn disabled_event_attempts(&self) -> &[String] {
        &self.disabled_event_attempts
    }

    /// PluginLoaded attempts already issued by this operation.
    pub fn loaded_event_attempts(&self) -> &[String] {
        &self.loaded_event_attempts
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

/// Coordinator refusal or a retryable actual-state gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCoordinatorError {
    OperationPending(Box<PluginOperationReceipt>),
    NoPendingOperation,
    OperationIdExhausted,
    WrongAgentTarget {
        active_generation: Option<u64>,
    },
    DesiredMutation {
        kind: PluginOperationKind,
        message: String,
    },
    ActualPending {
        receipt: Box<PluginOperationReceipt>,
        message: String,
    },
}

impl PluginCoordinatorError {
    pub fn receipt(&self) -> Option<&PluginOperationReceipt> {
        match self {
            Self::OperationPending(receipt) | Self::ActualPending { receipt, .. } => {
                Some(receipt.as_ref())
            }
            Self::NoPendingOperation
            | Self::OperationIdExhausted
            | Self::WrongAgentTarget { .. }
            | Self::DesiredMutation { .. } => None,
        }
    }
}

impl fmt::Display for PluginCoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperationPending(receipt) => write!(
                formatter,
                "plugin operation {} is pending at {:?}",
                receipt.operation_id, receipt.phase
            ),
            Self::NoPendingOperation => formatter.write_str("no plugin operation is pending"),
            Self::OperationIdExhausted => {
                formatter.write_str("plugin operation identity exhausted")
            }
            Self::WrongAgentTarget { active_generation } => match active_generation {
                Some(generation) => write!(
                    formatter,
                    "plugin coordinator is bound to another Agent at generation {generation}"
                ),
                None => formatter.write_str("plugin coordinator is bound to another Agent"),
            },
            Self::DesiredMutation { kind, message } => {
                write!(
                    formatter,
                    "plugin {kind:?} desired-state mutation failed: {message}"
                )
            }
            Self::ActualPending { receipt, message } => write!(
                formatter,
                "plugin operation {} has desired revision {} committed but actual state is pending at {:?}: {message}",
                receipt.operation_id, receipt.desired_revision, receipt.phase
            ),
        }
    }
}

impl std::error::Error for PluginCoordinatorError {}

struct PendingOperation {
    receipt: PluginOperationReceipt,
    previous_ids: Vec<String>,
    desired_ids: Vec<String>,
    prepared: Option<Arc<PreparedPluginSet>>,
    disabled_hooks: Option<HookRegistry>,
    disabled_ids: Vec<String>,
    loaded_ids: Vec<String>,
    disabled_cursor: usize,
    loaded_cursor: usize,
    callback_retry_required: bool,
}

/// Coordinates durable plugin intent, Agent publication, and callbacks.
///
/// Methods require `&mut self`, making this value the transition admission
/// token. Hosts sharing it across tasks should place the coordinator and its
/// Agent behind the same async mutex.
pub struct PluginCoordinator {
    registry: PluginRegistry,
    integrator: PluginIntegrator,
    lifecycle: PluginLifecycleManager,
    publication_target: Option<PluginPublicationTarget>,
    active_receipt: Option<PluginWiringResult>,
    active_ids: Vec<String>,
    active_revision: Option<u64>,
    pending: Option<PendingOperation>,
    last_receipt: Option<PluginOperationReceipt>,
    next_operation_id: u64,
}

impl PluginCoordinator {
    pub fn new(registry: PluginRegistry, integrator: PluginIntegrator) -> Self {
        Self {
            registry,
            integrator,
            lifecycle: PluginLifecycleManager::new(),
            publication_target: None,
            active_receipt: None,
            active_ids: Vec::new(),
            active_revision: None,
            pending: None,
            last_receipt: None,
            next_operation_id: 1,
        }
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    pub fn register_lifecycle(
        &mut self,
        plugin_id: impl Into<String>,
        callbacks: Arc<dyn PluginLifecycle>,
    ) -> Result<(), String> {
        if let Some(pending) = &self.pending {
            return Err(format!(
                "Cannot register lifecycle callbacks while plugin operation {} is pending",
                pending.receipt.operation_id
            ));
        }
        self.lifecycle.register(plugin_id, callbacks)?;
        self.active_revision = None;
        Ok(())
    }

    pub fn last_receipt(&self) -> Option<&PluginOperationReceipt> {
        self.last_receipt.as_ref()
    }

    pub fn pending_receipt(&self) -> Option<&PluginOperationReceipt> {
        self.pending.as_ref().map(|pending| &pending.receipt)
    }

    pub fn active_generation(&self) -> Option<u64> {
        self.active_receipt
            .as_ref()
            .map(PluginWiringResult::generation)
    }

    /// Scan durable plugin locations, then converge a fresh Agent.
    pub async fn startup(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        self.registry
            .scan_all()
            .map_err(|error| PluginCoordinatorError::DesiredMutation {
                kind: PluginOperationKind::Startup,
                message: error.to_string(),
            })?;
        self.integrator.invalidate(&self.registry);
        self.begin(PluginOperationKind::Startup)?;
        self.run_pending(agent).await
    }

    /// Reconcile current registry intent without mutating it.
    pub async fn reconcile(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        if self.is_converged(agent) {
            return self.commit_noop(PluginOperationKind::Reconcile);
        }
        self.integrator.invalidate(&self.registry);
        self.begin(PluginOperationKind::Reconcile)?;
        self.run_pending(agent).await
    }

    pub async fn enable(
        &mut self,
        agent: &mut ReactAgent,
        plugin_id: &str,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        let kind = PluginOperationKind::Enable {
            plugin_id: plugin_id.to_string(),
        };
        self.registry.enable(plugin_id).map_err(|message| {
            PluginCoordinatorError::DesiredMutation {
                kind: kind.clone(),
                message,
            }
        })?;
        if self.is_converged(agent) {
            return self.commit_noop(kind);
        }
        self.integrator.invalidate(&self.registry);
        self.begin(kind)?;
        self.run_pending(agent).await
    }

    pub async fn reload(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        self.integrator.invalidate(&self.registry);
        self.begin(PluginOperationKind::Reload)?;
        self.run_pending(agent).await
    }

    pub async fn disable(
        &mut self,
        agent: &mut ReactAgent,
        plugin_id: &str,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        let kind = PluginOperationKind::Disable {
            plugin_id: plugin_id.to_string(),
        };
        self.registry.disable(plugin_id).map_err(|message| {
            PluginCoordinatorError::DesiredMutation {
                kind: kind.clone(),
                message,
            }
        })?;
        if self.is_converged(agent) {
            return self.commit_noop(kind);
        }
        self.integrator.invalidate(&self.registry);
        self.begin(kind)?;
        self.run_pending(agent).await
    }

    pub async fn uninstall(
        &mut self,
        agent: &mut ReactAgent,
        plugin_id: &str,
        keep_data: bool,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        let kind = PluginOperationKind::Uninstall {
            plugin_id: plugin_id.to_string(),
            keep_data,
        };
        self.registry
            .uninstall(plugin_id, keep_data)
            .map_err(|message| PluginCoordinatorError::DesiredMutation {
                kind: kind.clone(),
                message,
            })?;
        self.integrator.invalidate(&self.registry);
        self.begin(kind)?;
        self.run_pending(agent).await
    }

    /// Withdraw process-local effects while retaining durable enabled intent.
    pub async fn shutdown(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        self.ensure_idle()?;
        self.ensure_agent_target(agent)?;
        self.begin(PluginOperationKind::Shutdown)?;
        self.run_pending(agent).await
    }

    /// Resume the exact phase retained by the failed or cancelled operation.
    pub async fn retry(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        if self.pending.is_none() {
            return Err(PluginCoordinatorError::NoPendingOperation);
        }
        self.ensure_agent_target(agent)?;
        self.run_pending(agent).await
    }

    fn ensure_idle(&self) -> Result<(), PluginCoordinatorError> {
        if let Some(pending) = &self.pending {
            Err(PluginCoordinatorError::OperationPending(Box::new(
                pending.receipt.clone(),
            )))
        } else {
            Ok(())
        }
    }

    fn ensure_agent_target(&self, agent: &ReactAgent) -> Result<(), PluginCoordinatorError> {
        if self
            .publication_target
            .as_ref()
            .is_some_and(|target| !target.is_for_agent(agent))
        {
            Err(PluginCoordinatorError::WrongAgentTarget {
                active_generation: self.active_generation(),
            })
        } else {
            Ok(())
        }
    }

    fn begin(&mut self, kind: PluginOperationKind) -> Result<(), PluginCoordinatorError> {
        let operation_id = self.next_operation_id;
        self.next_operation_id = self
            .next_operation_id
            .checked_add(1)
            .ok_or(PluginCoordinatorError::OperationIdExhausted)?;
        let previous_ids = self.active_ids.clone();
        let is_shutdown = matches!(kind, PluginOperationKind::Shutdown);
        let desired_ids = Vec::new();
        let disabled_ids = if is_shutdown {
            previous_ids.iter().rev().cloned().collect()
        } else {
            Vec::new()
        };
        let loaded_ids = Vec::new();
        self.pending = Some(PendingOperation {
            receipt: PluginOperationReceipt {
                operation_id,
                kind,
                desired_revision: self.registry.revision(),
                phase: PluginOperationPhase::DesiredCommitted,
                status: PluginRuntimeStatus::ActualPending,
                actual_generation: None,
                disabled_event_attempts: Vec::new(),
                loaded_event_attempts: Vec::new(),
                last_error: None,
            },
            previous_ids,
            desired_ids,
            prepared: None,
            disabled_hooks: None,
            disabled_ids,
            loaded_ids,
            disabled_cursor: 0,
            loaded_cursor: 0,
            callback_retry_required: false,
        });
        Ok(())
    }

    fn is_converged(&self, agent: &ReactAgent) -> bool {
        self.active_receipt.is_some()
            && self.active_revision == Some(self.registry.revision())
            && self
                .registry
                .resolve_enabled_dependencies()
                .is_ok_and(|desired_ids| self.active_ids == desired_ids)
            && self
                .publication_target
                .as_ref()
                .is_some_and(|target| target.is_for_agent(agent))
    }

    fn commit_noop(
        &mut self,
        kind: PluginOperationKind,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        let operation_id = self.next_operation_id;
        self.next_operation_id = self
            .next_operation_id
            .checked_add(1)
            .ok_or(PluginCoordinatorError::OperationIdExhausted)?;
        let receipt = PluginOperationReceipt {
            operation_id,
            kind,
            desired_revision: self.registry.revision(),
            phase: PluginOperationPhase::Committed,
            status: PluginRuntimeStatus::Converged,
            actual_generation: self.active_generation(),
            disabled_event_attempts: Vec::new(),
            loaded_event_attempts: Vec::new(),
            last_error: None,
        };
        self.last_receipt = Some(receipt.clone());
        Ok(receipt)
    }

    async fn run_pending(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        if self.phase()? == PluginOperationPhase::DesiredCommitted {
            if self.is_shutdown()? {
                self.set_phase(PluginOperationPhase::CallbackWithdrawal)?;
            } else {
                self.set_phase(PluginOperationPhase::Preparation)?;
            }
        }
        if self.phase()? == PluginOperationPhase::Preparation {
            self.prepare_pending().await?;
            self.set_phase(PluginOperationPhase::CallbackWithdrawal)?;
        }
        if self.phase()? == PluginOperationPhase::CallbackWithdrawal {
            self.withdraw_callbacks()?;
            self.set_phase(PluginOperationPhase::WiringWithdrawal)?;
        }
        if self.phase()? == PluginOperationPhase::WiringWithdrawal {
            self.withdraw_wiring(agent).await?;
            let needs_disabled_snapshot = self
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.disabled_ids.is_empty());
            if needs_disabled_snapshot {
                let snapshot = agent.hook_registry().read().await.clone();
                if let Some(pending) = &mut self.pending {
                    pending.disabled_hooks = Some(snapshot);
                }
            }
            if self.is_shutdown()? {
                self.set_phase(PluginOperationPhase::EventEmission)?;
            } else {
                self.set_phase(PluginOperationPhase::WiringPublication)?;
            }
        }
        if self.phase()? == PluginOperationPhase::WiringPublication {
            self.publish_wiring(agent).await?;
            self.set_phase(PluginOperationPhase::CallbackActivation)?;
        }
        if self.phase()? == PluginOperationPhase::CallbackActivation {
            self.activate_callbacks()?;
            self.set_phase(PluginOperationPhase::EventEmission)?;
        }
        if self.phase()? == PluginOperationPhase::EventEmission {
            self.emit_events(agent).await?;
            self.set_phase(PluginOperationPhase::Committed)?;
        }
        self.commit_pending()
    }

    fn phase(&self) -> Result<PluginOperationPhase, PluginCoordinatorError> {
        self.pending
            .as_ref()
            .map(|pending| pending.receipt.phase)
            .ok_or(PluginCoordinatorError::NoPendingOperation)
    }

    fn set_phase(&mut self, phase: PluginOperationPhase) -> Result<(), PluginCoordinatorError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or(PluginCoordinatorError::NoPendingOperation)?;
        pending.receipt.phase = phase;
        Ok(())
    }

    fn is_shutdown(&self) -> Result<bool, PluginCoordinatorError> {
        self.pending
            .as_ref()
            .map(|pending| matches!(pending.receipt.kind, PluginOperationKind::Shutdown))
            .ok_or(PluginCoordinatorError::NoPendingOperation)
    }

    async fn prepare_pending(&mut self) -> Result<(), PluginCoordinatorError> {
        if let Err(error) = self.registry.refresh_current_view() {
            self.clear_failed_preparation();
            return Err(self.actual_pending(format!(
                "plugin registry refresh failed before withdrawal: {error}"
            )));
        }
        let desired_ids = match self.registry.resolve_enabled_dependencies() {
            Ok(desired_ids) => desired_ids,
            Err(error) => {
                self.clear_failed_preparation();
                return Err(self.actual_pending(format!(
                    "plugin dependency resolution failed before withdrawal: {error}"
                )));
            }
        };
        self.integrator.invalidate(&self.registry);
        let prepared = self.integrator.prepare(&mut self.registry).await;
        if !prepared.is_applicable() {
            self.clear_failed_preparation();
            return Err(self.actual_pending(format!(
                "prepared plugin generation {} is not applicable",
                prepared.generation()
            )));
        }
        let previous_ids = self
            .pending
            .as_ref()
            .map(|pending| pending.previous_ids.clone())
            .ok_or(PluginCoordinatorError::NoPendingOperation)?;
        let desired = desired_ids.iter().cloned().collect::<BTreeSet<_>>();
        let is_reload = self
            .pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.receipt.kind, PluginOperationKind::Reload));
        let disabled_ids = if is_reload {
            previous_ids.iter().rev().cloned().collect()
        } else {
            previous_ids
                .iter()
                .rev()
                .filter(|plugin_id| !desired.contains(*plugin_id))
                .cloned()
                .collect()
        };
        if let Some(pending) = &mut self.pending {
            pending.receipt.desired_revision = self.registry.revision();
            pending.desired_ids = desired_ids.clone();
            pending.prepared = Some(prepared);
            pending.disabled_ids = disabled_ids;
            pending.loaded_ids = desired_ids;
        }
        Ok(())
    }

    fn clear_failed_preparation(&mut self) {
        self.integrator.invalidate(&self.registry);
        if let Some(pending) = &mut self.pending {
            pending.prepared = None;
            pending.desired_ids.clear();
            pending.disabled_ids.clear();
            pending.loaded_ids.clear();
        }
    }

    fn withdraw_callbacks(&mut self) -> Result<(), PluginCoordinatorError> {
        let kind = self
            .pending
            .as_ref()
            .map(|pending| pending.receipt.kind.clone())
            .ok_or(PluginCoordinatorError::NoPendingOperation)?;
        let active_ids = self.active_ids.clone();
        let mut errors = self
            .lifecycle
            .deactivate_in_order(active_ids.iter().rev().map(String::as_str));
        match kind {
            PluginOperationKind::Uninstall { plugin_id, .. } => {
                if let Err(error) = self.lifecycle.unregister(&plugin_id) {
                    errors.push(error);
                }
            }
            PluginOperationKind::Shutdown => {
                let mut installed_ids = self.registry.resolve_dependencies().unwrap_or_else(|_| {
                    self.registry
                        .list()
                        .into_iter()
                        .map(|entry| entry.manifest.name.clone())
                        .collect()
                });
                installed_ids.reverse();
                errors.extend(
                    self.lifecycle
                        .unregister_in_order(installed_ids.iter().map(String::as_str)),
                );
                if errors.is_empty() {
                    // Registrations without an installed package have no graph
                    // position; settle those only after graph-owned callbacks.
                    errors.extend(self.lifecycle.shutdown());
                }
            }
            PluginOperationKind::Startup
            | PluginOperationKind::Reconcile
            | PluginOperationKind::Enable { .. }
            | PluginOperationKind::Reload
            | PluginOperationKind::Disable { .. } => {}
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(self.actual_pending(errors.join("; ")))
        }
    }

    async fn withdraw_wiring(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<(), PluginCoordinatorError> {
        let Some(receipt) = self.active_receipt.clone() else {
            return Ok(());
        };
        let target = self.target(agent);
        match target.rollback(agent, &receipt).await {
            Ok(()) => {
                self.active_receipt = None;
                Ok(())
            }
            Err(error) => Err(self.actual_pending(error.to_string())),
        }
    }

    async fn publish_wiring(
        &mut self,
        agent: &mut ReactAgent,
    ) -> Result<(), PluginCoordinatorError> {
        let target = self.target(agent);
        if let Some(cleanup) = target.pending_cleanup_receipt().await
            && let Err(error) = target.rollback(agent, &cleanup).await
        {
            return Err(self.actual_pending(error.to_string()));
        }
        let prepared = match self
            .pending
            .as_ref()
            .and_then(|pending| pending.prepared.clone())
        {
            Some(prepared) => prepared,
            None => {
                return Err(self.actual_pending(
                    "validated plugin preparation is missing before publication".to_string(),
                ));
            }
        };
        match target.wire_prepared(agent, &prepared).await {
            Ok(receipt) => {
                if let Some(pending) = &mut self.pending {
                    pending.receipt.actual_generation = Some(receipt.generation());
                }
                self.active_receipt = Some(receipt);
                Ok(())
            }
            Err(error @ PluginWiringError::InvalidPreparedSet { .. }) => {
                self.set_phase(PluginOperationPhase::Preparation)?;
                self.clear_failed_preparation();
                Err(self.actual_pending(wiring_error_message(error)))
            }
            Err(error) => Err(self.actual_pending(wiring_error_message(error))),
        }
    }

    fn activate_callbacks(&mut self) -> Result<(), PluginCoordinatorError> {
        let desired_ids = self
            .pending
            .as_ref()
            .map(|pending| pending.desired_ids.clone())
            .ok_or(PluginCoordinatorError::NoPendingOperation)?;
        let retry_required = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.callback_retry_required);
        if retry_required {
            let mut cleanup_errors = Vec::new();
            for plugin_id in desired_ids.iter().rev() {
                if let Err(error) = self.lifecycle.reset_for_retry(plugin_id) {
                    cleanup_errors.push(error);
                }
            }
            if !cleanup_errors.is_empty() {
                return Err(self.actual_pending(cleanup_errors.join("; ")));
            }
            if let Some(pending) = &mut self.pending {
                pending.callback_retry_required = false;
            }
        }
        let errors = self
            .lifecycle
            .activate_in_order(desired_ids.iter().map(String::as_str));
        if errors.is_empty() {
            Ok(())
        } else {
            if let Some(pending) = &mut self.pending {
                pending.callback_retry_required = true;
            }
            Err(self.actual_pending(errors.join("; ")))
        }
    }

    async fn emit_events(&mut self, agent: &mut ReactAgent) -> Result<(), PluginCoordinatorError> {
        loop {
            let next = {
                let pending = self
                    .pending
                    .as_mut()
                    .ok_or(PluginCoordinatorError::NoPendingOperation)?;
                if let Some(plugin_id) = pending.disabled_ids.get(pending.disabled_cursor).cloned()
                {
                    pending.disabled_cursor = pending.disabled_cursor.saturating_add(1);
                    pending
                        .receipt
                        .disabled_event_attempts
                        .push(plugin_id.clone());
                    Some((
                        HookEvent::PluginDisabled,
                        plugin_id,
                        pending.disabled_hooks.clone(),
                    ))
                } else if let Some(plugin_id) =
                    pending.loaded_ids.get(pending.loaded_cursor).cloned()
                {
                    pending.loaded_cursor = pending.loaded_cursor.saturating_add(1);
                    pending
                        .receipt
                        .loaded_event_attempts
                        .push(plugin_id.clone());
                    Some((HookEvent::PluginLoaded, plugin_id, None))
                } else {
                    None
                }
            };
            let Some((event, plugin_id, registry_snapshot)) = next else {
                return Ok(());
            };
            let registry = match registry_snapshot {
                Some(registry) => registry,
                None => agent.hook_registry().read().await.clone(),
            };
            let context = HookContext::for_lifecycle(
                event,
                &plugin_id,
                agent.config().get_session_id().unwrap_or_default(),
                agent.config().get_agent_name(),
            );
            let _ = registry.run_lifecycle_hooks(&context).await;
        }
    }

    fn target(&mut self, agent: &ReactAgent) -> PluginPublicationTarget {
        match &self.publication_target {
            Some(target) => target.clone(),
            None => {
                let target = self.integrator.publication_target(agent);
                self.publication_target = Some(target.clone());
                target
            }
        }
    }

    fn actual_pending(&mut self, message: String) -> PluginCoordinatorError {
        let receipt = match &mut self.pending {
            Some(pending) => {
                pending.receipt.status = PluginRuntimeStatus::ActualPending;
                pending.receipt.last_error = Some(message.clone());
                pending.receipt.clone()
            }
            None => return PluginCoordinatorError::NoPendingOperation,
        };
        PluginCoordinatorError::ActualPending {
            receipt: Box::new(receipt),
            message,
        }
    }

    fn commit_pending(&mut self) -> Result<PluginOperationReceipt, PluginCoordinatorError> {
        let mut pending = self
            .pending
            .take()
            .ok_or(PluginCoordinatorError::NoPendingOperation)?;
        pending.receipt.status = PluginRuntimeStatus::Converged;
        pending.receipt.last_error = None;
        self.active_ids = pending.desired_ids;
        self.active_revision = if matches!(pending.receipt.kind, PluginOperationKind::Shutdown) {
            None
        } else {
            Some(pending.receipt.desired_revision)
        };
        let receipt = pending.receipt;
        self.last_receipt = Some(receipt.clone());
        Ok(receipt)
    }
}

fn wiring_error_message(error: PluginWiringError) -> String {
    error.to_string()
}
