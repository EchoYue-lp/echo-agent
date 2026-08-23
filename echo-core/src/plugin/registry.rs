//! Plugin registry — discovery, installation, lifecycle management.
//!
//! The `PluginRegistry` is the central hub for managing plugins.
//! It handles scanning, installing, uninstalling, enabling/disabling,
//! and dependency resolution.

use crate::plugin::PluginCapability;
use crate::plugin::manifest::PluginManifest;
use crate::plugin::scope::{InstallSource, PluginScope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Unique identifier for a plugin (format: `name` or `name@scope`).
pub type PluginId = String;

/// State of an installed plugin, persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    /// Plugin manifest metadata.
    pub manifest: PluginManifest,
    /// Absolute path to the plugin's install directory.
    pub root: PathBuf,
    /// Which scope this plugin was installed to.
    pub scope: PluginScope,
    /// Whether the plugin is currently enabled.
    pub enabled: bool,
    /// Validated user configuration, including manifest defaults.
    #[serde(default)]
    pub user_config: HashMap<String, serde_json::Value>,
    /// Resolved component paths (absolute, populated at load time).
    #[serde(skip)]
    pub resolved_components: Option<ResolvedComponents>,
}

impl PluginEntry {
    /// Infer supported capabilities from their fixed package locations.
    pub fn inferred_capabilities(&self) -> Vec<PluginCapability> {
        let mut capabilities = Vec::new();
        if self.root.join("skills").is_dir() {
            capabilities.push(PluginCapability::Skill);
        }
        if self.root.join("mcp.json").is_file() {
            capabilities.push(PluginCapability::McpServer);
        }
        if self.root.join("agents").is_dir() {
            capabilities.push(PluginCapability::Agent);
        }
        if self.root.join("hooks/hooks.yaml").is_file() {
            capabilities.push(PluginCapability::Hook);
        }
        if self.root.join("lsp.yaml").is_file() {
            capabilities.push(PluginCapability::LspServer);
        }
        capabilities
    }
}

/// Resolved absolute paths for all plugin components.
///
/// Populated after the manifest is loaded and fixed locations are resolved
/// relative to the plugin root. `None` fields mean the component is absent.
#[derive(Debug, Clone, Default)]
pub struct ResolvedComponents {
    /// Standard root `skills/` directory.
    pub skill_dirs: Vec<PathBuf>,
    /// Agent definition markdown files.
    pub agent_files: Vec<PathBuf>,
    /// Hook configuration file.
    pub hooks_file: Option<PathBuf>,
    /// MCP server configuration file.
    pub mcp_config_file: Option<PathBuf>,
    /// LSP server configuration file.
    pub lsp_config_file: Option<PathBuf>,
    /// Non-fatal standard component diagnostics.
    pub diagnostics: Vec<String>,
}

/// Persistent registry state, serialized to `plugins.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryState {
    /// Map from plugin ID to its entry.
    plugins: HashMap<String, PluginEntry>,
}

/// The plugin registry — manages discovery, installation, and lifecycle.
pub struct PluginRegistry {
    /// Loaded plugin entries, keyed by plugin ID.
    plugins: HashMap<PluginId, PluginEntry>,
    /// Persistent state file path.
    state_file: PathBuf,
    /// Persistent data directory root.
    data_dir: PathBuf,
    /// Application-supplied user plugin installation directory.
    user_plugins_dir: PathBuf,
    /// Project root for resolving Project/Local scopes.
    project_root: Option<PathBuf>,
}

impl PluginRegistry {
    /// Create a registry under an explicit application data root.
    pub fn new(data_root: impl Into<PathBuf>, project_root: Option<PathBuf>) -> Self {
        let base = data_root.into();
        let user_plugins_dir = base.join("plugins");
        let state_file = base.join("plugins").join("registry.json");
        let data_dir = base.join("plugins").join("data");

        Self {
            plugins: HashMap::new(),
            state_file,
            data_dir,
            user_plugins_dir,
            project_root,
        }
    }

