//! Plugin registry — discovery, installation, lifecycle management.
//!
//! The `PluginRegistry` is the central hub for managing plugins.
//! It handles scanning, installing, uninstalling, enabling/disabling,
//! and dependency resolution.

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
    /// Resolved component paths (absolute, populated at load time).
    #[serde(skip)]
    pub resolved_components: Option<ResolvedComponents>,
}

/// Resolved absolute paths for all plugin components.
///
/// Populated after the manifest is loaded and paths are resolved
/// relative to the plugin root. `None` fields mean the component
/// is not declared in the manifest.
#[derive(Debug, Clone, Default)]
pub struct ResolvedComponents {
    /// Directories containing SKILL.md files.
    pub skill_dirs: Vec<PathBuf>,
    /// Agent definition markdown files.
    pub agent_files: Vec<PathBuf>,
    /// Hook configuration file.
    pub hooks_file: Option<PathBuf>,
    /// MCP server configuration file.
    pub mcp_config_file: Option<PathBuf>,
    /// LSP server configuration file.
    pub lsp_config_file: Option<PathBuf>,
    /// Monitor configuration file.
    pub monitors_file: Option<PathBuf>,
    /// Theme definition files.
    pub theme_files: Vec<PathBuf>,
    /// Output style files.
    pub output_style_files: Vec<PathBuf>,
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
    /// Project root for resolving Project/Local scopes.
    project_root: Option<PathBuf>,
}

impl PluginRegistry {
    /// Create a new registry with the default state file location.
    ///
    /// State and data paths resolve under the configurable plugin base dir
    /// ([`super::plugin_data_base_dir`], default `~/.echo-agent`); applications
    /// override it at startup via [`super::set_plugin_data_base_dir`] so plugin
    /// data co-locates with their brand directory (e.g. `~/.eko`).
    pub fn new(project_root: Option<PathBuf>) -> Self {
        let base = super::plugin_data_base_dir();
        let state_file = base.join("plugins").join("registry.json");
        let data_dir = base.join("plugins").join("data");

        Self {
            plugins: HashMap::new(),
            state_file,
            data_dir,
            project_root,
        }
    }

    /// Create a registry with custom paths (useful for testing).
    pub fn with_paths(
        state_file: PathBuf,
        data_dir: PathBuf,
        project_root: Option<PathBuf>,
    ) -> Self {
        Self {
            plugins: HashMap::new(),
            state_file,
            data_dir,
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
            let dir = scope.resolve_dir(self.project_root.as_deref());
            let count = self.scan_scope_dir(*scope, &dir)?;
            total += count;
        }

        // Load persisted enabled/disabled state
        self.load_state();
        Ok(total)
    }

