use super::{PluginRegistry, PluginVariables};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Severity of a preparation diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDiagnosticSeverity {
    Warning,
    Error,
}

/// Structured diagnostic captured while preparing a plugin generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPreparationDiagnostic {
    plugin_id: Option<String>,
    component: String,
    severity: PluginDiagnosticSeverity,
    path: Option<PathBuf>,
    message: String,
}

impl PluginPreparationDiagnostic {
    pub fn plugin_id(&self) -> Option<&str> {
        self.plugin_id.as_deref()
    }

    pub fn component(&self) -> &str {
        &self.component
    }

    pub fn severity(&self) -> PluginDiagnosticSeverity {
        self.severity
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PluginPreparationDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(plugin_id) = self.plugin_id() {
            write!(formatter, "plugin '{plugin_id}' {}: ", self.component)?;
        } else {
            write!(formatter, "{}: ", self.component)?;
        }
        if let Some(path) = self.path() {
            write!(formatter, "{}: ", path.display())?;
        }
        formatter.write_str(&self.message)
    }
}

/// Frozen Skill parsed from one package read.
#[derive(Debug, Clone)]
pub struct PreparedPluginSkill {
    descriptor: crate::skills::external::SkillDescriptor,
    legacy_instructions: Option<String>,
    document: String,
}

impl PreparedPluginSkill {
    pub fn descriptor(&self) -> &crate::skills::external::SkillDescriptor {
        &self.descriptor
    }

    pub fn legacy_instructions(&self) -> Option<&str> {
        self.legacy_instructions.as_deref()
    }

    pub fn document(&self) -> &str {
        &self.document
    }
}

/// Owner-preserving frozen application document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPluginDocument {
    plugin_id: String,
    source_path: PathBuf,
    contents: String,
}

impl PreparedPluginDocument {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// One dependency-ordered prepared plugin.
#[derive(Debug, Clone)]
pub struct PreparedPlugin {
    id: String,
    variables: PluginVariables,
    skills: Vec<PreparedPluginSkill>,
    hooks: Option<echo_execution::skills::hooks::HooksDefinition>,
    #[cfg(feature = "mcp")]
    mcp: Option<crate::mcp::McpConfigFile>,
    subagent_documents: Vec<PreparedPluginDocument>,
    lsp_document: Option<PreparedPluginDocument>,
}

impl PreparedPlugin {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.variables.plugin_root
    }

    pub fn variables(&self) -> &PluginVariables {
        &self.variables
    }

    pub fn skills(&self) -> &[PreparedPluginSkill] {
        &self.skills
    }

    pub fn hooks(&self) -> Option<&echo_execution::skills::hooks::HooksDefinition> {
        self.hooks.as_ref()
    }

    #[cfg(feature = "mcp")]
    pub fn mcp(&self) -> Option<&crate::mcp::McpConfigFile> {
        self.mcp.as_ref()
    }

    pub fn subagent_documents(&self) -> &[PreparedPluginDocument] {
        &self.subagent_documents
    }

    pub fn lsp_document(&self) -> Option<&PreparedPluginDocument> {
        self.lsp_document.as_ref()
    }
}

/// Immutable output of one complete preparation generation.
#[derive(Debug, Clone)]
pub struct PreparedPluginSet {
    generation: u64,
    identity: String,
    plugins: Vec<PreparedPlugin>,
    diagnostics: Vec<PluginPreparationDiagnostic>,
    applicable: bool,
}

impl PreparedPluginSet {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn plugins(&self) -> &[PreparedPlugin] {
        &self.plugins
    }

    pub fn diagnostics(&self) -> &[PluginPreparationDiagnostic] {
        &self.diagnostics
    }

    pub fn is_applicable(&self) -> bool {
        self.applicable
    }
}

#[derive(Default)]
struct PluginPreparationCache {
    sets: HashMap<(String, u64), Arc<PreparedPluginSet>>,
    next_generation: u64,
}