    /// Create a registry with custom paths (useful for testing).
    pub fn with_paths(
        state_file: PathBuf,
        data_dir: PathBuf,
        project_root: Option<PathBuf>,
    ) -> Self {
        let user_plugins_dir = state_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| data_dir.clone());
        Self {
            plugins: HashMap::new(),
            state_file,
            data_dir,
            user_plugins_dir,
            project_root,
        }
    }

    // ── Discovery ──────────────────────────────────────────────────────

    /// Scan all scopes for installed plugins and load them.
    ///
    /// Returns the number of plugins discovered.
    pub fn scan_all(&mut self) -> std::io::Result<usize> {
        self.scan_scopes(PluginScope::all())
    }

    /// Scan only the requested installation scopes.
    ///
    /// This is useful for embedded runtimes and isolated integration tests
    /// that intentionally expose a subset of the standard plugin scopes.
    pub fn scan_scopes(&mut self, scopes: &[PluginScope]) -> std::io::Result<usize> {
        self.plugins.clear();
        let mut total = 0;

        for scope in scopes {
            let dir = scope.resolve_dir(&self.user_plugins_dir, self.project_root.as_deref());
            let count = self.scan_scope_dir(*scope, &dir)?;
            total += count;
        }

        // Load persisted enabled/disabled state
        self.load_state()?;
        Ok(total)
    }

    /// Validate a plugin directory and resolve every supported component found
    /// at its fixed package location. Missing optional locations remain valid.
    pub fn validate_plugin_dir(
        root: &Path,
    ) -> Result<(PluginManifest, ResolvedComponents), Vec<String>> {
        let manifest_path = root.join("plugin.json");
        let manifest = PluginManifest::from_file(&manifest_path).map_err(|error| vec![error])?;
        let errors = manifest
            .validate()
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors);
        }

        let user_config = manifest.user_config_defaults();

        let plugin_id = manifest.name.clone();
        let mut registry = Self::with_paths(
            root.join(".plugin-validation-state.json"),
            root.join(".plugin-validation-data"),
            root.parent().map(Path::to_path_buf),
        );
        registry.plugins.insert(
            plugin_id.clone(),
            PluginEntry {
                manifest: manifest.clone(),
                root: root.to_path_buf(),
                scope: PluginScope::Local,
                enabled: true,
                user_config,
                resolved_components: None,
            },
        );
        registry
            .resolve_components(&plugin_id)
            .map(|resolved| (manifest, resolved))
            .map_err(|error| vec![error])
    }

    /// Scan a single directory for plugin subdirectories.
    fn scan_scope_dir(&mut self, scope: PluginScope, dir: &Path) -> std::io::Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut paths = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            if !path.is_dir() {
                continue;
            }

            // Agent Plugins manifests live at the package root.
            let manifest_path = path.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }

            match PluginManifest::from_file(&manifest_path) {
                Ok(manifest) => {
                    let validation_errors = manifest.validate();
                    if !validation_errors.is_empty() {
                        tracing::warn!(
                            path = %manifest_path.display(),
                            errors = %validation_errors
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join("; "),
                            "Skipping invalid plugin manifest"
                        );
                        continue;
                    }
                    let unknown_fields = manifest.unknown_top_level_fields();
                    if !unknown_fields.is_empty() {
                        tracing::warn!(
                            path = %manifest_path.display(),
                            fields = %unknown_fields.join(", "),
                            "Ignoring unknown Agent Plugins manifest fields"
                        );
                    }
                    let id = manifest.name.clone();
                    let user_config = manifest.user_config_defaults();
                    let config_ready = manifest.validate_user_config(&user_config).is_empty();
                    let enabled = manifest.default_enabled && config_ready;
                    if manifest.default_enabled && !config_ready {
                        tracing::info!(
                            plugin = %manifest.name,
                            "Plugin starts disabled until required configuration is provided"
                        );
                    }
                    let entry = PluginEntry {
                        manifest,
                        root: path.clone(),
                        scope,
                        enabled, // overridden by persisted state when present
                        user_config,
                        resolved_components: None,
                    };
                    if let Some(existing) = self.plugins.get(&id) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!(
                                "plugin name collision for '{id}': {} ({:?}) and {} ({scope:?})",
                                existing.root.display(),
                                existing.scope,
                                path.display()
                            ),
                        ));
                    }
                    self.plugins.insert(id, entry);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load plugin manifest at {}: {e}",
                        manifest_path.display()
                    );
                }
            }
        }

        Ok(count)
    }

    // ── Installation ───────────────────────────────────────────────────

    /// Install a plugin from a source into the given scope.
    ///
    /// For local sources, copies the directory.
    /// For git sources, clones the repository (shallow).
    ///
    /// Returns the plugin ID on success.
    pub fn install(
        &mut self,
        source: &InstallSource,
        scope: PluginScope,
    ) -> Result<PluginId, String> {
        let target_dir = scope.resolve_dir(&self.user_plugins_dir, self.project_root.as_deref());
        std::fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Failed to create plugin directory: {e}"))?;

        match source {
            InstallSource::Local(src_path) => self.install_local(src_path, &target_dir, scope),
            InstallSource::Git { url, subdir } => {
                self.install_git(url, subdir.as_deref(), &target_dir, scope)
            }
        }
    }

    fn install_local(
        &mut self,
        src: &Path,
        target_dir: &Path,
        scope: PluginScope,
    ) -> Result<PluginId, String> {
        // Validate source has a manifest
        let manifest_path = src.join("plugin.json");
        if !manifest_path.exists() {
            return Err(format!(
                "Source directory {} does not contain root plugin.json",
                src.display()
            ));
        }

        let manifest = PluginManifest::from_file(&manifest_path)?;
        let errors = manifest.validate();
        if !errors.is_empty() {
            return Err(format!(
                "Manifest validation failed: {}",
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let plugin_id = manifest.name.clone();
        let dest = target_dir.join(&plugin_id);

        if dest.exists() {
            return Err(format!(
                "Plugin '{plugin_id}' is already installed at {}",
                dest.display()
            ));
        }

        let staging = target_dir.join(format!(".{}.{}.staging", plugin_id, uuid::Uuid::new_v4()));
        if let Err(error) = copy_dir_recursive(src, &staging) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("Failed to stage plugin copy: {error}"));
        }
        if let Err(errors) = Self::validate_plugin_dir(&staging) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "Staged plugin validation failed: {}",
                errors.join("; ")
            ));
        }
        if let Err(error) = std::fs::rename(&staging, &dest) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("Failed to commit staged plugin: {error}"));
        }

        let user_config = manifest.user_config_defaults();
        let enabled =
            manifest.default_enabled && manifest.validate_user_config(&user_config).is_empty();
        let entry = PluginEntry {
            manifest,
            root: dest.clone(),
            scope,
            enabled,
            user_config,
            resolved_components: None,
        };

        self.plugins.insert(plugin_id.clone(), entry);
        if let Err(error) = self.resolve_enabled_dependencies() {
            self.plugins.remove(&plugin_id);
            let cleanup_error = std::fs::remove_dir_all(&dest).err();
            return Err(match cleanup_error {
                Some(cleanup_error) => format!(
                    "Cannot install plugin '{plugin_id}': {error}; additionally failed to roll back {}: {cleanup_error}",
                    dest.display()
                ),
                None => format!("Cannot install plugin '{plugin_id}': {error}"),
            });
        }
        if let Err(error) = self.save_state() {
            self.plugins.remove(&plugin_id);
            let cleanup_error = std::fs::remove_dir_all(&dest).err();
            return Err(match cleanup_error {
                Some(cleanup_error) => format!(
                    "{error}; additionally failed to roll back {}: {cleanup_error}",
                    dest.display()
                ),
                None => error,
            });
        }
        Ok(plugin_id)
    }

    fn install_git(
        &mut self,
        url: &str,
        subdir: Option<&str>,
        target_dir: &Path,
        scope: PluginScope,
    ) -> Result<PluginId, String> {
        // embedding application is a local user-controlled application. Accept the standard
        // encrypted Git transports, including private repositories over SSH;
        // reject only cleartext/obviously malformed remote inputs.
        let supported = url.starts_with("https://")
            || url.starts_with("ssh://")
            || (url.starts_with("git@") && url.contains(':'));
        if !supported {
            return Err(format!(
                "Plugin git URL must use https:// or SSH (received: {})",
                url.split("://").next().unwrap_or(url)
            ));
        }

        // Clone to a temporary directory
        let tmp_dir = target_dir.join(".tmp-clone");
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir)
                .map_err(|e| format!("Failed to clean temp directory: {e}"))?;
        }

        let status = std::process::Command::new("git")
            .args(["clone", "--depth", "1", url])
            .arg(&tmp_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("Failed to run git clone: {e}"))?;

        if !status.success() {
            return Err(format!("git clone failed for {url}"));
        }

        let src = if let Some(sub) = subdir {
            tmp_dir.join(sub)
        } else {
            tmp_dir.clone()
        };

        let result = self.install_local(&src, target_dir, scope);

        // Clean up temp directory
        let _ = std::fs::remove_dir_all(&tmp_dir);

        result
    }

    // ── Uninstallation ─────────────────────────────────────────────────

    /// Uninstall a plugin. If `keep_data` is false, the persistent data
    /// directory is also removed.
    pub fn uninstall(&mut self, plugin_id: &str, keep_data: bool) -> Result<(), String> {
        self.ensure_no_enabled_dependents(plugin_id)?;
        let entry = self
            .plugins
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;

        let tombstone = entry
            .root
            .with_extension(format!("plugin-uninstall-{}", uuid::Uuid::new_v4()));
        if entry.root.exists() {
            std::fs::rename(&entry.root, &tombstone)
                .map_err(|error| format!("Failed to stage plugin removal: {error}"))?;
        }
        self.plugins.remove(plugin_id);
        if let Err(error) = self.save_state() {
            self.plugins.insert(plugin_id.to_string(), entry.clone());
            let restore_error = if tombstone.exists() {
                std::fs::rename(&tombstone, &entry.root).err()
            } else {
                None
            };
            return Err(match restore_error {
                Some(restore_error) => format!(
                    "{error}; additionally failed to restore plugin directory: {restore_error}"
                ),
                None => error,
            });
        }
        if tombstone.exists()
            && let Err(error) = std::fs::remove_dir_all(&tombstone)
        {
            tracing::warn!(path = %tombstone.display(), %error, "plugin uninstall tombstone cleanup deferred");
        }
        if !keep_data {
            let data = PluginEntry::data_dir_for(plugin_id, &self.data_dir);
            if data.exists()
                && let Err(error) = std::fs::remove_dir_all(&data)
            {
                tracing::warn!(path = %data.display(), %error, "plugin data cleanup deferred");
            }
        }
        Ok(())
    }

    // ── Enable / Disable ───────────────────────────────────────────────

    /// Enable a disabled plugin.
    pub fn enable(&mut self, plugin_id: &str) -> Result<(), String> {
        let entry = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;
        let was_enabled = entry.enabled;
        if was_enabled {
            return Ok(());
        }
        let config_errors = entry.manifest.validate_user_config(&entry.user_config);
        if !config_errors.is_empty() {
            return Err(format!(
                "Cannot enable plugin '{plugin_id}': {}",
                config_errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }

        if let Some(entry) = self.plugins.get_mut(plugin_id) {
            entry.enabled = true;
        }
        if let Err(error) = self.resolve_enabled_dependencies() {
            if let Some(entry) = self.plugins.get_mut(plugin_id) {
                entry.enabled = false;
            }
            return Err(format!("Cannot enable plugin '{plugin_id}': {error}"));
        }
        if let Err(error) = self.save_state() {
            if let Some(entry) = self.plugins.get_mut(plugin_id) {
                entry.enabled = false;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Disable an enabled plugin without uninstalling it.
    pub fn disable(&mut self, plugin_id: &str) -> Result<(), String> {
        let was_enabled = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?
            .enabled;
        if !was_enabled {
            return Ok(());
        }

        self.ensure_no_enabled_dependents(plugin_id)?;
        if let Some(entry) = self.plugins.get_mut(plugin_id) {
            entry.enabled = false;
        }
        if let Err(error) = self.save_state() {
            if let Some(entry) = self.plugins.get_mut(plugin_id) {
                entry.enabled = true;
            }
            return Err(error);
        }
        Ok(())
    }

    fn ensure_no_enabled_dependents(&self, plugin_id: &str) -> Result<(), String> {
        let mut dependents = self
            .plugins
            .iter()
            .filter(|(_, entry)| {
                entry.enabled
                    && entry
                        .manifest
                        .dependencies
                        .iter()
                        .any(|dependency| dependency.name() == plugin_id)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        dependents.sort();
        if dependents.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Plugin '{plugin_id}' is required by enabled plugin(s): {}",
                dependents.join(", ")
            ))
        }
    }

    // ── Queries ────────────────────────────────────────────────────────

    /// Get a plugin entry by ID.
    pub fn get(&self, plugin_id: &str) -> Option<&PluginEntry> {
        self.plugins.get(plugin_id)
    }

    /// Replace and persist a plugin's user configuration.
    pub fn configure(
        &mut self,
        plugin_id: &str,
        values: HashMap<String, serde_json::Value>,
    ) -> Result<(), String> {
        let resolved = {
            let entry = self
                .plugins
                .get(plugin_id)
                .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;
            entry
                .manifest
                .resolve_user_config(&values)
                .map_err(|errors| {
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                })?
        };
        let previous = self
            .plugins
            .get(plugin_id)
            .map(|entry| entry.user_config.clone())
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;
        if let Some(entry) = self.plugins.get_mut(plugin_id) {
            entry.user_config = resolved;
        }
        if let Err(error) = self.save_state() {
            if let Some(entry) = self.plugins.get_mut(plugin_id) {
                entry.user_config = previous;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Build the substitution context for a plugin's component files.
    pub fn variables_for(&self, plugin_id: &str) -> Result<super::PluginVariables, String> {
        let entry = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;
        let project_dir = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| entry.root.clone()));
        Ok(super::PluginVariables::new(
            entry.root.clone(),
            self.data_dir_for(plugin_id),
            project_dir,
        )
        .with_json_user_config(&entry.user_config))
    }

    /// List all installed plugins.
    pub fn list(&self) -> Vec<&PluginEntry> {
        let mut entries: Vec<_> = self.plugins.values().collect();
        entries.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        entries
    }

    /// List enabled plugins only.
    pub fn list_enabled(&self) -> Vec<&PluginEntry> {
        self.list().into_iter().filter(|e| e.enabled).collect()
    }

    /// Search plugins by keyword (matches name, description, keywords).
    pub fn search(&self, query: &str) -> Vec<&PluginEntry> {
        let q = query.to_lowercase();
        self.list()
            .into_iter()
            .filter(|e| {
                e.manifest.name.to_lowercase().contains(&q)
                    || e.manifest.description.to_lowercase().contains(&q)
                    || e.manifest
                        .keywords
                        .iter()
                        .any(|k| k.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Get the total number of installed plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    // ── Component Resolution ───────────────────────────────────────────

    /// Resolve component paths for a plugin, making them absolute.
    ///
    /// Component locations are fixed by the flat package layout. Missing
    /// optional locations are valid; a location with the wrong filesystem
    /// kind is isolated to that component and reported as a diagnostic.
    pub fn resolve_components(&mut self, plugin_id: &str) -> Result<ResolvedComponents, String> {
        let entry = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' not found"))?;

        let root = &entry.root;
        let mut resolved = ResolvedComponents::default();

        // Agent Plugins uses fixed portable component locations. Missing
        // locations are valid; the wrong filesystem kind disables only that
        // component type and is reported as a non-fatal diagnostic.
        let skills_dir = root.join("skills");
        if skills_dir.is_dir() {
            resolved.skill_dirs.push(skills_dir);
        } else if skills_dir.exists() {
            resolved.diagnostics.push(format!(
                "Plugin '{plugin_id}' standard skills path '{}' is not a directory",
                skills_dir.display()
            ));
        }

        let agents_dir = root.join("agents");
        if agents_dir.is_dir() {
            match std::fs::read_dir(&agents_dir) {
                Ok(entries) => {
                    resolved.agent_files.extend(entries.filter_map(|entry| {
                        let path = entry.ok()?.path();
                        (path.is_file() && path.extension().is_some_and(|value| value == "md"))
                            .then_some(path)
                    }));
                    resolved.agent_files.sort();
                }
                Err(error) => resolved.diagnostics.push(format!(
                    "Plugin '{plugin_id}' could not scan agents path '{}': {error}",
                    agents_dir.display()
                )),
            }
        } else if agents_dir.exists() {
            resolved.diagnostics.push(format!(
                "Plugin '{plugin_id}' agents path '{}' is not a directory",
                agents_dir.display()
            ));
        }

        let hooks_file = root.join("hooks/hooks.yaml");
        if hooks_file.is_file() {
            resolved.hooks_file = Some(hooks_file);
        } else if hooks_file.exists() {
            resolved.diagnostics.push(format!(
                "Plugin '{plugin_id}' hooks path '{}' is not a regular file",
                hooks_file.display()
            ));
        }

        let mcp_file = root.join("mcp.json");
        if mcp_file.is_file() {
            resolved.mcp_config_file = Some(mcp_file);
        } else if mcp_file.exists() {
            resolved.diagnostics.push(format!(
                "Plugin '{plugin_id}' standard MCP path '{}' is not a regular file",
                mcp_file.display()
            ));
        }

        let lsp_file = root.join("lsp.yaml");
        if lsp_file.is_file() {
            resolved.lsp_config_file = Some(lsp_file);
        } else if lsp_file.exists() {
            resolved.diagnostics.push(format!(
                "Plugin '{plugin_id}' LSP path '{}' is not a regular file",
                lsp_file.display()
            ));
        }

        // Store resolved components back
        if let Some(entry) = self.plugins.get_mut(plugin_id) {
            entry.resolved_components = Some(resolved.clone());
        }

        Ok(resolved)
    }

    // ── Dependency Resolution ──────────────────────────────────────────

    /// Resolve plugin dependencies via topological sort.
    ///
    /// Returns plugin IDs in dependency order (dependencies first).
    /// Returns an error if there are circular dependencies or missing deps.
    pub fn resolve_dependencies(&self) -> Result<Vec<PluginId>, String> {
        self.resolve_dependencies_matching(|_| true)
    }

    /// Resolve only enabled plugins and require every dependency of an enabled
    /// plugin to be enabled as well. Disabled plugins cannot poison startup,
    /// and an enabled plugin cannot silently consume a disabled dependency.
    pub fn resolve_enabled_dependencies(&self) -> Result<Vec<PluginId>, String> {
        self.resolve_dependencies_matching(|entry| entry.enabled)
    }

    fn resolve_dependencies_matching(
        &self,
        include: impl Fn(&PluginEntry) -> bool,
    ) -> Result<Vec<PluginId>, String> {
        let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();

        // Initialize
        for (id, entry) in &self.plugins {
            if !include(entry) {
                continue;
            }
            graph.entry(id.as_str()).or_default();
            in_degree.entry(id.as_str()).or_insert(0);
        }

        // Build edges + enforce version constraints
        for (id, entry) in &self.plugins {
            if !include(entry) {
                continue;
            }
            for dep in &entry.manifest.dependencies {
                let dep_name = dep.name();
                let dep_entry = self.plugins.get(dep_name).ok_or_else(|| {
                    format!("Plugin '{id}' depends on '{dep_name}' which is not installed")
                })?;
                if !include(dep_entry) {
                    return Err(format!(
                        "Plugin '{id}' depends on '{dep_name}' which is disabled"
                    ));
                }
                // Enforce the declared version constraint (P1 — previously the
                // name-exists check ignored version entirely, so any version
                // satisfied the dependency).
                match dep.satisfies(dep_entry.manifest.version.as_deref()) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(format!(
                            "Plugin '{id}' requires '{dep_name} {}' but installed version is '{}'",
                            dep.version_constraint().unwrap_or("any"),
                            dep_entry.manifest.version_label()
                        ));
                    }
                    Err(e) => {
                        return Err(format!(
                            "Plugin '{id}' dependency '{dep_name}' version check failed: {e}"
                        ));
                    }
                }
                graph.entry(dep_name).or_default().push(id.as_str());
                *in_degree.entry(id.as_str()).or_insert(0) += 1;
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();
        queue.sort_by(|left, right| right.cmp(left));

        let mut sorted = Vec::new();
        while let Some(node) = queue.pop() {
            sorted.push(node.to_string());
            if let Some(neighbors) = graph.get(node) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(neighbor);
                            queue.sort_by(|left, right| right.cmp(left));
                        }
                    }
                }
            }
        }

        if sorted.len() != in_degree.len() {
            return Err("Circular dependency detected among plugins".to_string());
        }

        Ok(sorted)
    }

    // ── Persistence ────────────────────────────────────────────────────

    /// Save the current enabled/disabled state to disk.
    fn save_state(&self) -> Result<(), String> {
        let state = RegistryState {
            plugins: self
                .plugins
                .iter()
                .map(|(id, entry)| {
                    (
                        id.clone(),
                        PluginEntry {
                            resolved_components: None,
                            ..entry.clone()
                        },
                    )
                })
                .collect(),
        };

        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create plugin state directory: {error}"))?;
        }
        let json = serde_json::to_string_pretty(&state)
            .map_err(|error| format!("Failed to serialize plugin state: {error}"))?;
        let temporary = self.state_file.with_extension("json.tmp");
        std::fs::write(&temporary, json)
            .map_err(|error| format!("Failed to write plugin state: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&temporary, permissions)
                .map_err(|error| format!("Failed to protect plugin state: {error}"))?;
        }
        std::fs::rename(&temporary, &self.state_file)
            .map_err(|error| format!("Failed to replace plugin state: {error}"))
    }

    /// Load persisted state and merge with discovered plugins.
    fn load_state(&mut self) -> std::io::Result<()> {
        let state: RegistryState = match std::fs::read_to_string(&self.state_file) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(state) => state,
                Err(error) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "invalid plugin registry state {}: {error}",
                            self.state_file.display()
                        ),
                    ));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };

        // Merge enabled/disabled state and validated config from persisted file.
        for (id, saved_entry) in &state.plugins {
            if let Some(entry) = self.plugins.get_mut(id) {
                match entry.manifest.resolve_user_config(&saved_entry.user_config) {
                    Ok(config) => {
                        entry.user_config = config;
                        entry.enabled = saved_entry.enabled;
                    }
                    Err(errors) => {
                        entry.enabled = false;
                        tracing::warn!(
                            plugin = %id,
                            errors = %errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
                            "Plugin disabled because its persisted configuration is invalid"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Get the persistent data directory for a plugin.
    pub fn data_dir_for(&self, plugin_id: &str) -> PathBuf {
        PluginEntry::data_dir_for(plugin_id, &self.data_dir)
    }
}

impl PluginEntry {
    /// Compute the data directory path for a plugin.
    fn data_dir_for(name: &str, base_data_dir: &Path) -> PathBuf {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        base_data_dir.join(sanitized)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            // Skip hidden directories like .git
            if entry.file_name().to_string_lossy().starts_with(".git") {
                continue;
            }
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::AGENT_PLUGIN_SCHEMA_V1;

    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "echo-agent-plugin-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn create_plugin(
        parent: &Path,
        name: &str,
        dependencies: serde_json::Value,
    ) -> Result<PathBuf, String> {
        let root = parent.join(name);
        std::fs::create_dir_all(root.join("skills/example")).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(root.join("agents")).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(root.join("hooks")).map_err(|error| error.to_string())?;
        std::fs::write(
            root.join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: Example skill\n---\nUse it.\n",
        )
        .map_err(|error| error.to_string())?;
        std::fs::write(
            root.join("agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews changes\n---\nReview carefully.\n",
        )
        .map_err(|error| error.to_string())?;
        std::fs::write(root.join("hooks/hooks.yaml"), "{}\n").map_err(|error| error.to_string())?;
        std::fs::write(root.join("lsp.yaml"), "languages: {}\n")
            .map_err(|error| error.to_string())?;
        std::fs::write(
            root.join("mcp.json"),
            "{\"$schema\":\"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json\",\"mcpServers\":{}}",
        )
        .map_err(|error| error.to_string())?;
        let manifest = serde_json::json!({
            "$schema": AGENT_PLUGIN_SCHEMA_V1,
            "name": name,
            "version": "1.0.0",
            "description": "Test plugin",
            "config": {
                "endpoint": {
                    "type": "string",
                    "title": "Endpoint",
                    "default": "https://example.com"
                }
            },
            "dependencies": dependencies
        });
        let manifest_text =
            serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
        std::fs::write(root.join("plugin.json"), manifest_text)
            .map_err(|error| error.to_string())?;
        Ok(root)
    }

    #[test]
    fn validates_and_resolves_flat_components() -> Result<(), String> {
        let temporary = temporary_root("resolve");
        let plugin = create_plugin(&temporary, "resolve.test", serde_json::json!([]))?;
        let (manifest, resolved) =
            PluginRegistry::validate_plugin_dir(&plugin).map_err(|errors| errors.join("; "))?;
        assert_eq!(manifest.name, "resolve.test");
        assert_eq!(resolved.skill_dirs, vec![plugin.join("skills")]);
        assert_eq!(resolved.mcp_config_file, Some(plugin.join("mcp.json")));
        assert_eq!(resolved.agent_files.len(), 1);
        assert_eq!(resolved.hooks_file, Some(plugin.join("hooks/hooks.yaml")));
        assert!(resolved.diagnostics.is_empty());
        std::fs::remove_dir_all(temporary).map_err(|error| error.to_string())
    }

    #[test]
    fn fixed_component_wrong_kind_is_non_fatal() -> Result<(), String> {
        let temporary = temporary_root("wrong-kind");
        let plugin = create_plugin(&temporary, "wrong.kind", serde_json::json!([]))?;
        std::fs::remove_dir_all(plugin.join("skills")).map_err(|error| error.to_string())?;
        std::fs::write(plugin.join("skills"), "not a directory")
            .map_err(|error| error.to_string())?;
        let (_, resolved) =
            PluginRegistry::validate_plugin_dir(&plugin).map_err(|errors| errors.join("; "))?;
        assert!(resolved.skill_dirs.is_empty());
        assert_eq!(resolved.diagnostics.len(), 1);
        std::fs::remove_dir_all(temporary).map_err(|error| error.to_string())
    }

    #[test]
    fn scans_root_plugin_json_and_applies_configuration_defaults() -> Result<(), String> {
        let temporary = temporary_root("scan");
        let plugins = temporary.join("plugins");
        create_plugin(&plugins, "scan.test", serde_json::json!([]))?;
        let mut registry = PluginRegistry::with_paths(
            temporary.join("registry.json"),
            temporary.join("data"),
            Some(temporary.clone()),
        );
        assert_eq!(
            registry
                .scan_scope_dir(PluginScope::User, &plugins)
                .map_err(|error| error.to_string())?,
            1
        );
        let entry = registry
            .get("scan.test")
            .ok_or_else(|| "scan.test was not discovered".to_string())?;
        assert_eq!(
            entry
                .user_config
                .get("endpoint")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com")
        );
        std::fs::remove_dir_all(temporary).map_err(|error| error.to_string())
    }

    #[test]
    fn resolves_plugin_dependencies_in_version_order() -> Result<(), String> {
        let temporary = temporary_root("dependencies");
        let plugins = temporary.join("plugins");
        create_plugin(&plugins, "base.tools", serde_json::json!([]))?;
        create_plugin(
            &plugins,
            "consumer.tools",
            serde_json::json!([{"name":"base.tools","version":">=1.0.0"}]),
        )?;
        let mut registry = PluginRegistry::with_paths(
            temporary.join("registry.json"),
            temporary.join("data"),
            Some(temporary.clone()),
        );
        registry
            .scan_scope_dir(PluginScope::User, &plugins)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            registry.resolve_enabled_dependencies()?,
            vec!["base.tools".to_string(), "consumer.tools".to_string()]
        );
        std::fs::remove_dir_all(temporary).map_err(|error| error.to_string())
    }

    #[test]
    fn install_rejects_the_removed_yaml_layout() -> Result<(), String> {
        let temporary = temporary_root("legacy");
        let source = temporary.join("legacy");
        std::fs::create_dir_all(source.join(".echo-plugin")).map_err(|error| error.to_string())?;
        std::fs::write(source.join(".echo-plugin/manifest.yaml"), "name: legacy\n")
            .map_err(|error| error.to_string())?;
        let mut registry = PluginRegistry::with_paths(
            temporary.join("registry.json"),
            temporary.join("data"),
            Some(temporary.clone()),
        );
        let error = registry
            .install(&InstallSource::Local(source), PluginScope::Local)
            .err()
            .ok_or_else(|| "legacy layout unexpectedly installed".to_string())?;
        assert!(error.contains("plugin.json"));
        std::fs::remove_dir_all(temporary).map_err(|error| error.to_string())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────