    /// Validate a plugin directory and resolve every declared component.
    ///
    /// Unlike discovery, validation is strict: an explicitly declared path
    /// must exist. Conventional optional defaults (for example `skills/` when
    /// `components.skills` is omitted) remain optional.
    pub fn validate_plugin_dir(
        root: &Path,
    ) -> Result<(PluginManifest, ResolvedComponents), Vec<String>> {
        let manifest_path = root.join(".echo-plugin").join("manifest.yaml");
        let manifest = PluginManifest::from_file(&manifest_path).map_err(|error| vec![error])?;
        let errors = manifest
            .validate()
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors);
        }

        let plugin_id = manifest.name.clone();
        let mut registry = Self::with_paths(
            root.join(".echo-plugin").join(".validation-state.json"),
            root.join(".echo-plugin").join(".validation-data"),
            root.parent().map(Path::to_path_buf),
        );
        registry.plugins.insert(
            plugin_id.clone(),
            PluginEntry {
                manifest: manifest.clone(),
                root: root.to_path_buf(),
                scope: PluginScope::Local,
                enabled: true,
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
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }

            // Look for manifest at .echo-plugin/manifest.yaml
            let manifest_path = path.join(".echo-plugin").join("manifest.yaml");
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
                    let id = manifest.name.clone();
                    let enabled = manifest.default_enabled;
                    let entry = PluginEntry {
                        manifest,
                        root: path.clone(),
                        scope,
                        enabled, // overridden by persisted state when present
                        resolved_components: None,
                    };
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
        let target_dir = scope.resolve_dir(self.project_root.as_deref());
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
        let manifest_path = src.join(".echo-plugin").join("manifest.yaml");
        if !manifest_path.exists() {
            return Err(format!(
                "Source directory {} does not contain .echo-plugin/manifest.yaml",
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

        // Copy directory recursively
        copy_dir_recursive(src, &dest).map_err(|e| format!("Failed to copy plugin: {e}"))?;

        let enabled = manifest.default_enabled;
        let entry = PluginEntry {
            manifest,
            root: dest.clone(),
            scope,
            enabled,
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
        // EKO is a local user-controlled application. Accept the standard
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

        // Remove plugin directory
        if entry.root.exists() {
            std::fs::remove_dir_all(&entry.root)
                .map_err(|e| format!("Failed to remove plugin directory: {e}"))?;
        }

        // Only remove the in-memory entry after the filesystem operation has
        // succeeded. A failed delete must leave the live registry truthful.
        self.plugins.remove(plugin_id);

        // Remove data directory unless keeping
        if !keep_data {
            let data = PluginEntry::data_dir_for(plugin_id, &self.data_dir);
            if data.exists() {
                let _ = std::fs::remove_dir_all(&data);
            }
        }

        self.save_state().map_err(|error| {
            format!("Plugin files were removed, but registry state could not be saved: {error}")
        })
    }

    // ── Enable / Disable ───────────────────────────────────────────────

    /// Enable a disabled plugin.
    pub fn enable(&mut self, plugin_id: &str) -> Result<(), String> {
        let was_enabled = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?
            .enabled;
        if was_enabled {
            return Ok(());
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
    /// This reads the manifest's component declarations and resolves
    /// relative paths against the plugin root. It also discovers
    /// files within declared directories (e.g., scanning for SKILL.md).
    pub fn resolve_components(&mut self, plugin_id: &str) -> Result<ResolvedComponents, String> {
        let entry = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' not found"))?;

        let root = &entry.root;
        let manifest = &entry.manifest;
        let mut resolved = ResolvedComponents::default();

        // Skills — scan directories for SKILL.md files
        if let Some(ref paths) = manifest.components.skills {
            for p in paths.as_paths() {
                let dir = resolve_plugin_path(root, p);
                if dir.is_dir() {
                    resolved.skill_dirs.push(dir);
                } else {
                    return Err(missing_component_path(plugin_id, "skills", &dir));
                }
            }
        } else {
            // Default: ./skills/
            let default_dir = root.join("skills");
            if default_dir.is_dir() {
                resolved.skill_dirs.push(default_dir);
            }
        }

        // Agents
        if let Some(ref paths) = manifest.components.agents {
            for p in paths.as_paths() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.agent_files.push(path);
                } else if path.is_dir() {
                    // Scan directory for .md files
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().is_some_and(|e| e == "md") {
                                resolved.agent_files.push(p);
                            }
                        }
                    }
                } else {
                    return Err(missing_component_path(plugin_id, "agents", &path));
                }
            }
        }

        // Hooks
        if let Some(ref paths) = manifest.components.hooks
            && let Some(p) = paths.first()
        {
            let path = resolve_plugin_path(root, p);
            if path.is_file() {
                resolved.hooks_file = Some(path);
            } else {
                return Err(missing_component_path(plugin_id, "hooks", &path));
            }
        }

        // MCP servers
        if let Some(ref paths) = manifest.components.mcp_servers {
            if let Some(p) = paths.first() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.mcp_config_file = Some(path);
                } else {
                    return Err(missing_component_path(plugin_id, "mcp_servers", &path));
                }
            }
        } else {
            // Default: .mcp.json
            let default_file = root.join(".mcp.json");
            if default_file.is_file() {
                resolved.mcp_config_file = Some(default_file);
            }
        }

        // LSP servers
        if let Some(ref paths) = manifest.components.lsp_servers
            && let Some(p) = paths.first()
        {
            let path = resolve_plugin_path(root, p);
            if path.is_file() {
                resolved.lsp_config_file = Some(path);
            } else {
                return Err(missing_component_path(plugin_id, "lsp_servers", &path));
            }
        }

        // Monitors
        if let Some(ref paths) = manifest.components.monitors
            && let Some(p) = paths.first()
        {
            let path = resolve_plugin_path(root, p);
            if path.is_file() {
                resolved.monitors_file = Some(path);
            } else {
                return Err(missing_component_path(plugin_id, "monitors", &path));
            }
        }

        // Themes
        if let Some(ref paths) = manifest.components.themes {
            for p in paths.as_paths() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.theme_files.push(path);
                } else if path.is_dir()
                    && let Ok(entries) = std::fs::read_dir(&path)
                {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().is_some_and(|e| e == "json") {
                            resolved.theme_files.push(p);
                        }
                    }
                } else {
                    return Err(missing_component_path(plugin_id, "themes", &path));
                }
            }
        }

        // Output styles
        if let Some(ref paths) = manifest.components.output_styles {
            for p in paths.as_paths() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.output_style_files.push(path);
                } else if path.is_dir()
                    && let Ok(entries) = std::fs::read_dir(&path)
                {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().is_some_and(|e| e == "md") {
                            resolved.output_style_files.push(p);
                        }
                    }
                } else {
                    return Err(missing_component_path(plugin_id, "output_styles", &path));
                }
            }
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
                match dep.satisfies(&dep_entry.manifest.version) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(format!(
                            "Plugin '{id}' requires '{dep_name} {}' but installed version is '{}'",
                            dep.version_constraint().unwrap_or("any"),
                            dep_entry.manifest.version
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
        std::fs::write(&self.state_file, json)
            .map_err(|error| format!("Failed to write plugin state: {error}"))
    }

    /// Load persisted state and merge with discovered plugins.
    fn load_state(&mut self) {
        let state: RegistryState = match std::fs::read_to_string(&self.state_file) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!(
                        path = %self.state_file.display(),
                        %error,
                        "Ignoring invalid plugin registry state"
                    );
                    return;
                }
            },
            Err(_) => return,
        };

        // Merge enabled/disabled state from persisted file
        for (id, saved_entry) in &state.plugins {
            if let Some(entry) = self.plugins.get_mut(id) {
                entry.enabled = saved_entry.enabled;
            }
        }
    }

    /// Get the persistent data directory for a plugin.
    pub fn data_dir_for(&self, plugin_id: &str) -> PathBuf {
        PluginEntry::data_dir_for(plugin_id, &self.data_dir)
    }
}

