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
    PluginUserConfigType, PluginVariables, ResolvedComponents, plugin_data_base_dir,
    set_plugin_data_base_dir, set_plugin_data_base_dir_name,
};

use std::collections::HashMap;
use std::path::PathBuf;

/// Successfully assembled components grouped by their owning plugin.
#[derive(Debug, Clone, Default)]
pub struct WiredPluginComponents {
    pub skills: Vec<String>,
    pub hooks_registered: bool,
    pub mcp_servers: Vec<String>,
}

/// Result of wiring plugin components into an agent.
#[derive(Debug, Default)]
pub struct PluginWiringResult {
    /// Enabled plugins with at least one live component wired successfully and
    /// no known wiring error in this reload.
    pub plugins_loaded: Vec<String>,
    /// Names of skills loaded.
    pub skills_loaded: Vec<String>,
    /// Names of plugins whose hooks were registered.
    pub hooks_registered: Vec<String>,
    /// Names of MCP servers connected.
    pub mcp_connected: Vec<String>,
    /// Agent definition files discovered for an application-layer constructor.
    /// They are not advertised as runnable until an executable instance exists.
    pub agents_discovered: Vec<String>,
    /// LSP config files discovered but not yet wired (TODO: framework
    /// `ReactAgent` does not hold an `LspManager`; the application layer
    /// constructs one and feeds it `LspConfig::from_file`).
    pub lsp_discovered: Vec<String>,
    /// Monitor config files discovered (TODO: no framework runtime consumer).
    pub monitors_discovered: Vec<String>,
    /// Theme files discovered (TODO: UI-layer consumer, not framework).
    pub themes_discovered: Vec<String>,
    /// Output-style files discovered (TODO: output-format consumer, not framework).
    pub output_styles_discovered: Vec<String>,
    /// Successful live registrations, grouped for exact unload/reload.
    pub components_by_plugin: HashMap<String, WiredPluginComponents>,
    /// Errors encountered during wiring.
    pub errors: Vec<String>,
}

impl PluginWiringResult {
    /// Whether wiring completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total number of components wired into the agent.
    ///
    /// Discovery-only categories are excluded because they have no live
    /// runtime consumer yet.
    pub fn total_wired(&self) -> usize {
        self.skills_loaded.len() + self.hooks_registered.len() + self.mcp_connected.len()
    }
}

