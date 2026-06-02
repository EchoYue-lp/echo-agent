//! Plugin system — re-exports core types and provides integration layer.
//!
//! This module bridges the plugin types defined in `echo-core` with
//! the concrete subsystems (`SkillRegistry`, `HookRegistry`, `McpManager`).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use echo_agent::plugin::{PluginRegistry, PluginScope, InstallSource, PluginIntegrator};
//!
//! let mut registry = PluginRegistry::new(None);
//! registry.scan_all().unwrap();
//!
//! for entry in registry.list_enabled() {
//!     println!("{}: {}", entry.manifest.name, entry.manifest.description);
//! }
//! ```

// ── Re-exports from echo-core ──────────────────────────────────────────

pub use echo_core::plugin::{
    InstallSource, PluginAuthor, PluginCapability, PluginComponents, PluginDependency, PluginEntry,
    PluginId, PluginLifecycle, PluginManifest, PluginRegistry, PluginScope, PluginUserConfigEntry,
    PluginUserConfigType, PluginVariables, ResolvedComponents,
};

use std::path::PathBuf;

/// Result of wiring plugin components into an agent.
#[derive(Debug, Default)]
pub struct PluginWiringResult {
    /// Names of skills loaded.
    pub skills_loaded: Vec<String>,
    /// Names of plugins whose hooks were registered.
    pub hooks_registered: Vec<String>,
    /// Names of MCP servers connected.
    pub mcp_connected: Vec<String>,
    /// Errors encountered during wiring.
    pub errors: Vec<String>,
}

impl PluginWiringResult {
    /// Whether wiring completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total number of components wired.
    pub fn total_wired(&self) -> usize {
        self.skills_loaded.len() + self.hooks_registered.len() + self.mcp_connected.len()
    }
}

/// Wires plugin components into the agent's subsystems.
///
/// The integrator reads resolved component paths from a `PluginEntry`
/// and registers them with the appropriate subsystem:
///
/// | Component | Target |
/// |-----------|--------|
/// | Skills | `SkillRegistry` via `ReactAgent::load_skills_from_dir` |
/// | Hooks | `HookRegistry` via `HookRegistry::register` |
/// | MCP servers | `McpManager` via `ReactAgent::load_mcp_from_file` |
///
/// This struct lives in the facade crate because it needs access to
/// types from multiple workspace crates.
pub struct PluginIntegrator;

impl PluginIntegrator {
    pub fn new() -> Self {
        Self
    }

    /// Wire all enabled plugin components into a `ReactAgent`.
    ///
    /// This is the primary entry point. It:
    /// 1. Scans all plugin scopes and resolves dependencies.
    /// 2. For each enabled plugin, resolves component paths.
    /// 3. Wires skills, hooks, and MCP servers into the agent.
    pub async fn wire_all(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        registry: &mut PluginRegistry,
    ) -> PluginWiringResult {
        let mut result = PluginWiringResult::default();

        // Resolve dependency order
        let ordered_ids = match registry.resolve_dependencies() {
            Ok(ids) => ids,
            Err(e) => {
                result
                    .errors
                    .push(format!("Dependency resolution failed: {e}"));
                return result;
            }
        };

        // Collect components from all enabled plugins
        let mut skill_dirs: Vec<PathBuf> = Vec::new();
        let mut hooks_defs: Vec<(
            String,
            String,
            echo_execution::skills::hooks::HooksDefinition,
        )> = Vec::new();
        let mut mcp_files: Vec<PathBuf> = Vec::new();

        for plugin_id in &ordered_ids {
            // Extract entry info before mutable borrow
            let entry_info = registry
                .get(plugin_id)
                .map(|e| (e.enabled, e.root.display().to_string()));

            let Some((enabled, root_display)) = entry_info else {
                continue;
            };
            if !enabled {
                continue;
            }

            let resolved = match registry.resolve_components(plugin_id) {
                Ok(r) => r,
                Err(e) => {
                    result
                        .errors
                        .push(format!("Plugin '{plugin_id}' component resolution: {e}"));
                    continue;
                }
            };

            // Collect skill dirs
            skill_dirs.extend(resolved.skill_dirs.iter().cloned());

            // Collect hooks
            if let Some(ref hooks_file) = resolved.hooks_file
                && let Ok(content) = std::fs::read_to_string(hooks_file)
            {
                match serde_yaml_ng::from_str::<echo_execution::skills::hooks::HooksDefinition>(
                    &content,
                ) {
                    Ok(def) => {
                        hooks_defs.push((plugin_id.clone(), root_display, def));
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("Plugin '{plugin_id}' hooks YAML parse: {e}"));
                    }
                }
            }

            // Collect MCP files
            if let Some(ref mcp_file) = resolved.mcp_config_file {
                mcp_files.push(mcp_file.clone());
            }
        }