fn missing_component_path(plugin_id: &str, component: &str, path: &Path) -> String {
    format!(
        "Plugin '{plugin_id}' declares {component} path '{}' but it does not exist",
        path.display()
    )
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

/// Resolve a plugin-relative path to an absolute path.
fn resolve_plugin_path(root: &Path, relative: &str) -> PathBuf {
    let stripped = relative.strip_prefix("./").unwrap_or(relative);
    root.join(stripped)
}

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

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::PluginManifest;

    fn test_registry(tmp: &Path) -> PluginRegistry {
        PluginRegistry::with_paths(
            tmp.join("registry.json"),
            tmp.join("data"),
            Some(tmp.join("project")),
        )
    }

    fn create_test_plugin(dir: &Path, name: &str) -> PathBuf {
        let plugin_dir = dir.join(name);
        let manifest_dir = plugin_dir.join(".echo-plugin");
        std::fs::create_dir_all(&manifest_dir).unwrap();

        let manifest =
            format!("name: {name}\nversion: \"1.0.0\"\ndescription: \"Test plugin {name}\"");
        std::fs::write(manifest_dir.join("manifest.yaml"), manifest).unwrap();

        // Create a skills directory
        let skills_dir = plugin_dir.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        plugin_dir
    }

    #[test]
    fn test_scan_empty_dir() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-scan-empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut reg = test_registry(&tmp);
        let count = reg.scan_scope_dir(PluginScope::Local, &tmp).unwrap();
        assert_eq!(count, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_discovers_plugins() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-scan");
        let _ = std::fs::remove_dir_all(&tmp);

        // Create plugins in user scope
        let user_dir = tmp.join("home").join(".echo-agent").join("plugins");
        std::fs::create_dir_all(&user_dir).unwrap();
        create_test_plugin(&user_dir, "plugin-a");
        create_test_plugin(&user_dir, "plugin-b");

        // We need to override HOME for the test
        // Instead, use with_paths directly
        let mut reg = PluginRegistry::with_paths(tmp.join("registry.json"), tmp.join("data"), None);

        // Manually scan the directory
        let count = reg.scan_scope_dir(PluginScope::User, &user_dir).unwrap();
        assert_eq!(count, 2);
        assert_eq!(reg.count(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_honors_manifest_default_enabled() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-default-disabled");
        let _ = std::fs::remove_dir_all(&tmp);
        let plugin_dir = create_test_plugin(&tmp, "disabled-by-default");
        let manifest = "name: disabled-by-default\nversion: \"1.0.0\"\ndescription: disabled\ndefault_enabled: false";
        std::fs::write(
            plugin_dir.join(".echo-plugin").join("manifest.yaml"),
            manifest,
        )
        .unwrap();

        let mut reg = test_registry(&tmp);
        let count = reg.scan_scope_dir(PluginScope::User, &tmp).unwrap();

        assert_eq!(count, 1);
        assert!(
            reg.get("disabled-by-default")
                .is_some_and(|entry| !entry.enabled)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_skips_invalid_manifest() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-scan-invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        let manifest_dir = tmp.join("bad-plugin").join(".echo-plugin");
        let create_result = std::fs::create_dir_all(&manifest_dir);
        assert!(create_result.is_ok(), "failed to create plugin fixture");
        if create_result.is_err() {
            return;
        }
        let write_result = std::fs::write(
            manifest_dir.join("manifest.yaml"),
            "name: ../bad\nversion: not-semver\ndescription: invalid",
        );
        assert!(write_result.is_ok(), "failed to write plugin fixture");
        if write_result.is_err() {
            return;
        }

        let mut reg = test_registry(&tmp);
        let count = reg
            .scan_scope_dir(PluginScope::User, &tmp)
            .unwrap_or_default();
        assert_eq!(count, 0);
        assert!(reg.list().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_local() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-install");
        let _ = std::fs::remove_dir_all(&tmp);

        let src_dir = tmp.join("src-plugin");
        create_test_plugin(&tmp, "src-plugin");

        let target_dir = tmp.join("installed");
        std::fs::create_dir_all(&target_dir).unwrap();

        // Use Local scope with project_root pointing to tmp
        // so install goes to tmp/.echo-agent/plugins.local/
        let mut reg = PluginRegistry::with_paths(
            tmp.join("registry.json"),
            tmp.join("data"),
            Some(tmp.clone()),
        );
        let id = reg
            .install(&InstallSource::Local(src_dir), PluginScope::Local)
            .unwrap();
        assert_eq!(id, "src-plugin");
        assert!(reg.get("src-plugin").is_some());
        assert_eq!(
            reg.get("src-plugin").map(|entry| entry.scope),
            Some(PluginScope::Local)
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enable_disable() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-enable");
        let _ = std::fs::remove_dir_all(&tmp);

        let user_dir = tmp.join("plugins");
        std::fs::create_dir_all(&user_dir).unwrap();
        create_test_plugin(&user_dir, "toggle-me");

        let mut reg = test_registry(&tmp);
        reg.scan_scope_dir(PluginScope::User, &user_dir).unwrap();

        assert!(reg.get("toggle-me").unwrap().enabled);

        reg.disable("toggle-me").unwrap();
        assert!(!reg.get("toggle-me").unwrap().enabled);

        reg.enable("toggle-me").unwrap();
        assert!(reg.get("toggle-me").unwrap().enabled);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_uninstall() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-uninstall");
        let _ = std::fs::remove_dir_all(&tmp);

        let user_dir = tmp.join("plugins");
        std::fs::create_dir_all(&user_dir).unwrap();
        create_test_plugin(&user_dir, "remove-me");

        let mut reg = test_registry(&tmp);
        reg.scan_scope_dir(PluginScope::User, &user_dir).unwrap();
        assert_eq!(reg.count(), 1);

        reg.uninstall("remove-me", false).unwrap();
        assert_eq!(reg.count(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_uninstall_keeps_registry_entry_when_directory_delete_fails() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-uninstall-failure");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let root_file = tmp.join("not-a-directory");
        std::fs::write(&root_file, "plugin").unwrap();
        let manifest =
            PluginManifest::from_yaml("name: remove-me\nversion: \"1.0.0\"\ndescription: test")
                .unwrap();

        let mut reg = test_registry(&tmp);
        reg.plugins.insert(
            "remove-me".to_string(),
            PluginEntry {
                manifest,
                root: root_file,
                scope: PluginScope::User,
                enabled: true,
                resolved_components: None,
            },
        );

        assert!(reg.uninstall("remove-me", true).is_err());
        assert!(reg.get("remove-me").is_some());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_dependency_resolution() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-deps");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut reg = test_registry(&tmp);

        // Create plugins with dependencies: A depends on B, B depends on C
        let yaml_c = "name: plugin-c\nversion: \"1.0.0\"\ndescription: \"C\"";
        let yaml_b =
            "name: plugin-b\nversion: \"1.0.0\"\ndescription: \"B\"\ndependencies:\n  - plugin-c";
        let yaml_a = "name: plugin-a\nversion: \"1.0.0\"\ndescription: \"A\"\ndependencies:\n  - name: plugin-b\n    version: \">=1.0.0\"";

        for (name, yaml) in [
            ("plugin-c", yaml_c),
            ("plugin-b", yaml_b),
            ("plugin-a", yaml_a),
        ] {
            let manifest: PluginManifest = PluginManifest::from_yaml(yaml).unwrap();
            reg.plugins.insert(
                name.to_string(),
                PluginEntry {
                    manifest,
                    root: tmp.join(name),
                    scope: PluginScope::User,
                    enabled: true,
                    resolved_components: None,
                },
            );
        }

        let sorted = reg.resolve_dependencies().unwrap();
        // C should come before B, B before A
        let pos_a = sorted.iter().position(|x| x == "plugin-a").unwrap();
        let pos_b = sorted.iter().position(|x| x == "plugin-b").unwrap();
        let pos_c = sorted.iter().position(|x| x == "plugin-c").unwrap();
        assert!(pos_c < pos_b);
        assert!(pos_b < pos_a);

        let disable_error = reg.disable("plugin-b").err().unwrap_or_default();
        assert!(disable_error.contains("plugin-a"));
        assert!(reg.get("plugin-b").is_some_and(|entry| entry.enabled));
        let uninstall_error = reg.uninstall("plugin-b", true).err().unwrap_or_default();
        assert!(uninstall_error.contains("plugin-a"));
        assert!(reg.get("plugin-b").is_some());

        if let Some(entry) = reg.plugins.get_mut("plugin-a") {
            entry.enabled = false;
        }
        let enabled = reg.resolve_enabled_dependencies().unwrap_or_default();
        assert!(!enabled.iter().any(|id| id == "plugin-a"));
        assert!(enabled.iter().any(|id| id == "plugin-b"));

        if let Some(entry) = reg.plugins.get_mut("plugin-a") {
            entry.enabled = true;
        }
        if let Some(entry) = reg.plugins.get_mut("plugin-b") {
            entry.enabled = false;
        }
        let dependency_error = reg.resolve_enabled_dependencies().err().unwrap_or_default();
        assert!(dependency_error.contains("plugin-b"));
        assert!(dependency_error.contains("disabled"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_circular_dependency_detected() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-circular");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut reg = test_registry(&tmp);

        let yaml_a = "name: a\nversion: \"1.0.0\"\ndescription: \"A\"\ndependencies:\n  - b";
        let yaml_b = "name: b\nversion: \"1.0.0\"\ndescription: \"B\"\ndependencies:\n  - a";

        for (name, yaml) in [("a", yaml_a), ("b", yaml_b)] {
            let manifest = PluginManifest::from_yaml(yaml).unwrap();
            reg.plugins.insert(
                name.to_string(),
                PluginEntry {
                    manifest,
                    root: tmp.join(name),
                    scope: PluginScope::User,
                    enabled: true,
                    resolved_components: None,
                },
            );
        }

        assert!(reg.resolve_dependencies().is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_search() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-search");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut reg = test_registry(&tmp);

        let yaml = r#"
name: data-analysis
version: "1.0.0"
description: "Data analysis with polars"
keywords: [data, polars, visualization]
"#;
        let manifest = PluginManifest::from_yaml(yaml).unwrap();
        reg.plugins.insert(
            "data-analysis".to_string(),
            PluginEntry {
                manifest,
                root: tmp.join("data-analysis"),
                scope: PluginScope::User,
                enabled: true,
                resolved_components: None,
            },
        );

        assert_eq!(reg.search("polars").len(), 1);
        assert_eq!(reg.search("data").len(), 1);
        assert_eq!(reg.search("nothing").len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_components() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-resolve");
        let _ = std::fs::remove_dir_all(&tmp);

        let user_dir = tmp.join("plugins");
        let plugin_dir = create_test_plugin(&user_dir, "resolve-test");

        // Create a hooks file
        let hooks_dir = plugin_dir.join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("hooks.yaml"), "hooks: {}").unwrap();

        // Create an MCP config
        std::fs::write(plugin_dir.join(".mcp.json"), "{}").unwrap();

        // Update manifest to include components
        let manifest_dir = plugin_dir.join(".echo-plugin");
        let manifest = r#"
name: resolve-test
version: "1.0.0"
description: "Test"
components:
  skills: "./skills/"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"
"#;
        std::fs::write(manifest_dir.join("manifest.yaml"), manifest).unwrap();

        let mut reg = test_registry(&tmp);
        reg.scan_scope_dir(PluginScope::User, &user_dir).unwrap();

        let resolved = reg.resolve_components("resolve-test").unwrap();
        assert_eq!(resolved.skill_dirs.len(), 1);
        assert!(resolved.hooks_file.is_some());
        assert!(resolved.mcp_config_file.is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn validate_plugin_dir_rejects_declared_missing_component() -> Result<(), String> {
        let tmp = std::env::temp_dir().join(format!(
            "echo-plugin-validate-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".echo-plugin")).map_err(|error| error.to_string())?;
        std::fs::write(
            tmp.join(".echo-plugin/manifest.yaml"),
            "name: strict-validation\nversion: \"1.0.0\"\ndescription: Test\ncomponents:\n  skills: ./missing-skills\n",
        )
        .map_err(|error| error.to_string())?;

        let errors = PluginRegistry::validate_plugin_dir(&tmp)
            .err()
            .ok_or_else(|| "declared missing component unexpectedly validated".to_string())?;
        assert!(
            errors
                .iter()
                .any(|error| { error.contains("skills") && error.contains("missing-skills") })
        );
        std::fs::remove_dir_all(&tmp).map_err(|error| error.to_string())?;
        Ok(())
    }
}
