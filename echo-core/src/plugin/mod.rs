//! Plugin system core contracts.

pub mod capability;
pub mod lifecycle;
pub mod manifest;
pub mod registry;
pub mod scope;
pub mod variables;

pub use capability::PluginCapability;
pub use lifecycle::{PluginLifecycle, PluginLifecycleManager};
pub use manifest::{
    AGENT_PLUGIN_SCHEMA_V1, PluginAuthor, PluginDependency, PluginManifest, PluginUserConfigEntry,
    PluginUserConfigType,
};
pub use registry::{
    PluginEntry, PluginId, PluginRegistry, PluginRegistryDiagnostic, ResolvedComponents,
};
pub use scope::{InstallSource, PluginScope};
pub use variables::PluginVariables;