        // Wire skills
        for dir in &skill_dirs {
            match agent.load_skills_from_dir(dir).await {
                Ok(names) => {
                    result.skills_loaded.extend(names);
                }
                Err(e) => {
                    result
                        .errors
                        .push(format!("Skills from {}: {e}", dir.display()));
                }
            }
        }

        // Wire hooks
        {
            let mut hook_reg = agent.hook_registry().write().await;
            for (plugin_name, source_dir, def) in &hooks_defs {
                hook_reg.register(&format!("plugin:{plugin_name}"), source_dir, def.clone());
                result.hooks_registered.push(plugin_name.clone());
            }
        }

        // Wire MCP servers
        #[cfg(feature = "mcp")]
        {
            for mcp_file in &mcp_files {
                match agent.load_mcp_from_file(mcp_file).await {
                    Ok(clients) => {
                        for _c in &clients {
                            result.mcp_connected.push(mcp_file.display().to_string());
                        }
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("MCP from {}: {e}", mcp_file.display()));
                    }
                }
            }
        }

        result
    }

    /// Wire only skills from resolved components into the agent.
    pub async fn wire_skills(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        skill_dirs: &[PathBuf],
    ) -> Vec<String> {
        let mut loaded = Vec::new();
        for dir in skill_dirs {
            if let Ok(names) = agent.load_skills_from_dir(dir).await {
                loaded.extend(names);
            }
        }
        loaded
    }

    /// Wire only hooks from resolved components into the agent.
    pub async fn wire_hooks(
        &self,
        agent: &crate::agent::react::ReactAgent,
        hooks: &[(
            String,
            String,
            echo_execution::skills::hooks::HooksDefinition,
        )],
    ) {
        let mut registry = agent.hook_registry().write().await;
        for (plugin_name, source_dir, def) in hooks {
            registry.register(&format!("plugin:{plugin_name}"), source_dir, def.clone());
        }
    }

    /// Wire only MCP servers from resolved components into the agent.
    #[cfg(feature = "mcp")]
    pub async fn wire_mcp(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        mcp_files: &[PathBuf],
    ) -> Vec<String> {
        let mut connected = Vec::new();
        for file in mcp_files {
            if let Ok(clients) = agent.load_mcp_from_file(file).await {
                for _ in clients {
                    connected.push(file.display().to_string());
                }
            }
        }
        connected
    }
}

impl Default for PluginIntegrator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Backward-compatible legacy trait ───────────────────────────────────

/// Legacy native plugin trait — for code-level extensions.
///
/// Prefer file-based plugins (with `manifest.yaml`) for most use cases.
/// This trait is retained for backward compatibility with code-level plugins
/// that need to inject custom Rust logic.
pub trait NativePlugin: Send + Sync {
    /// Unique plugin identifier.
    fn id(&self) -> &str;
    /// Human-readable name.
    fn name(&self) -> &str;
    /// What this plugin provides.
    fn capabilities(&self) -> Vec<PluginCapability>;
    /// Plugin version.
    fn version(&self) -> &str;

    /// Initialize the plugin. Called once at startup.
    fn init(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Shutdown the plugin. Called at agent shutdown.
    fn shutdown(&mut self) -> Result<(), String> {
        Ok(())
    }
}