/// Successfully applied framework components grouped by plugin owner.
#[derive(Debug, Clone, Default)]
pub struct WiredPluginComponents {
    pub skills: Vec<String>,
    pub hooks_registered: bool,
    pub mcp_servers: Vec<String>,
}

/// Apply receipt. It contains no package inventory and owns no reload policy.
#[derive(Debug, Default)]
pub struct PluginWiringResult {
    pub plugins_loaded: Vec<String>,
    pub skills_loaded: Vec<String>,
    pub hooks_registered: Vec<String>,
    pub mcp_connected: Vec<String>,
    pub agents_discovered: Vec<String>,
    pub lsp_discovered: Vec<String>,
    pub warnings: Vec<String>,
    pub components_by_plugin: HashMap<String, WiredPluginComponents>,
}

impl PluginWiringResult {
    pub fn is_ok(&self) -> bool {
        true
    }

    pub fn total_wired(&self) -> usize {
        self.skills_loaded.len() + self.hooks_registered.len() + self.mcp_connected.len()
    }
}

/// Typed refusal or atomic apply failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginWiringError {
    InvalidPreparedSet {
        generation: u64,
    },
    ApplyFailed {
        generation: u64,
        diagnostics: String,
    },
}

impl fmt::Display for PluginWiringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPreparedSet { generation } => write!(
                formatter,
                "prepared plugin generation {generation} is not applicable"
            ),
            Self::ApplyFailed {
                generation,
                diagnostics,
            } => write!(
                formatter,
                "plugin generation {generation} failed to apply: {diagnostics}"
            ),
        }
    }
}

impl std::error::Error for PluginWiringError {}

/// Shared preparation cache and zero-read apply manager.
#[derive(Clone)]
pub struct PluginIntegrator {
    cache: Arc<Mutex<PluginPreparationCache>>,
    preparation: Arc<tokio::sync::Mutex<()>>,
}

