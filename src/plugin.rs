//! Plugin facade and immutable preparation API.
//!
//! Discovery and parsing happen once in [`PluginIntegrator::prepare`]. Live
//! Agents consume the returned generation without rereading package files.

pub use echo_core::plugin::{
    AGENT_PLUGIN_SCHEMA_V1, InstallSource, PluginAuthor, PluginCapability, PluginDependency,
    PluginEntry, PluginId, PluginLifecycle, PluginLifecycleManager, PluginManifest, PluginRegistry,
    PluginRegistryDiagnostic, PluginScope, PluginUserConfigEntry, PluginUserConfigType,
    PluginVariables, ResolvedComponents,
};

mod coordinator;
mod prepared;

pub use coordinator::{
    PluginCoordinator, PluginCoordinatorError, PluginOperationKind, PluginOperationPhase,
    PluginOperationReceipt, PluginRuntimeStatus,
};

pub use prepared::{
    PluginDiagnosticSeverity, PluginIntegrator, PluginPreparationDiagnostic,
    PluginPublicationTarget, PluginWiringError, PluginWiringResult, PreparedPlugin,
    PreparedPluginDocument, PreparedPluginSet, PreparedPluginSkill, WiredPluginComponents,
};

pub(crate) use prepared::PluginPublicationAuthority;
