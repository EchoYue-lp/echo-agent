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
    pub fn new(project_root: Option<PathBuf>) -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("~"));

        let base = home.join(".echo-agent");
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
    pub fn with_paths(state_file: PathBuf, data_dir: PathBuf, project_root: Option<PathBuf>) -> Self {
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
        self.plugins.clear();
        let mut total = 0;

        for scope in PluginScope::all() {
            let dir = scope.resolve_dir(self.project_root.as_deref());
            let count = self.scan_scope_dir(*scope, &dir)?;
            total += count;
        }

        // Load persisted enabled/disabled state
        self.load_state();
        Ok(total)
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
                    let id = manifest.name.clone();
                    let entry = PluginEntry {
                        manifest,
                        root: path.clone(),
                        scope,
                        enabled: true, // default; overridden by state
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
            InstallSource::Local(src_path) => self.install_local(src_path, &target_dir),
            InstallSource::Git { url, subdir } => {
                self.install_git(url, subdir.as_deref(), &target_dir)
            }
        }
    }

    fn install_local(&mut self, src: &Path, target_dir: &Path) -> Result<PluginId, String> {
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
                errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")
            ));
        }

        let plugin_id = manifest.name.clone();
        let dest = target_dir.join(&plugin_id);

        if dest.exists() {
            return Err(format!("Plugin '{plugin_id}' is already installed at {}", dest.display()));
        }

        // Copy directory recursively
        copy_dir_recursive(src, &dest)
            .map_err(|e| format!("Failed to copy plugin: {e}"))?;

        let entry = PluginEntry {
            manifest,
            root: dest,
            scope: PluginScope::User, // will be overridden by caller
            enabled: true,
            resolved_components: None,
        };

        self.plugins.insert(plugin_id.clone(), entry);
        self.save_state();
        Ok(plugin_id)
    }

    fn install_git(
        &mut self,
        url: &str,
        subdir: Option<&str>,
        target_dir: &Path,
    ) -> Result<PluginId, String> {
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

        let result = self.install_local(&src, target_dir);

        // Clean up temp directory
        let _ = std::fs::remove_dir_all(&tmp_dir);

        result
    }

    // ── Uninstallation ─────────────────────────────────────────────────

    /// Uninstall a plugin. If `keep_data` is false, the persistent data
    /// directory is also removed.
    pub fn uninstall(&mut self, plugin_id: &str, keep_data: bool) -> Result<(), String> {
        let entry = self
            .plugins
            .remove(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;

        // Remove plugin directory
        if entry.root.exists() {
            std::fs::remove_dir_all(&entry.root)
                .map_err(|e| format!("Failed to remove plugin directory: {e}"))?;
        }

        // Remove data directory unless keeping
        if !keep_data {
            let data = PluginEntry::data_dir_for(plugin_id, &self.data_dir);
            if data.exists() {
                let _ = std::fs::remove_dir_all(&data);
            }
        }

        self.save_state();
        Ok(())
    }

    // ── Enable / Disable ───────────────────────────────────────────────

    /// Enable a disabled plugin.
    pub fn enable(&mut self, plugin_id: &str) -> Result<(), String> {
        let entry = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;

        if entry.enabled {
            return Ok(());
        }

        entry.enabled = true;
        self.save_state();
        Ok(())
    }

    /// Disable an enabled plugin without uninstalling it.
    pub fn disable(&mut self, plugin_id: &str) -> Result<(), String> {
        let entry = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("Plugin '{plugin_id}' is not installed"))?;

        if !entry.enabled {
            return Ok(());
        }

        entry.enabled = false;
        self.save_state();
        Ok(())
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
                    || e.manifest.keywords.iter().any(|k| k.to_lowercase().contains(&q))
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
                }
            }
        }

        // Hooks
        if let Some(ref paths) = manifest.components.hooks {
            if let Some(p) = paths.first() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.hooks_file = Some(path);
                }
            }
        }

        // MCP servers
        if let Some(ref paths) = manifest.components.mcp_servers {
            if let Some(p) = paths.first() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.mcp_config_file = Some(path);
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
        if let Some(ref paths) = manifest.components.lsp_servers {
            if let Some(p) = paths.first() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.lsp_config_file = Some(path);
                }
            }
        }

        // Monitors
        if let Some(ref paths) = manifest.components.monitors {
            if let Some(p) = paths.first() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.monitors_file = Some(path);
                }
            }
        }

        // Themes
        if let Some(ref paths) = manifest.components.themes {
            for p in paths.as_paths() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.theme_files.push(path);
                } else if path.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().is_some_and(|e| e == "json") {
                                resolved.theme_files.push(p);
                            }
                        }
                    }
                }
            }
        }

        // Output styles
        if let Some(ref paths) = manifest.components.output_styles {
            for p in paths.as_paths() {
                let path = resolve_plugin_path(root, p);
                if path.is_file() {
                    resolved.output_style_files.push(path);
                } else if path.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().is_some_and(|e| e == "md") {
                                resolved.output_style_files.push(p);
                            }
                        }
                    }
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
        let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();

        // Initialize
        for id in self.plugins.keys() {
            graph.entry(id.as_str()).or_default();
            in_degree.entry(id.as_str()).or_insert(0);
        }

        // Build edges
        for (id, entry) in &self.plugins {
            for dep in &entry.manifest.dependencies {
                let dep_name = dep.name();
                if !self.plugins.contains_key(dep_name) {
                    return Err(format!(
                        "Plugin '{id}' depends on '{dep_name}' which is not installed"
                    ));
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
        queue.sort(); // deterministic order

        let mut sorted = Vec::new();
        while let Some(node) = queue.pop() {
            sorted.push(node.to_string());
            if let Some(neighbors) = graph.get(node) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(neighbor);
                        }
                    }
                }
            }
        }

        if sorted.len() != self.plugins.len() {
            return Err("Circular dependency detected among plugins".to_string());
        }

        Ok(sorted)
    }

    // ── Persistence ────────────────────────────────────────────────────

    /// Save the current enabled/disabled state to disk.
    fn save_state(&self) {
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
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&self.state_file, json);
        }
    }

    /// Load persisted state and merge with discovered plugins.
    fn load_state(&mut self) {
        let state: RegistryState = match std::fs::read_to_string(&self.state_file) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
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

        let manifest = format!(
            "name: {name}\nversion: \"1.0.0\"\ndescription: \"Test plugin {name}\""
        );
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
        let mut reg = PluginRegistry::with_paths(
            tmp.join("registry.json"),
            tmp.join("data"),
            None,
        );

        // Manually scan the directory
        let count = reg.scan_scope_dir(PluginScope::User, &user_dir).unwrap();
        assert_eq!(count, 2);
        assert_eq!(reg.count(), 2);

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
    fn test_dependency_resolution() {
        let tmp = std::env::temp_dir().join("echo-plugin-test-deps");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut reg = test_registry(&tmp);

        // Create plugins with dependencies: A depends on B, B depends on C
        let yaml_c = "name: plugin-c\nversion: \"1.0.0\"\ndescription: \"C\"";
        let yaml_b = "name: plugin-b\nversion: \"1.0.0\"\ndescription: \"B\"\ndependencies:\n  - plugin-c";
        let yaml_a = "name: plugin-a\nversion: \"1.0.0\"\ndescription: \"A\"\ndependencies:\n  - name: plugin-b\n    version: \">=1.0.0\"";

        for (name, yaml) in [("plugin-c", yaml_c), ("plugin-b", yaml_b), ("plugin-a", yaml_a)] {
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
}