impl PluginIntegrator {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(PluginPreparationCache::default())),
            preparation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Evict the cached revision. Existing `Arc` generations remain valid.
    pub fn invalidate(&self, registry: &PluginRegistry) {
        if let Ok(mut cache) = self.cache.lock() {
            cache
                .sets
                .retain(|(cache_id, _), _| cache_id != registry.preparation_cache_id());
        }
    }

    /// Capture and parse one immutable generation.
    pub async fn prepare(&self, registry: &mut PluginRegistry) -> Arc<PreparedPluginSet> {
        let _preparation = self.preparation.lock().await;
        let key = (
            registry.preparation_cache_id().to_string(),
            registry.revision(),
        );
        if let Some(cached) = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.sets.get(&key).cloned())
        {
            return cached;
        }

        let mut diagnostics = registry
            .scan_diagnostics()
            .iter()
            .map(|diagnostic| PluginPreparationDiagnostic {
                plugin_id: None,
                component: "manifest".to_string(),
                severity: if diagnostic.is_error {
                    PluginDiagnosticSeverity::Error
                } else {
                    PluginDiagnosticSeverity::Warning
                },
                path: Some(diagnostic.path.clone()),
                message: diagnostic.message.clone(),
            })
            .collect::<Vec<_>>();
        let mut identity = Sha256::new();
        let mut plugins = Vec::new();
        let ordered = match registry.resolve_enabled_dependencies() {
            Ok(ordered) => ordered,
            Err(message) => {
                diagnostics.push(error_diagnostic(None, "dependencies", None, message));
                Vec::new()
            }
        };

        for plugin_id in ordered {
            hash_field(&mut identity, "plugin-id", plugin_id.as_bytes());
            if let Some(entry) = registry.get(&plugin_id) {
                match serde_json::to_vec(&entry.manifest) {
                    Ok(serialized) => hash_field(&mut identity, "manifest", &serialized),
                    Err(error) => diagnostics.push(error_diagnostic(
                        Some(&plugin_id),
                        "manifest",
                        None,
                        error.to_string(),
                    )),
                }
                let ordered_config = entry
                    .user_config
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<BTreeMap<_, _>>();
                match serde_json::to_vec(&ordered_config) {
                    Ok(serialized) => hash_field(&mut identity, "user-config", &serialized),
                    Err(error) => diagnostics.push(error_diagnostic(
                        Some(&plugin_id),
                        "user-config",
                        None,
                        error.to_string(),
                    )),
                }
            }

            let variables = match registry.variables_for(&plugin_id) {
                Ok(variables) => variables,
                Err(message) => {
                    diagnostics.push(error_diagnostic(
                        Some(&plugin_id),
                        "variables",
                        None,
                        message,
                    ));
                    continue;
                }
            };
            hash_variables(&mut identity, &variables);
            if let Err(error) = tokio::fs::create_dir_all(&variables.plugin_data).await {
                diagnostics.push(error_diagnostic(
                    Some(&plugin_id),
                    "data",
                    Some(variables.plugin_data.clone()),
                    error.to_string(),
                ));
                continue;
            }
            let resolved = match registry.resolve_components_async(&plugin_id).await {
                Ok(resolved) => resolved,
                Err(message) => {
                    diagnostics.push(error_diagnostic(
                        Some(&plugin_id),
                        "components",
                        None,
                        message,
                    ));
                    continue;
                }
            };
            diagnostics.extend(
                resolved.diagnostics.iter().cloned().map(|message| {
                    warning_diagnostic(Some(&plugin_id), "components", None, message)
                }),
            );

            let mut skills = Vec::new();
            let mut skill_dirs = resolved.skill_dirs;
            skill_dirs.sort();
            for directory in skill_dirs {
                let mut loader = crate::skills::external::SkillLoader::new()
                    .with_plugin_variables(variables.clone());
                match loader.discover_agent_plugin_skills(&directory).await {
                    Ok(mut descriptors) => {
                        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
                        for descriptor in descriptors {
                            let name = descriptor.name.clone();
                            let Some(document) = loader.get_prepared_document(&name).cloned()
                            else {
                                diagnostics.push(error_diagnostic(
                                    Some(&plugin_id),
                                    "skill",
                                    Some(descriptor.location.clone()),
                                    "prepared Skill document is missing".to_string(),
                                ));
                                continue;
                            };
                            if let Some(inputs) = loader.get_prepared_identity_documents(&name) {
                                for (path, contents) in inputs {
                                    hash_document(
                                        &mut identity,
                                        "skill",
                                        &variables.plugin_root,
                                        path,
                                        contents.as_bytes(),
                                    );
                                }
                            }
                            skills.push(PreparedPluginSkill {
                                legacy_instructions: loader.get_legacy_instructions(&name).cloned(),
                                descriptor,
                                document,
                            });
                        }
                    }
                    Err(error) => diagnostics.push(error_diagnostic(
                        Some(&plugin_id),
                        "skill",
                        Some(directory.clone()),
                        error.to_string(),
                    )),
                }
                diagnostics.extend(loader.discovery_diagnostics().iter().map(|diagnostic| {
                    PluginPreparationDiagnostic {
                        plugin_id: Some(plugin_id.clone()),
                        component: "skill".to_string(),
                        severity: if diagnostic.is_error {
                            PluginDiagnosticSeverity::Error
                        } else {
                            PluginDiagnosticSeverity::Warning
                        },
                        path: Some(diagnostic.path.clone()),
                        message: diagnostic.message.clone(),
                    }
                }));
            }

            let hooks = match resolved.hooks_file {
                Some(path) => {
                    match read_text(&path, &variables, &plugin_id, "hooks", &mut diagnostics).await
                    {
                        Some(contents) => {
                            hash_document(
                                &mut identity,
                                "hooks",
                                &variables.plugin_root,
                                &path,
                                contents.as_bytes(),
                            );
                            match serde_yaml_ng::from_str(&contents) {
                                Ok(hooks) => Some(hooks),
                                Err(error) => {
                                    diagnostics.push(error_diagnostic(
                                        Some(&plugin_id),
                                        "hooks",
                                        Some(path),
                                        error.to_string(),
                                    ));
                                    None
                                }
                            }
                        }
                        None => None,
                    }
                }
                None => None,
            };

            #[cfg(feature = "mcp")]
            let mcp = match resolved.mcp_config_file {
                Some(path) => {
                    match read_text(&path, &variables, &plugin_id, "mcp", &mut diagnostics).await {
                        Some(contents) => {
                            hash_document(
                                &mut identity,
                                "mcp",
                                &variables.plugin_root,
                                &path,
                                contents.as_bytes(),
                            );
                            match crate::mcp::McpConfigFile::parse_agent_plugin(
                                &contents,
                                &variables.plugin_root,
                                &variables.plugin_data,
                            ) {
                                Ok(parsed) => {
                                    diagnostics.extend(parsed.diagnostics.into_iter().map(
                                        |message| {
                                            warning_diagnostic(
                                                Some(&plugin_id),
                                                "mcp",
                                                Some(path.clone()),
                                                message,
                                            )
                                        },
                                    ));
                                    Some(parsed.config)
                                }
                                Err(error) => {
                                    diagnostics.push(error_diagnostic(
                                        Some(&plugin_id),
                                        "mcp",
                                        Some(path),
                                        error.to_string(),
                                    ));
                                    None
                                }
                            }
                        }
                        None => None,
                    }
                }
                None => None,
            };
            #[cfg(not(feature = "mcp"))]
            if let Some(path) = resolved.mcp_config_file {
                if let Some(contents) =
                    read_text(&path, &variables, &plugin_id, "mcp", &mut diagnostics).await
                {
                    hash_document(
                        &mut identity,
                        "mcp",
                        &variables.plugin_root,
                        &path,
                        contents.as_bytes(),
                    );
                    diagnostics.push(warning_diagnostic(
                        Some(&plugin_id),
                        "mcp",
                        Some(path),
                        "MCP component is frozen but cannot be applied without the 'mcp' feature"
                            .to_string(),
                    ));
                }
            }

            let mut subagent_documents = Vec::new();
            for path in resolved.agent_files {
                if let Some(document) =
                    freeze_document(&plugin_id, path, &variables, "subagent", &mut diagnostics)
                        .await
                {
                    hash_document(
                        &mut identity,
                        "subagent",
                        &variables.plugin_root,
                        document.source_path(),
                        document.contents().as_bytes(),
                    );
                    subagent_documents.push(document);
                }
            }
            let lsp_document = match resolved.lsp_config_file {
                Some(path) => {
                    freeze_document(&plugin_id, path, &variables, "lsp", &mut diagnostics).await
                }
                None => None,
            };
            if let Some(document) = lsp_document.as_ref() {
                hash_document(
                    &mut identity,
                    "lsp",
                    &variables.plugin_root,
                    document.source_path(),
                    document.contents().as_bytes(),
                );
            }

            plugins.push(PreparedPlugin {
                id: plugin_id,
                variables,
                skills,
                hooks,
                #[cfg(feature = "mcp")]
                mcp,
                subagent_documents,
                lsp_document,
            });
        }

        let generation = self.next_generation().unwrap_or(u64::MAX);
        if generation == u64::MAX {
            diagnostics.push(error_diagnostic(
                None,
                "generation",
                None,
                "plugin preparation generation exhausted or cache unavailable".to_string(),
            ));
        }
        let applicable = !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == PluginDiagnosticSeverity::Error);
        let prepared = Arc::new(PreparedPluginSet {
            generation,
            identity: format!("{:x}", identity.finalize()),
            plugins,
            diagnostics,
            applicable,
        });
        if let Ok(mut cache) = self.cache.lock() {
            cache.sets.retain(|(cache_id, _), _| cache_id != &key.0);
            cache.sets.insert(key, Arc::clone(&prepared));
        }
        prepared
    }

    fn next_generation(&self) -> Option<u64> {
        let mut cache = self.cache.lock().ok()?;
        let next = cache.next_generation.checked_add(1)?;
        cache.next_generation = next;
        Some(next)
    }

    /// Apply a frozen generation. No package files are read here.
    pub async fn wire_prepared(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        prepared: &PreparedPluginSet,
    ) -> Result<PluginWiringResult, PluginWiringError> {
        if !prepared.is_applicable() {
            return Err(PluginWiringError::InvalidPreparedSet {
                generation: prepared.generation(),
            });
        }
        let mut receipt = PluginWiringResult::default();
        receipt.warnings.extend(
            prepared
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.severity() == PluginDiagnosticSeverity::Warning)
                .map(ToString::to_string),
        );
        let mut errors = Vec::new();

        for plugin in prepared.plugins() {
            let source = format!("plugin:{}", plugin.id());
            match agent
                .register_prepared_plugin_skills(&source, plugin.variables(), plugin.skills())
                .await
            {
                Ok(names) => {
                    receipt.skills_loaded.extend(names.clone());
                    receipt
                        .components_by_plugin
                        .entry(plugin.id().to_string())
                        .or_default()
                        .skills
                        .extend(names);
                }
                Err(error) => errors.push(format!("Plugin '{}' Skills: {error}", plugin.id())),
            }
            if let Some(hooks) = plugin.hooks() {
                let registered = agent.hook_registry().write().await.register_plugin_hooks(
                    plugin.id(),
                    &plugin.variables().plugin_root.display().to_string(),
                    &plugin.variables().plugin_data.display().to_string(),
                    hooks.clone(),
                );
                if registered {
                    receipt.hooks_registered.push(plugin.id().to_string());
                    receipt
                        .components_by_plugin
                        .entry(plugin.id().to_string())
                        .or_default()
                        .hooks_registered = true;
                }
            }
            #[cfg(feature = "mcp")]
            if let Some(config) = plugin.mcp() {
                match agent.load_mcp_config(config.clone()).await {
                    Ok(clients) => {
                        for client in clients {
                            let name = client.server_name().to_string();
                            receipt.mcp_connected.push(name.clone());
                            receipt
                                .components_by_plugin
                                .entry(plugin.id().to_string())
                                .or_default()
                                .mcp_servers
                                .push(name);
                        }
                    }
                    Err(error) => errors.push(format!("Plugin '{}' MCP: {error}", plugin.id())),
                }
            }
            receipt
                .agents_discovered
                .extend(plugin.subagent_documents().iter().map(|document| {
                    format!(
                        "{}:{}",
                        document.plugin_id(),
                        document.source_path().display()
                    )
                }));
            if let Some(document) = plugin.lsp_document() {
                receipt.lsp_discovered.push(format!(
                    "{}:{}",
                    document.plugin_id(),
                    document.source_path().display()
                ));
            }
            if receipt.components_by_plugin.contains_key(plugin.id()) {
                receipt.plugins_loaded.push(plugin.id().to_string());
            }
        }

        if errors.is_empty() {
            Ok(receipt)
        } else {
            self.rollback(agent, &receipt).await;
            Err(PluginWiringError::ApplyFailed {
                generation: prepared.generation(),
                diagnostics: errors.join("; "),
            })
        }
    }

    /// Undo exactly the registrations recorded by one apply receipt.
    pub async fn rollback(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        receipt: &PluginWiringResult,
    ) {
        Self::unwire(agent, &receipt.components_by_plugin).await;
    }

    pub async fn unwire(
        agent: &mut crate::agent::react::ReactAgent,
        components: &HashMap<String, WiredPluginComponents>,
    ) {
        for (plugin_id, owned) in components {
            let source = format!("plugin:{plugin_id}");
            let _ = agent.unregister_skills_by_source(&source).await;
            if owned.hooks_registered {
                agent
                    .hook_registry()
                    .write()
                    .await
                    .unregister(&crate::skills::hooks::HookSource::Plugin(plugin_id.clone()));
            }
            #[cfg(feature = "mcp")]
            for server in &owned.mcp_servers {
                let _ = agent.disconnect_mcp(server).await;
            }
        }
    }
}