/// Wires plugin components into the agent's subsystems.
///
/// The integrator reads resolved component paths from a `PluginEntry`
/// and registers them with the appropriate subsystem:
///
/// | Component | Target | Status |
/// |-----------|--------|--------|
/// | Skills | `SkillRegistry` via `ReactAgent::load_skills_from_dir` | assembled |
/// | Hooks | `HookRegistry` via `HookRegistry::register` | assembled |
/// | MCP servers | `McpManager` via `ReactAgent::load_mcp_from_file` | assembled |
/// | Agents | discovered path for an application-owned constructor | discovery only |
/// | LSP servers | discovered path; `ReactAgent` holds no `LspManager` | TODO |
/// | Monitors / Themes / Output styles | discovered paths; no framework runtime consumer | TODO |
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
    /// 3. Wires skills, hooks, and MCP servers into the agent; reports
    ///    discovered-but-unconsumed agent/LSP/monitor/theme/output-style files.
    ///
    /// # Subagent definitions (agents)
    ///
    /// Agent definition files remain discovery-only here. Registering a
    /// definition without an executable instance makes it appear in
    /// `agent_tool` while every dispatch fails, so the application must parse,
    /// construct, and register the definition and factory atomically.
    ///
    /// # Discovery-only components
    ///
    /// LSP servers, monitors, themes, and output styles are resolved and
    /// reported (`*_discovered`) but not assembled: `ReactAgent` holds no
    /// `LspManager`, and monitors/themes/output styles have no framework
    /// runtime consumer. See the [`PluginIntegrator`] table for the TODOs.
    pub async fn wire_all(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        registry: &mut PluginRegistry,
    ) -> PluginWiringResult {
        let mut result = PluginWiringResult::default();

        // Resolve dependency order
        let ordered_ids = match registry.resolve_enabled_dependencies() {
            Ok(ids) => ids,
            Err(e) => {
                result
                    .errors
                    .push(format!("Dependency resolution failed: {e}"));
                return result;
            }
        };

        // Collect components from all enabled plugins
        let mut skill_dirs: Vec<(String, PathBuf)> = Vec::new();
        let mut hooks_defs: Vec<(
            String,
            String,
            echo_execution::skills::hooks::HooksDefinition,
        )> = Vec::new();
        let mut mcp_files: Vec<(String, PathBuf)> = Vec::new();
        let mut agent_files: Vec<(String, PathBuf)> = Vec::new();
        let mut failed_plugins = std::collections::HashSet::new();

        for plugin_id in &ordered_ids {
            // Extract entry info before mutable borrow
            let entry_info = registry
                .get(plugin_id)
                .map(|e| e.root.display().to_string());

            let Some(root_display) = entry_info else {
                continue;
            };

            let resolved = match registry.resolve_components(plugin_id) {
                Ok(r) => r,
                Err(e) => {
                    failed_plugins.insert(plugin_id.clone());
                    result
                        .errors
                        .push(format!("Plugin '{plugin_id}' component resolution: {e}"));
                    continue;
                }
            };

            // Collect skill dirs, tagged with the owning plugin id so the
            // wiring loop can `tag_source` them for grouped unload (P1-reload).
            for d in &resolved.skill_dirs {
                skill_dirs.push((plugin_id.clone(), d.clone()));
            }

            // Collect hooks
            if let Some(ref hooks_file) = resolved.hooks_file {
                match std::fs::read_to_string(hooks_file) {
                    Ok(content) => {
                        match serde_yaml_ng::from_str::<
                            echo_execution::skills::hooks::HooksDefinition,
                        >(&content)
                        {
                            Ok(def) => hooks_defs.push((plugin_id.clone(), root_display, def)),
                            Err(error) => {
                                failed_plugins.insert(plugin_id.clone());
                                result.errors.push(format!(
                                    "Plugin '{plugin_id}' hooks YAML parse: {error}"
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        failed_plugins.insert(plugin_id.clone());
                        result.errors.push(format!(
                            "Plugin '{plugin_id}' hooks file {}: {error}",
                            hooks_file.display()
                        ));
                    }
                }
            }

            // Collect MCP files
            if let Some(ref mcp_file) = resolved.mcp_config_file {
                mcp_files.push((plugin_id.clone(), mcp_file.clone()));
            }

            // Collect agent definition files for application-owned construction.
            for file in &resolved.agent_files {
                agent_files.push((plugin_id.clone(), file.clone()));
            }

            // Discovery-only: report but do not assemble (no framework consumer).
            if let Some(ref lsp_file) = resolved.lsp_config_file {
                result.lsp_discovered.push(lsp_file.display().to_string());
            }
            if let Some(ref monitors_file) = resolved.monitors_file {
                result
                    .monitors_discovered
                    .push(monitors_file.display().to_string());
            }
            for theme_file in &resolved.theme_files {
                result
                    .themes_discovered
                    .push(theme_file.display().to_string());
            }
            for style_file in &resolved.output_style_files {
                result
                    .output_styles_discovered
                    .push(style_file.display().to_string());
            }
        }

        // Wire skills — load then tag each batch with its owning plugin id so
        // `SkillRegistry::unregister_by_source("plugin:{id}")` can remove them
        // on disable/uninstall (P1-reload).
        for (plugin_id, dir) in &skill_dirs {
            match agent.load_skills_from_dir(dir).await {
                Ok(names) => {
                    let source_tag = format!("plugin:{plugin_id}");
                    agent.tag_skills_source(&names, &source_tag).await;
                    result
                        .components_by_plugin
                        .entry(plugin_id.clone())
                        .or_default()
                        .skills
                        .extend(names.clone());
                    result.skills_loaded.extend(names);
                }
                Err(e) => {
                    failed_plugins.insert(plugin_id.clone());
                    result.errors.push(format!(
                        "Plugin '{plugin_id}' skills from {}: {e}",
                        dir.display()
                    ));
                }
            }
        }

        // Wire hooks — use the plugin-source registration path so hooks carry
        // `HookSource::Plugin(name)`, distinct from skill/user-config sources
        // (audit P0-2). Previously this filed plugin hooks under
        // `HookSource::Skill("plugin:…")`, collapsing source identity.
        {
            let mut hook_reg = agent.hook_registry().write().await;
            for (plugin_name, source_dir, def) in &hooks_defs {
                if hook_reg.register_plugin_hooks(plugin_name, source_dir, def.clone()) {
                    result
                        .components_by_plugin
                        .entry(plugin_name.clone())
                        .or_default()
                        .hooks_registered = true;
                    result.hooks_registered.push(plugin_name.clone());
                } else if !def.is_empty() {
                    failed_plugins.insert(plugin_name.clone());
                    result.errors.push(format!(
                        "Plugin '{plugin_name}' registered no valid hook actions"
                    ));
                }
            }
        }

        // Wire MCP servers
        #[cfg(feature = "mcp")]
        {
            for (plugin_id, mcp_file) in &mcp_files {
                let expected_servers = match crate::mcp::McpConfigFile::from_file(mcp_file) {
                    Ok(config) => {
                        let mut names = config.mcp_servers.keys().cloned().collect::<Vec<_>>();
                        names.sort();
                        names
                    }
                    Err(error) => {
                        failed_plugins.insert(plugin_id.clone());
                        result.errors.push(format!(
                            "Plugin '{plugin_id}' MCP config {}: {error}",
                            mcp_file.display()
                        ));
                        continue;
                    }
                };
                match agent.load_mcp_from_file(mcp_file).await {
                    Ok(clients) => {
                        let connected_servers = clients
                            .iter()
                            .map(|client| client.server_name().to_string())
                            .collect::<std::collections::HashSet<_>>();
                        for client in &clients {
                            let server_name = client.server_name().to_string();
                            result.mcp_connected.push(server_name.clone());
                            result
                                .components_by_plugin
                                .entry(plugin_id.clone())
                                .or_default()
                                .mcp_servers
                                .push(server_name);
                        }
                        let missing = expected_servers
                            .into_iter()
                            .filter(|name| !connected_servers.contains(name))
                            .collect::<Vec<_>>();
                        if !missing.is_empty() {
                            failed_plugins.insert(plugin_id.clone());
                            result.errors.push(format!(
                                "Plugin '{plugin_id}' failed to connect MCP server(s): {}",
                                missing.join(", ")
                            ));
                        }
                    }
                    Err(e) => {
                        failed_plugins.insert(plugin_id.clone());
                        result.errors.push(format!(
                            "Plugin '{plugin_id}' MCP from {}: {e}",
                            mcp_file.display()
                        ));
                    }
                }
            }
        }

        #[cfg(not(feature = "mcp"))]
        for (plugin_id, mcp_file) in &mcp_files {
            failed_plugins.insert(plugin_id.clone());
            result.errors.push(format!(
                "Plugin '{plugin_id}' declares MCP config {}, but the framework was built without the 'mcp' feature",
                mcp_file.display()
            ));
        }

        for (plugin_id, file) in agent_files {
            result
                .agents_discovered
                .push(format!("{plugin_id}:{}", file.display()));
        }

        result.plugins_loaded = ordered_ids
            .into_iter()
            .filter(|plugin_id| {
                result.components_by_plugin.contains_key(plugin_id)
                    && !failed_plugins.contains(plugin_id)
            })
            .collect();

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
            let _ = registry.register_plugin_hooks(plugin_name, source_dir, def.clone());
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
                for client in clients {
                    connected.push(client.server_name().to_string());
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
