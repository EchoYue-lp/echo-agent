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

mod prepared;

pub use prepared::{
    PluginDiagnosticSeverity, PluginIntegrator, PluginPreparationDiagnostic, PluginWiringError,
    PluginWiringResult, PreparedPlugin, PreparedPluginDocument, PreparedPluginSet,
    PreparedPluginSkill, WiredPluginComponents,
};