impl Default for PluginIntegrator {
    fn default() -> Self {
        Self::new()
    }
}

fn error_diagnostic(
    plugin_id: Option<&str>,
    component: &str,
    path: Option<PathBuf>,
    message: String,
) -> PluginPreparationDiagnostic {
    PluginPreparationDiagnostic {
        plugin_id: plugin_id.map(str::to_string),
        component: component.to_string(),
        severity: PluginDiagnosticSeverity::Error,
        path,
        message,
    }
}

fn warning_diagnostic(
    plugin_id: Option<&str>,
    component: &str,
    path: Option<PathBuf>,
    message: String,
) -> PluginPreparationDiagnostic {
    PluginPreparationDiagnostic {
        plugin_id: plugin_id.map(str::to_string),
        component: component.to_string(),
        severity: PluginDiagnosticSeverity::Warning,
        path,
        message,
    }
}

fn hash_field(hasher: &mut Sha256, tag: &str, contents: &[u8]) {
    hasher.update(u64::try_from(tag.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(tag.as_bytes());
    hasher.update(
        u64::try_from(contents.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(contents);
}

fn hash_variables(hasher: &mut Sha256, variables: &PluginVariables) {
    for (name, value) in variables.user_config.iter().collect::<BTreeMap<_, _>>() {
        hash_field(hasher, "variable-name", name.as_bytes());
        hash_field(hasher, "variable-value", value.as_bytes());
    }
}

fn hash_document(
    hasher: &mut Sha256,
    kind: &str,
    plugin_root: &Path,
    path: &Path,
    contents: &[u8],
) {
    hash_field(hasher, "component-kind", kind.as_bytes());
    let relative = path.strip_prefix(plugin_root).unwrap_or(path);
    hash_field(
        hasher,
        "component-path",
        relative.to_string_lossy().as_bytes(),
    );
    hash_field(hasher, "component-content", contents);
}

async fn read_text(
    path: &Path,
    variables: &PluginVariables,
    plugin_id: &str,
    component: &str,
    diagnostics: &mut Vec<PluginPreparationDiagnostic>,
) -> Option<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Some(variables.substitute(&contents)),
        Err(error) => {
            diagnostics.push(error_diagnostic(
                Some(plugin_id),
                component,
                Some(path.to_path_buf()),
                error.to_string(),
            ));
            None
        }
    }
}

async fn freeze_document(
    plugin_id: &str,
    path: PathBuf,
    variables: &PluginVariables,
    component: &str,
    diagnostics: &mut Vec<PluginPreparationDiagnostic>,
) -> Option<PreparedPluginDocument> {
    read_text(&path, variables, plugin_id, component, diagnostics)
        .await
        .map(|contents| PreparedPluginDocument {
            plugin_id: plugin_id.to_string(),
            source_path: path,
            contents,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{AGENT_PLUGIN_SCHEMA_V1, InstallSource, PluginScope};

    fn create_plugin(
        parent: &Path,
        name: &str,
        dependencies: serde_json::Value,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let root = parent.join(name);
        std::fs::create_dir_all(root.join("skills/example"))?;
        std::fs::create_dir_all(root.join("agents"))?;
        std::fs::create_dir_all(root.join("hooks"))?;
        std::fs::write(
            root.join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: Example\n---\nfirst\n",
        )?;
        std::fs::write(root.join("skills/example/hooks.json"), "{}\n")?;
        std::fs::write(
            root.join("agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews changes\n---\nReview carefully.\n",
        )?;
        std::fs::write(root.join("hooks/hooks.yaml"), "{}\n")?;
        std::fs::write(root.join("lsp.yaml"), "languages: {}\n")?;
        std::fs::write(
            root.join("mcp.json"),
            "{\"$schema\":\"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json\",\"mcpServers\":{}}",
        )?;
        let manifest = serde_json::json!({
            "$schema": AGENT_PLUGIN_SCHEMA_V1,
            "name": name,
            "version": "1.0.0",
            "description": "Prepared plugin test",
            "dependencies": dependencies,
            "config": {
                "endpoint": {
                    "type": "string",
                    "title": "Endpoint",
                    "default": "https://example.com"
                }
            }
        });
        std::fs::write(root.join("plugin.json"), serde_json::to_vec(&manifest)?)?;
        Ok(root)
    }

    fn registry(root: &Path) -> PluginRegistry {
        PluginRegistry::with_paths(
            root.join("registry.json"),
            root.join("data"),
            Some(root.to_path_buf()),
        )
    }

    fn missing(message: &str) -> std::io::Error {
        std::io::Error::other(message.to_string())
    }

    #[tokio::test]
    async fn cache_is_shared_bounded_and_reload_advances_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let shared = integrator.clone();

        let first = integrator.prepare(&mut registry).await;
        let cached = shared.prepare(&mut registry).await;
        assert!(Arc::ptr_eq(&first, &cached));
        integrator.invalidate(&registry);
        let reloaded = integrator.prepare(&mut registry).await;

        assert!(reloaded.generation() > first.generation());
        assert_eq!(reloaded.identity(), first.identity());
        assert_eq!(
            integrator
                .cache
                .lock()
                .map_err(|_| missing("plugin cache lock poisoned"))?
                .sets
                .len(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_shared_integrators_publish_one_arc_per_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let registry = Arc::new(tokio::sync::Mutex::new(registry));
        let integrator = PluginIntegrator::new();
        let first_integrator = integrator.clone();
        let second_integrator = integrator.clone();
        let first_registry = Arc::clone(&registry);
        let second_registry = Arc::clone(&registry);

        let (first, second) = tokio::join!(
            async move {
                let mut registry = first_registry.lock().await;
                first_integrator.prepare(&mut registry).await
            },
            async move {
                let mut registry = second_registry.lock().await;
                second_integrator.prepare(&mut registry).await
            }
        );
        assert!(Arc::ptr_eq(&first, &second));
        Ok(())
    }

    #[tokio::test]
    async fn equivalent_installations_have_the_same_content_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let first_root = tempfile::tempdir()?;
        let second_root = tempfile::tempdir()?;
        let first_source =
            create_plugin(first_root.path(), "equivalent.test", serde_json::json!([]))?;
        let second_source =
            create_plugin(second_root.path(), "equivalent.test", serde_json::json!([]))?;
        let mut first_registry = registry(first_root.path());
        let mut second_registry = registry(second_root.path());
        first_registry.install(&InstallSource::Local(first_source), PluginScope::Local)?;
        second_registry.install(&InstallSource::Local(second_source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();

        let first = integrator.prepare(&mut first_registry).await;
        let second = integrator.prepare(&mut second_registry).await;
        assert_eq!(first.identity(), second.identity());
        assert_ne!(first.generation(), second.generation());
        Ok(())
    }

    #[tokio::test]
    async fn dependency_order_and_every_frozen_component_drive_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let base = create_plugin(temporary.path(), "base.test", serde_json::json!([]))?;
        let consumer = create_plugin(
            temporary.path(),
            "consumer.test",
            serde_json::json!([{"name":"base.test","version":">=1.0.0"}]),
        )?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(base), PluginScope::Local)?;
        registry.install(&InstallSource::Local(consumer), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let first = integrator.prepare(&mut registry).await;
        assert_eq!(
            first
                .plugins()
                .iter()
                .map(PreparedPlugin::id)
                .collect::<Vec<_>>(),
            vec!["base.test", "consumer.test"]
        );

        let root = registry
            .get("consumer.test")
            .ok_or_else(|| missing("consumer plugin missing"))?
            .root
            .clone();
        let mutations = [
            (
                "skills/example/SKILL.md",
                "---\nname: example\ndescription: Example\n---\nsecond\n",
            ),
            ("skills/example/hooks.json", "{ }\n"),
            ("hooks/hooks.yaml", "SessionStart: []\n"),
            (
                "mcp.json",
                "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json\",\n  \"mcpServers\": {}\n}\n",
            ),
            (
                "agents/reviewer.md",
                "---\nname: reviewer\ndescription: Reviews changes\n---\nChanged.\n",
            ),
            ("lsp.yaml", "languages:\n  rust: {}\n"),
        ];
        let mut previous_identity = first.identity().to_string();
        for (relative, contents) in mutations {
            std::fs::write(root.join(relative), contents)?;
            integrator.invalidate(&registry);
            let changed = integrator.prepare(&mut registry).await;
            assert_ne!(changed.identity(), previous_identity, "{relative}");
            previous_identity = changed.identity().to_string();
        }

        let mut config = HashMap::new();
        config.insert(
            "endpoint".to_string(),
            serde_json::Value::String("https://changed.example.com".to_string()),
        );
        registry.configure("consumer.test", config)?;
        let changed_config = integrator.prepare(&mut registry).await;
        assert_ne!(changed_config.identity(), previous_identity);

        let manifest_path = root.join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        manifest
            .as_object_mut()
            .ok_or_else(|| missing("plugin manifest is not an object"))?
            .insert(
                "description".to_string(),
                serde_json::Value::String("Changed manifest".to_string()),
            );
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        registry.scan_scopes(&[PluginScope::Local])?;
        let changed_manifest = integrator.prepare(&mut registry).await;
        assert_ne!(changed_manifest.identity(), changed_config.identity());
        Ok(())
    }

    #[tokio::test]
    async fn parse_error_is_structured_and_non_applicable() -> Result<(), Box<dyn std::error::Error>>
    {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "invalid.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        let plugin_id = registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let hooks = registry
            .get(&plugin_id)
            .ok_or_else(|| missing("invalid plugin missing"))?
            .root
            .join("hooks/hooks.yaml");
        std::fs::write(&hooks, "not: [valid\n")?;
        let prepared = PluginIntegrator::new().prepare(&mut registry).await;

        assert!(!prepared.is_applicable());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("invalid.test")
                && diagnostic.component() == "hooks"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic.path() == Some(hooks.as_path())
        }));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_dependency_generation_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let base = create_plugin(temporary.path(), "base.test", serde_json::json!([]))?;
        let consumer = create_plugin(
            temporary.path(),
            "consumer.test",
            serde_json::json!([{"name":"base.test","version":">=1.0.0"}]),
        )?;
        let mut registry = registry(temporary.path());
        let base_id = registry.install(&InstallSource::Local(base), PluginScope::Local)?;
        registry.install(&InstallSource::Local(consumer), PluginScope::Local)?;
        let base_root = registry
            .get(&base_id)
            .ok_or_else(|| missing("base plugin missing"))?
            .root
            .clone();
        std::fs::remove_dir_all(base_root)?;
        registry.scan_scopes(&[PluginScope::Local])?;

        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        assert!(!prepared.is_applicable());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.component() == "dependencies"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
        }));
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        assert!(matches!(
            integrator.wire_prepared(&mut agent, &prepared).await,
            Err(PluginWiringError::InvalidPreparedSet { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn wire_and_rollback_read_no_component_files() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        let plugin_id = registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        let installed_root = registry
            .get(&plugin_id)
            .ok_or_else(|| missing("installed plugin missing"))?
            .root
            .clone();
        std::fs::remove_dir_all(installed_root)?;

        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let receipt = integrator.wire_prepared(&mut agent, &prepared).await?;
        integrator.rollback(&mut agent, &receipt).await;
        Ok(())
    }
}
