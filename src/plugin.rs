//! Unified Plugin trait — common interface for Skills, Hooks, MCP, Channels.
//!
//! All extension points implement this trait, enabling hot-load from
//! `~/.echo-agent/plugins/<name>/manifest.toml`.

use std::path::PathBuf;

/// What a plugin provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginCapability {
    Skill,
    Hook,
    McpServer,
    Channel,
    Tool,
    Custom,
}

/// Plugin manifest (loaded from manifest.toml).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub entry_point: Option<String>,
    pub dependencies: Vec<String>,
}

/// Unified plugin trait.
pub trait Plugin: Send + Sync {
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

/// Registry of loaded plugins.
pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
    /// name → index
    by_name: std::collections::HashMap<String, usize>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            by_name: std::collections::HashMap::new(),
        }
    }

    /// Register a plugin.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        let idx = self.plugins.len();
        self.by_name.insert(plugin.id().to_string(), idx);
        self.plugins.push(plugin);
    }

    /// Scan a directory for plugin manifests (manifest.toml or manifest.json).
    /// NOTE: entry_point loading is not yet implemented — only manifest metadata is registered.
    /// Returns count of discovered plugins.
    pub fn scan_dir(&mut self, dir: &PathBuf) -> std::io::Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            // Read manifest.json (manifest.toml support pending toml dependency)
            let manifest = if path.join("manifest.json").exists() {
                let content = std::fs::read_to_string(path.join("manifest.json"))?;
                serde_json::from_str::<PluginManifest>(&content).ok()
            } else {
                continue;
            };

            if let Some(manifest) = manifest {
                self.register(Box::new(RegisteredPlugin::new(manifest)));
                count += 1;
            }
        }
        Ok(count)
    }

    /// Get a plugin by ID.
    pub fn get(&self, id: &str) -> Option<&dyn Plugin> {
        self.by_name.get(id).map(|&i| self.plugins[i].as_ref())
    }

    /// List all registered plugins.
    pub fn list(&self) -> Vec<&dyn Plugin> {
        self.plugins.iter().map(|p| p.as_ref()).collect()
    }

    /// Initialize all plugins.
    pub fn init_all(&mut self) -> Vec<Result<(), String>> {
        self.plugins.iter_mut().map(|p| p.init()).collect()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A plugin loaded from a manifest file (minimal implementation).
struct RegisteredPlugin {
    manifest: PluginManifest,
}

impl RegisteredPlugin {
    fn new(manifest: PluginManifest) -> Self {
        Self { manifest }
    }
}

impl Plugin for RegisteredPlugin {
    fn id(&self) -> &str {
        &self.manifest.name
    }
    fn name(&self) -> &str {
        &self.manifest.name
    }
    fn capabilities(&self) -> Vec<PluginCapability> {
        self.manifest
            .capabilities
            .iter()
            .map(|c| match c.as_str() {
                "skill" => PluginCapability::Skill,
                "hook" => PluginCapability::Hook,
                "mcp" => PluginCapability::McpServer,
                "channel" => PluginCapability::Channel,
                "tool" => PluginCapability::Tool,
                _ => PluginCapability::Custom,
            })
            .collect()
    }
    fn version(&self) -> &str {
        &self.manifest.version
    }
}
