use super::{PluginRegistry, PluginVariables};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// Prepared ordinals are ordered across independent Integrators in one process.
static NEXT_PREPARED_GENERATION: AtomicU64 = AtomicU64::new(0);

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
    document: crate::skills::external::SkillDocument,
}

impl PreparedPluginSkill {
    pub fn document(&self) -> &crate::skills::external::SkillDocument {
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
///
/// Error diagnostics may describe components excluded from an otherwise
/// applicable generation. Applicability is reserved for generation-wide
/// invariants required to publish this snapshot atomically.
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
}

/// Successfully applied framework components grouped by plugin owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WiredPluginComponents {
    pub skills: Vec<String>,
    pub hooks_registered: bool,
    pub mcp_servers: Vec<String>,
    #[cfg(feature = "mcp")]
    pub mcp_server_ids: Vec<crate::mcp::McpServerId>,
}

/// Apply receipt. It contains no package inventory and owns no reload policy.
#[derive(Debug, Clone, Default)]
pub struct PluginWiringResult {
    generation: u64,
    identity: String,
    publication_token: Option<Arc<()>>,
    pub plugins_loaded: Vec<String>,
    pub skills_loaded: Vec<String>,
    pub hooks_registered: Vec<String>,
    pub mcp_connected: Vec<String>,
    #[cfg(feature = "mcp")]
    pub mcp_connected_ids: Vec<crate::mcp::McpServerId>,
    pub agents_discovered: Vec<String>,
    pub lsp_discovered: Vec<String>,
    pub warnings: Vec<String>,
    pub components_by_plugin: HashMap<String, WiredPluginComponents>,
}

impl PartialEq for PluginWiringResult {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.identity == other.identity
            && match (&self.publication_token, &other.publication_token) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
            && self.plugins_loaded == other.plugins_loaded
            && self.skills_loaded == other.skills_loaded
            && self.hooks_registered == other.hooks_registered
            && self.mcp_connected == other.mcp_connected
            && {
                #[cfg(feature = "mcp")]
                {
                    self.mcp_connected_ids == other.mcp_connected_ids
                }
                #[cfg(not(feature = "mcp"))]
                {
                    true
                }
            }
            && self.agents_discovered == other.agents_discovered
            && self.lsp_discovered == other.lsp_discovered
            && self.warnings == other.warnings
            && self.components_by_plugin == other.components_by_plugin
    }
}

impl Eq for PluginWiringResult {}

impl PluginWiringResult {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn is_ok(&self) -> bool {
        true
    }

    pub fn total_wired(&self) -> usize {
        self.skills_loaded.len() + self.hooks_registered.len() + self.mcp_connected.len()
    }
}

/// Typed refusal or apply failure. An unsettled rollback retains its receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginWiringError {
    InvalidPreparedSet {
        generation: u64,
    },
    StaleGeneration {
        requested: u64,
        latest: u64,
    },
    AlreadyPublished {
        generation: u64,
    },
    ActiveGeneration {
        generation: u64,
    },
    CleanupPending {
        generation: u64,
    },
    StaleReceipt {
        requested: u64,
        latest: u64,
    },
    InvalidReceipt {
        generation: u64,
    },
    WrongTarget,
    ApplyFailed {
        generation: u64,
        diagnostics: String,
    },
    RollbackFailed {
        generation: u64,
        diagnostics: String,
        receipt: Box<PluginWiringResult>,
    },
}

impl PluginWiringError {
    /// Return the partial apply receipt only when cleanup remains unsettled.
    pub fn cleanup_receipt(&self) -> Option<&PluginWiringResult> {
        match self {
            Self::RollbackFailed { receipt, .. } => Some(receipt),
            _ => None,
        }
    }

    /// Transfer an unsettled receipt to the caller's retry/withdraw owner.
    pub fn into_cleanup_receipt(self) -> Option<PluginWiringResult> {
        match self {
            Self::RollbackFailed { receipt, .. } => Some(*receipt),
            _ => None,
        }
    }
}

impl fmt::Display for PluginWiringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPreparedSet { generation } => write!(
                formatter,
                "prepared plugin generation {generation} is not applicable"
            ),
            Self::StaleGeneration { requested, latest } => write!(
                formatter,
                "prepared plugin generation {requested} is stale; target has generation {latest}"
            ),
            Self::AlreadyPublished { generation } => write!(
                formatter,
                "prepared plugin generation {generation} was already published on this target"
            ),
            Self::ActiveGeneration { generation } => write!(
                formatter,
                "plugin generation {generation} must be withdrawn before replacement"
            ),
            Self::CleanupPending { generation } => write!(
                formatter,
                "plugin generation {generation} has unsettled cleanup"
            ),
            Self::StaleReceipt { requested, latest } => write!(
                formatter,
                "plugin receipt generation {requested} is stale; target has generation {latest}"
            ),
            Self::InvalidReceipt { generation } => write!(
                formatter,
                "plugin receipt for generation {generation} is not issued by this target"
            ),
            Self::WrongTarget => {
                formatter.write_str("plugin publication target belongs to another Agent")
            }
            Self::ApplyFailed {
                generation,
                diagnostics,
            } => write!(
                formatter,
                "plugin generation {generation} failed to apply: {diagnostics}"
            ),
            Self::RollbackFailed {
                generation,
                diagnostics,
                ..
            } => write!(
                formatter,
                "plugin generation {generation} failed to settle cleanup: {diagnostics}"
            ),
        }
    }
}

impl std::error::Error for PluginWiringError {}

#[derive(Default)]
struct PluginPublicationState {
    latest: Option<(u64, String)>,
    active: Option<PluginWiringResult>,
    cleanup_debt: Option<PluginWiringResult>,
    settled: Option<PluginWiringResult>,
}

/// One canonical state authority per ReactAgent, shared by its publication handles.
#[derive(Default)]
pub(crate) struct PluginPublicationAuthority {
    state: tokio::sync::Mutex<PluginPublicationState>,
}

/// Agent-bound handle for publishing and withdrawing immutable plugin generations.
#[derive(Clone)]
pub struct PluginPublicationTarget {
    authority: Arc<PluginPublicationAuthority>,
}

impl PluginPublicationTarget {
    fn for_agent(agent: &crate::agent::react::ReactAgent) -> Self {
        Self {
            authority: Arc::clone(&agent.plugin_publication),
        }
    }

    fn check_agent(
        &self,
        agent: &crate::agent::react::ReactAgent,
    ) -> Result<(), PluginWiringError> {
        if Arc::ptr_eq(&self.authority, &agent.plugin_publication) {
            Ok(())
        } else {
            Err(PluginWiringError::WrongTarget)
        }
    }

    /// A cancelled apply retains a cleanup receipt in the target authority.
    pub async fn pending_cleanup_receipt(&self) -> Option<PluginWiringResult> {
        self.authority.state.lock().await.cleanup_debt.clone()
    }

    /// Publish only a generation newer than this target's settled publication.
    pub async fn wire_prepared(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        prepared: &PreparedPluginSet,
    ) -> Result<PluginWiringResult, PluginWiringError> {
        self.check_agent(agent)?;
        if !prepared.is_applicable() {
            return Err(PluginWiringError::InvalidPreparedSet {
                generation: prepared.generation(),
            });
        }
        let mut state = self.authority.state.lock().await;
        if let Some((latest, identity)) = &state.latest {
            if prepared.generation() < *latest {
                return Err(PluginWiringError::StaleGeneration {
                    requested: prepared.generation(),
                    latest: *latest,
                });
            }
            if prepared.generation() == *latest {
                return Err(if prepared.identity() == identity {
                    PluginWiringError::AlreadyPublished {
                        generation: *latest,
                    }
                } else {
                    PluginWiringError::StaleGeneration {
                        requested: prepared.generation(),
                        latest: *latest,
                    }
                });
            }
        }
        if let Some(receipt) = &state.cleanup_debt {
            return Err(PluginWiringError::CleanupPending {
                generation: receipt.generation,
            });
        }
        if let Some(receipt) = &state.active {
            return Err(PluginWiringError::ActiveGeneration {
                generation: receipt.generation,
            });
        }

        state.cleanup_debt = Some(PluginWiringResult {
            generation: prepared.generation(),
            identity: prepared.identity().to_string(),
            publication_token: Some(Arc::new(())),
            ..PluginWiringResult::default()
        });
        let errors = if let Some(receipt) = &mut state.cleanup_debt {
            PluginIntegrator::apply_prepared(agent, prepared, receipt).await
        } else {
            return Err(PluginWiringError::ApplyFailed {
                generation: prepared.generation(),
                diagnostics: "plugin publication lost its pending receipt".to_string(),
            });
        };
        if errors.is_empty() {
            let receipt =
                state
                    .cleanup_debt
                    .take()
                    .ok_or_else(|| PluginWiringError::ApplyFailed {
                        generation: prepared.generation(),
                        diagnostics: "plugin publication lost its completed receipt".to_string(),
                    })?;
            state.latest = Some((receipt.generation, receipt.identity.clone()));
            state.settled = None;
            state.active = Some(receipt.clone());
            return Ok(receipt);
        }

        let receipt =
            state
                .cleanup_debt
                .as_ref()
                .cloned()
                .ok_or_else(|| PluginWiringError::ApplyFailed {
                    generation: prepared.generation(),
                    diagnostics: "plugin publication lost its cleanup receipt".to_string(),
                })?;
        let mut errors = errors;
        if let Err(error) = PluginIntegrator::unwire(agent, &receipt.components_by_plugin).await {
            errors.push(format!("plugin rollback cleanup: {error}"));
            return Err(PluginWiringError::RollbackFailed {
                generation: prepared.generation(),
                diagnostics: errors.join("; "),
                receipt: Box::new(receipt),
            });
        }
        state.cleanup_debt = None;
        Err(PluginWiringError::ApplyFailed {
            generation: prepared.generation(),
            diagnostics: errors.join("; "),
        })
    }

    /// Withdraw only the exact receipt issued for this Agent's current generation.
    pub async fn rollback(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        receipt: &PluginWiringResult,
    ) -> Result<(), PluginWiringError> {
        self.check_agent(agent)?;
        let mut state = self.authority.state.lock().await;
        if let Some((latest, _)) = &state.latest
            && receipt.generation < *latest
        {
            return Err(PluginWiringError::StaleReceipt {
                requested: receipt.generation,
                latest: *latest,
            });
        }
        if state
            .settled
            .as_ref()
            .is_some_and(|settled| receipt_matches(settled, receipt))
        {
            return Ok(());
        }
        let canonical = state
            .cleanup_debt
            .as_ref()
            .or(state.active.as_ref())
            .filter(|canonical| receipt_matches(canonical, receipt))
            .cloned()
            .ok_or(PluginWiringError::InvalidReceipt {
                generation: receipt.generation,
            })?;
        if let Err(error) = PluginIntegrator::unwire(agent, &canonical.components_by_plugin).await {
            return Err(PluginWiringError::RollbackFailed {
                generation: receipt.generation,
                diagnostics: format!("plugin rollback cleanup: {error}"),
                receipt: Box::new(canonical),
            });
        }
        state.cleanup_debt = None;
        state.active = None;
        state.settled = Some(canonical);
        Ok(())
    }
}

fn receipt_matches(canonical: &PluginWiringResult, submitted: &PluginWiringResult) -> bool {
    canonical.publication_token.is_some() && canonical == submitted
}

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

    /// Resolve the canonical publication authority attached to one Agent.
    pub fn publication_target(
        &self,
        agent: &crate::agent::react::ReactAgent,
    ) -> PluginPublicationTarget {
        PluginPublicationTarget::for_agent(agent)
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
        // A component parse/read failure is isolated to that component. Only
        // failures that prevent constructing the complete dependency-ordered
        // generation make the snapshot inapplicable.
        let mut applicable = true;
        let ordered = match registry.resolve_enabled_dependencies() {
            Ok(ordered) => ordered,
            Err(message) => {
                diagnostics.push(error_diagnostic(None, "dependencies", None, message));
                applicable = false;
                Vec::new()
            }
        };

        for plugin_id in ordered {
            hash_field(&mut identity, "plugin-id", plugin_id.as_bytes());
            if let Some(entry) = registry.get(&plugin_id) {
                match serde_json::to_vec(&entry.manifest) {
                    Ok(serialized) => hash_field(&mut identity, "manifest", &serialized),
                    Err(error) => {
                        applicable = false;
                        diagnostics.push(error_diagnostic(
                            Some(&plugin_id),
                            "manifest",
                            None,
                            error.to_string(),
                        ));
                    }
                }
                let ordered_config = entry
                    .user_config
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<BTreeMap<_, _>>();
                match serde_json::to_vec(&ordered_config) {
                    Ok(serialized) => hash_field(&mut identity, "user-config", &serialized),
                    Err(error) => {
                        applicable = false;
                        diagnostics.push(error_diagnostic(
                            Some(&plugin_id),
                            "user-config",
                            None,
                            error.to_string(),
                        ));
                    }
                }
            }

            let variables = match registry.variables_for(&plugin_id) {
                Ok(variables) => variables,
                Err(message) => {
                    applicable = false;
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
                applicable = false;
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
                    applicable = false;
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
                            let Some(document) = loader.get_document(&name).cloned() else {
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
                            skills.push(PreparedPluginSkill { document });
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
                                Ok(hooks) => validate_prepared_hooks(
                                    &plugin_id,
                                    &path,
                                    hooks,
                                    &mut diagnostics,
                                ),
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
            if let Some(path) = resolved.mcp_config_file
                && let Some(contents) =
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

        let generation = Self::next_generation().unwrap_or(u64::MAX);
        if generation == u64::MAX {
            applicable = false;
            diagnostics.push(error_diagnostic(
                None,
                "generation",
                None,
                "plugin preparation generation exhausted or cache unavailable".to_string(),
            ));
        }
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

    fn next_generation() -> Option<u64> {
        NEXT_PREPARED_GENERATION
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1).filter(|next| *next < u64::MAX)
            })
            .ok()?
            .checked_add(1)
    }

    /// Apply a frozen generation. No package files are read here.
    pub async fn wire_prepared(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        prepared: &PreparedPluginSet,
    ) -> Result<PluginWiringResult, PluginWiringError> {
        self.publication_target(agent)
            .wire_prepared(agent, prepared)
            .await
    }

    async fn apply_prepared(
        agent: &mut crate::agent::react::ReactAgent,
        prepared: &PreparedPluginSet,
        receipt: &mut PluginWiringResult,
    ) -> Vec<String> {
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
            // The receipt must already name possible mutations while an awaited
            // registration is in flight, so cancellation retains a cleanup owner.
            if !plugin.skills().is_empty() || plugin.hooks().is_some() {
                let owned = receipt
                    .components_by_plugin
                    .entry(plugin.id().to_string())
                    .or_default();
                owned.skills = plugin
                    .skills()
                    .iter()
                    .map(|skill| skill.document().descriptor().name.clone())
                    .collect();
                owned.hooks_registered = plugin.hooks().is_some();
            }
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
                        .skills = names;
                }
                Err(error) => errors.push(format!("Plugin '{}' Skills: {error}", plugin.id())),
            }
            if let Some(hooks) = plugin.hooks() {
                #[cfg(feature = "mcp")]
                let hooks = owner_qualified_plugin_hooks(plugin.id(), hooks);
                #[cfg(not(feature = "mcp"))]
                let hooks = hooks.clone();
                let registered = agent.hook_registry().write().await.register_plugin_hooks(
                    plugin.id(),
                    &plugin.variables().plugin_root.display().to_string(),
                    &plugin.variables().plugin_data.display().to_string(),
                    hooks,
                );
                if registered {
                    receipt.hooks_registered.push(plugin.id().to_string());
                }
                receipt
                    .components_by_plugin
                    .entry(plugin.id().to_string())
                    .or_default()
                    .hooks_registered = registered;
            }
            #[cfg(feature = "mcp")]
            if let Some(config) = plugin.mcp() {
                match config.to_server_configs() {
                    Ok(mut servers) => {
                        servers.sort_by(|left, right| left.name.cmp(&right.name));
                        for server in servers {
                            let name = server.name.clone();
                            let server_id = crate::mcp::McpServerId::plugin(plugin.id(), &name);
                            let selector = server_id.selector();
                            let preexisting = agent.mcp_client(&selector).is_some();
                            if !preexisting {
                                receipt
                                    .components_by_plugin
                                    .entry(plugin.id().to_string())
                                    .or_default()
                                    .mcp_servers
                                    .push(name.clone());
                                receipt
                                    .components_by_plugin
                                    .entry(plugin.id().to_string())
                                    .or_default()
                                    .mcp_server_ids
                                    .push(server_id.clone());
                            }
                            match agent.connect_mcp_owned(server_id.clone(), server).await {
                                Ok(client) => {
                                    let connected = client.server_name().to_string();
                                    receipt.mcp_connected.push(connected.clone());
                                    receipt.mcp_connected_ids.push(server_id.clone());
                                    if preexisting {
                                        receipt
                                            .components_by_plugin
                                            .entry(plugin.id().to_string())
                                            .or_default()
                                            .mcp_servers
                                            .push(connected);
                                        receipt
                                            .components_by_plugin
                                            .entry(plugin.id().to_string())
                                            .or_default()
                                            .mcp_server_ids
                                            .push(server_id.clone());
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        plugin = %plugin.id(),
                                        server = %name,
                                        error = %error,
                                        "Plugin MCP server connection failed, skipping"
                                    );
                                    if !preexisting {
                                        match agent.disconnect_mcp_owned(&server_id).await {
                                            Ok(_) => {
                                                if let Some(owned) = receipt
                                                    .components_by_plugin
                                                    .get_mut(plugin.id())
                                                {
                                                    owned
                                                        .mcp_servers
                                                        .retain(|server| server != &name);
                                                    owned
                                                        .mcp_server_ids
                                                        .retain(|server| server != &server_id);
                                                }
                                            }
                                            Err(cleanup_error) => errors.push(format!(
                                                "Plugin '{}' MCP '{name}' cleanup: {cleanup_error}",
                                                plugin.id()
                                            )),
                                        }
                                    }
                                }
                            }
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
            if receipt
                .components_by_plugin
                .get(plugin.id())
                .is_some_and(|owned| {
                    !owned.skills.is_empty()
                        || owned.hooks_registered
                        || !owned.mcp_servers.is_empty()
                })
            {
                receipt.plugins_loaded.push(plugin.id().to_string());
            }
        }

        errors
    }

    /// Undo exactly the registrations recorded by one apply receipt.
    pub async fn rollback(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        receipt: &PluginWiringResult,
    ) -> Result<(), PluginWiringError> {
        self.publication_target(agent)
            .rollback(agent, receipt)
            .await
    }

    async fn unwire(
        agent: &mut crate::agent::react::ReactAgent,
        components: &HashMap<String, WiredPluginComponents>,
    ) -> crate::error::Result<()> {
        let cleanup_failures: Vec<String> = Vec::new();
        #[cfg(feature = "mcp")]
        let mut cleanup_failures = cleanup_failures;
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
            for server_id in &owned.mcp_server_ids {
                if let Err(error) = agent.disconnect_mcp_owned(server_id).await {
                    cleanup_failures.push(format!("{plugin_id}/{server_id}: {error}"));
                }
            }
            #[cfg(feature = "mcp")]
            if owned.mcp_server_ids.is_empty() {
                for server in &owned.mcp_servers {
                    if let Err(error) = agent.disconnect_mcp(server).await {
                        cleanup_failures.push(format!("{plugin_id}/{server}: {error}"));
                    }
                }
            }
        }
        if cleanup_failures.is_empty() {
            Ok(())
        } else {
            Err(crate::error::ReactError::Other(format!(
                "plugin MCP cleanup did not settle: {}",
                cleanup_failures.join("; ")
            )))
        }
    }
}

#[cfg(feature = "mcp")]
fn owner_qualified_plugin_hooks(
    plugin_id: &str,
    hooks: &echo_execution::skills::hooks::HooksDefinition,
) -> echo_execution::skills::hooks::HooksDefinition {
    let mut qualified = hooks.clone();
    for rules in qualified.rules.values_mut() {
        for rule in rules {
            for action in &mut rule.hooks {
                if let echo_execution::skills::hooks::HookAction::McpTool { server, .. } = action {
                    *server =
                        crate::mcp::McpServerId::plugin(plugin_id, server.as_str()).selector();
                }
            }
        }
    }
    qualified
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

fn validate_prepared_hooks(
    plugin_id: &str,
    path: &Path,
    hooks: echo_execution::skills::hooks::HooksDefinition,
    diagnostics: &mut Vec<PluginPreparationDiagnostic>,
) -> Option<echo_execution::skills::hooks::HooksDefinition> {
    let mut failures = hooks
        .rules
        .iter()
        .flat_map(|(event, rules)| {
            rules.iter().flat_map(move |rule| {
                rule.hooks.iter().filter_map(move |action| {
                    action.validate().err().map(|error| {
                        format!("event {} action {}: {error}", event.as_str(), action.kind())
                    })
                })
            })
        })
        .collect::<Vec<_>>();
    failures.sort();
    if failures.is_empty() {
        return Some(hooks);
    }
    diagnostics.extend(failures.into_iter().map(|message| {
        error_diagnostic(Some(plugin_id), "hooks", Some(path.to_path_buf()), message)
    }));
    None
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

    #[cfg(feature = "mcp")]
    #[test]
    fn plugin_mcp_hook_server_is_qualified_by_prepared_owner() {
        use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};

        let mut hooks = HooksDefinition::default();
        hooks.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "probe".to_string(),
                hooks: vec![HookAction::McpTool {
                    server: "shared".to_string(),
                    tool: "probe".to_string(),
                    arguments: None,
                    timeout: 1,
                }],
            }],
        );
        let qualified = owner_qualified_plugin_hooks("plugin-a", &hooks);
        let server = qualified
            .rules_for(HookEvent::PreToolUse)
            .first()
            .and_then(|rule| rule.hooks.first())
            .and_then(|action| match action {
                HookAction::McpTool { server, .. } => Some(server.as_str()),
                _ => None,
            });
        let expected = crate::mcp::McpServerId::plugin("plugin-a", "shared").selector();
        assert_eq!(server, Some(expected.as_str()));
        assert!(matches!(
            hooks
                .rules_for(HookEvent::PreToolUse)
                .first()
                .and_then(|rule| rule.hooks.first()),
            Some(HookAction::McpTool { server, .. }) if server == "shared"
        ));
    }

    #[cfg(feature = "mcp")]
    struct RetryCloseTransport {
        close_attempts: std::sync::atomic::AtomicUsize,
    }

    #[cfg(feature = "mcp")]
    impl echo_integration::mcp::transport::McpTransport for RetryCloseTransport {
        fn send(
            &self,
            request: echo_integration::mcp::types::JsonRpcRequest,
        ) -> futures::future::BoxFuture<
            '_,
            crate::error::Result<echo_integration::mcp::types::JsonRpcResponse>,
        > {
            Box::pin(async move {
                use echo_integration::mcp::types::{InitializeResult, ServerCapabilities};
                let result = match request.method.as_str() {
                    "initialize" => serde_json::to_value(InitializeResult {
                        protocol_version: echo_integration::mcp::types::MCP_PROTOCOL_VERSION
                            .to_string(),
                        capabilities: ServerCapabilities::default(),
                        server_info: None,
                        instructions: None,
                    })?,
                    "tools/list" => serde_json::json!({"tools": []}),
                    "resources/list" => serde_json::json!({"resources": []}),
                    _ => serde_json::json!({}),
                };
                Ok(echo_integration::mcp::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(result),
                    error: None,
                })
            })
        }

        fn notify(
            &self,
            _notification: echo_integration::mcp::types::JsonRpcNotification,
        ) -> futures::future::BoxFuture<'_, crate::error::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> futures::future::BoxFuture<'_, crate::error::Result<()>> {
            let attempt = self
                .close_attempts
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Box::pin(async move {
                if attempt == 0 {
                    Err(crate::error::ReactError::Other(
                        "injected first close failure".to_string(),
                    ))
                } else {
                    Ok(())
                }
            })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn echo_integration::mcp::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

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
    async fn component_parse_error_is_isolated_and_healthy_siblings_apply()
    -> Result<(), Box<dyn std::error::Error>> {
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

        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        let plugin = prepared
            .plugins()
            .first()
            .ok_or_else(|| missing("prepared plugin missing"))?;
        assert_eq!(plugin.skills().len(), 1);
        assert!(plugin.hooks().is_none());
        assert_eq!(plugin.subagent_documents().len(), 1);
        assert!(plugin.lsp_document().is_some());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("invalid.test")
                && diagnostic.component() == "hooks"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic.path() == Some(hooks.as_path())
        }));

        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let receipt = PluginIntegrator::new()
            .wire_prepared(&mut agent, &prepared)
            .await?;
        assert_eq!(receipt.plugins_loaded, vec!["invalid.test"]);
        assert_eq!(receipt.skills_loaded.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_skill_is_excluded_without_dropping_healthy_plugin_components()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "skills.test", serde_json::json!([]))?;
        std::fs::create_dir_all(source.join("skills/healthy"))?;
        std::fs::write(
            source.join("skills/healthy/SKILL.md"),
            "---\nname: healthy\ndescription: Healthy\n---\nhealthy\n",
        )?;
        std::fs::write(
            source.join("hooks/hooks.yaml"),
            "SessionStart:\n  - matcher: startup\n    hooks:\n      - type: prompt\n        prompt: ready\n",
        )?;
        let invalid_skill = source.join("skills/example/SKILL.md");
        std::fs::write(
            &invalid_skill,
            "---\nname: wrong-name\ndescription: Invalid\n---\ninvalid\n",
        )?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;

        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;

        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        let plugin = prepared
            .plugins()
            .first()
            .ok_or_else(|| missing("prepared plugin missing"))?;
        assert_eq!(plugin.skills().len(), 1);
        assert_eq!(
            plugin
                .skills()
                .first()
                .map(|skill| skill.document().descriptor().name.as_str()),
            Some("healthy")
        );
        assert!(plugin.hooks().is_some());
        assert_eq!(plugin.subagent_documents().len(), 1);
        assert!(plugin.lsp_document().is_some());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("skills.test")
                && diagnostic.component() == "skill"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic
                    .path()
                    .is_some_and(|path| path.ends_with("skills/example/SKILL.md"))
        }));

        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let receipt = integrator.wire_prepared(&mut agent, &prepared).await?;
        assert_eq!(receipt.plugins_loaded, vec!["skills.test"]);
        assert_eq!(receipt.skills_loaded, vec!["healthy"]);
        assert_eq!(receipt.hooks_registered, vec!["skills.test"]);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_hook_action_excludes_hook_component_with_structured_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "hook.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        let plugin_id = registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let hooks = registry
            .get(&plugin_id)
            .ok_or_else(|| missing("hook plugin missing"))?
            .root
            .join("hooks/hooks.yaml");
        std::fs::write(
            &hooks,
            "PreToolUse:\n  - matcher: '*'\n    hooks:\n      - type: prompt\n        prompt: ''\n",
        )?;

        let prepared = PluginIntegrator::new().prepare(&mut registry).await;

        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        let plugin = prepared
            .plugins()
            .first()
            .ok_or_else(|| missing("prepared hook plugin missing"))?;
        assert_eq!(plugin.skills().len(), 1);
        assert!(plugin.hooks().is_none());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("hook.test")
                && diagnostic.component() == "hooks"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic.path() == Some(hooks.as_path())
                && diagnostic.message().contains("empty prompt")
        }));
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn invalid_mcp_excludes_mcp_component_without_poisoning_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "mcp.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        let plugin_id = registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let mcp = registry
            .get(&plugin_id)
            .ok_or_else(|| missing("MCP plugin missing"))?
            .root
            .join("mcp.json");
        std::fs::write(&mcp, "{\"mcpServers\":")?;

        let prepared = PluginIntegrator::new().prepare(&mut registry).await;

        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        let plugin = prepared
            .plugins()
            .first()
            .ok_or_else(|| missing("prepared MCP plugin missing"))?;
        assert_eq!(plugin.skills().len(), 1);
        assert!(plugin.mcp().is_none());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("mcp.test")
                && diagnostic.component() == "mcp"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic.path() == Some(mcp.as_path())
        }));
        Ok(())
    }

    #[tokio::test]
    async fn plugin_level_failure_rejects_generation_with_dependent_plugin()
    -> Result<(), Box<dyn std::error::Error>> {
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
        let data_path = registry.data_dir_for(&base_id);
        let data_parent = data_path
            .parent()
            .ok_or_else(|| missing("plugin data parent missing"))?;
        std::fs::create_dir_all(data_parent)?;
        std::fs::write(&data_path, "not a directory")?;

        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;

        assert!(!prepared.is_applicable());
        assert!(prepared.diagnostics().iter().any(|diagnostic| {
            diagnostic.plugin_id() == Some("base.test")
                && diagnostic.component() == "data"
                && diagnostic.severity() == PluginDiagnosticSeverity::Error
                && diagnostic.path() == Some(data_path.as_path())
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
        integrator.rollback(&mut agent, &receipt).await?;
        Ok(())
    }

    #[tokio::test]
    async fn publication_rejects_stale_prepared_and_receipt_before_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let old = integrator.prepare(&mut registry).await;
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let target = integrator.publication_target(&agent);
        let old_receipt = target.wire_prepared(&mut agent, &old).await?;
        target.rollback(&mut agent, &old_receipt).await?;
        integrator.invalidate(&registry);
        let current = integrator.prepare(&mut registry).await;
        let current_receipt = target.wire_prepared(&mut agent, &current).await?;
        assert_eq!(current_receipt.generation(), current.generation());
        assert_eq!(current_receipt.identity(), current.identity());
        assert!(matches!(
            target.wire_prepared(&mut agent, &old).await,
            Err(PluginWiringError::StaleGeneration { .. })
        ));
        assert!(matches!(
            target.rollback(&mut agent, &old_receipt).await,
            Err(PluginWiringError::StaleReceipt { .. })
        ));
        assert!(agent.skill_registry().get_descriptor("example").is_some());
        target.rollback(&mut agent, &current_receipt).await?;
        target.rollback(&mut agent, &current_receipt).await?;
        Ok(())
    }

    #[tokio::test]
    async fn publication_target_is_canonical_per_agent_and_rejects_receipt_forgery()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        let mut first_agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let mut second_agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let first = integrator.publication_target(&first_agent);
        let shared = integrator.clone().publication_target(&first_agent);
        let second = integrator.clone().publication_target(&second_agent);
        let first_receipt = first.wire_prepared(&mut first_agent, &prepared).await?;
        let second_receipt = second.wire_prepared(&mut second_agent, &prepared).await?;
        assert_ne!(first_receipt, second_receipt);
        assert!(matches!(
            shared.wire_prepared(&mut first_agent, &prepared).await,
            Err(PluginWiringError::AlreadyPublished { .. })
        ));
        assert!(matches!(
            second.rollback(&mut second_agent, &first_receipt).await,
            Err(PluginWiringError::InvalidReceipt { .. })
        ));
        let mut forged = first_receipt.clone();
        forged.components_by_plugin.clear();
        assert!(matches!(
            first.rollback(&mut first_agent, &forged).await,
            Err(PluginWiringError::InvalidReceipt { .. })
        ));
        #[cfg(feature = "mcp")]
        {
            let mut forged_identity = first_receipt.clone();
            forged_identity
                .mcp_connected_ids
                .push(crate::mcp::McpServerId::plugin("forged", "server"));
            assert!(matches!(
                first.rollback(&mut first_agent, &forged_identity).await,
                Err(PluginWiringError::InvalidReceipt { .. })
            ));
        }
        assert!(matches!(
            second.rollback(&mut first_agent, &second_receipt).await,
            Err(PluginWiringError::WrongTarget)
        ));
        shared.rollback(&mut first_agent, &first_receipt).await?;
        second.rollback(&mut second_agent, &second_receipt).await?;
        Ok(())
    }

    #[tokio::test]
    async fn independent_integrators_share_preparation_order_on_one_agent()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let first_integrator = PluginIntegrator::new();
        let second_integrator = PluginIntegrator::new();
        let first = first_integrator.prepare(&mut registry).await;
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let target = first_integrator.publication_target(&agent);
        let first_receipt = target.wire_prepared(&mut agent, &first).await?;
        target.rollback(&mut agent, &first_receipt).await?;

        let second = second_integrator.prepare(&mut registry).await;
        assert!(second.generation() > first.generation());
        let second_receipt = target.wire_prepared(&mut agent, &second).await?;
        target.rollback(&mut agent, &second_receipt).await?;

        for _ in 0..2 {
            first_integrator.invalidate(&registry);
            let _ = first_integrator.prepare(&mut registry).await;
        }
        let old_third = first_integrator.prepare(&mut registry).await;
        second_integrator.invalidate(&registry);
        let newer = second_integrator.prepare(&mut registry).await;
        assert!(newer.generation() > old_third.generation());
        let newer_receipt = target.wire_prepared(&mut agent, &newer).await?;
        target.rollback(&mut agent, &newer_receipt).await?;
        assert!(matches!(
            target.wire_prepared(&mut agent, &old_third).await,
            Err(PluginWiringError::StaleGeneration { .. })
        ));
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn cancelled_second_mcp_server_keeps_first_in_cleanup_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let first_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let first_address = first_listener.local_addr()?;
        let first_server = tokio::spawn(async move {
            let initialize = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": echo_integration::mcp::types::MCP_PROTOCOL_VERSION,
                    "capabilities": {}
                }
            })
            .to_string();
            for response in [
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{initialize}",
                    initialize.len()
                ),
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
            ] {
                let (mut connection, _) = first_listener.accept().await?;
                let mut request = [0_u8; 8192];
                let _ = connection.read(&mut request).await?;
                connection.write_all(response.as_bytes()).await?;
            }
            Ok::<(), std::io::Error>(())
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        std::fs::write(
            source.join("mcp.json"),
            serde_json::json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
                "mcpServers": {
                    "a-owned": {"type": "streamable-http", "url": format!("http://{first_address}/mcp")},
                    "z-blocked": {"type": "streamable-http", "url": format!("http://{address}/mcp")}
                }
            })
            .to_string(),
        )?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let blocked_server = tokio::spawn(async move {
            if let Ok((_connection, _)) = listener.accept().await {
                let _ = entered_tx.send(());
                std::future::pending::<()>().await;
            }
        });
        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        assert!(prepared.is_applicable(), "{:?}", prepared.diagnostics());
        assert!(
            prepared
                .plugins
                .first()
                .is_some_and(|plugin| plugin.mcp().is_some())
        );
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        assert!(agent.mcp_client("a-owned").is_none());
        let target = integrator.publication_target(&agent);
        {
            let apply = target.wire_prepared(&mut agent, &prepared);
            tokio::pin!(apply);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                tokio::select! {
                    reached = entered_rx => reached.map_err(|_| missing("second MCP request was not observed")),
                    result = &mut apply => Err(missing(&format!("apply completed before cancellation: {result:?}"))),
                }
            })
            .await??;
        }
        blocked_server.abort();
        let _ = blocked_server.await;
        first_server
            .await
            .map_err(|error| missing(&error.to_string()))??;

        assert!(
            agent
                .tools
                .mcp_manager
                .get_client_by_id(&crate::mcp::McpServerId::plugin("prepared.test", "a-owned",))
                .is_some()
        );
        let pending = target
            .pending_cleanup_receipt()
            .await
            .ok_or_else(|| missing("cancelled MCP apply lost cleanup receipt"))?;
        assert_eq!(pending.mcp_connected, ["a-owned"]);
        assert!(
            pending
                .components_by_plugin
                .get("prepared.test")
                .is_some_and(|owned| owned.mcp_servers.contains(&"z-blocked".to_string()))
        );
        assert!(matches!(
            target.wire_prepared(&mut agent, &prepared).await,
            Err(PluginWiringError::CleanupPending { .. })
        ));
        target.rollback(&mut agent, &pending).await?;
        assert!(
            agent
                .tools
                .mcp_manager
                .get_client_by_id(&crate::mcp::McpServerId::plugin("prepared.test", "a-owned",))
                .is_none()
        );
        assert!(
            agent
                .tools
                .mcp_manager
                .get_client_by_id(&crate::mcp::McpServerId::plugin(
                    "prepared.test",
                    "z-blocked",
                ))
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_apply_retains_cleanup_owner_and_blocks_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let target = integrator.publication_target(&agent);
        let hooks = Arc::clone(agent.hook_registry());
        let held = hooks.write_owned().await;
        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(40),
            target.wire_prepared(&mut agent, &prepared),
        )
        .await;
        assert!(timed_out.is_err());
        drop(held);

        let pending = target
            .pending_cleanup_receipt()
            .await
            .ok_or_else(|| missing("cancelled apply lost its cleanup receipt"))?;
        assert!(agent.skill_registry().get_descriptor("example").is_some());
        assert!(matches!(
            target.wire_prepared(&mut agent, &prepared).await,
            Err(PluginWiringError::CleanupPending { .. })
        ));
        target.rollback(&mut agent, &pending).await?;
        assert!(agent.skill_registry().get_descriptor("example").is_none());
        let retried = target.wire_prepared(&mut agent, &prepared).await?;
        target.rollback(&mut agent, &retried).await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_withdraw_retains_current_receipt_and_blocks_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let prepared = integrator.prepare(&mut registry).await;
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let target = integrator.publication_target(&agent);
        let receipt = target.wire_prepared(&mut agent, &prepared).await?;
        let hooks = Arc::clone(agent.hook_registry());
        let held = hooks.write_owned().await;
        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(40),
            target.rollback(&mut agent, &receipt),
        )
        .await;
        assert!(timed_out.is_err());
        drop(held);

        integrator.invalidate(&registry);
        let replacement = integrator.prepare(&mut registry).await;
        assert!(matches!(
            target.wire_prepared(&mut agent, &replacement).await,
            Err(PluginWiringError::ActiveGeneration { .. })
        ));
        target.rollback(&mut agent, &receipt).await?;
        let next = target.wire_prepared(&mut agent, &replacement).await?;
        target.rollback(&mut agent, &next).await?;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn failed_active_withdraw_keeps_publication_fenced_until_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let mut prepared = (*integrator.prepare(&mut registry).await).clone();
        let valid = crate::mcp::McpConfigFile::parse(
            r#"{"mcpServers":{"owned":{"command":"unused-test-command"}}}"#,
        )?;
        prepared
            .plugins
            .first_mut()
            .ok_or_else(|| missing("prepared plugin missing"))?
            .mcp = Some(valid.clone());
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        let transport = Arc::new(RetryCloseTransport {
            close_attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let client = crate::mcp::McpClient::from_transport("owned", transport.clone())?.await?;
        let config = valid
            .to_server_configs()?
            .pop()
            .ok_or_else(|| missing("test MCP target missing"))?;
        agent
            .tools
            .mcp_manager
            .install_prepared_server(
                crate::mcp::McpServerId::plugin("prepared.test", "owned"),
                config,
                client,
            )
            .await?;
        let target = integrator.publication_target(&agent);
        let receipt = target.wire_prepared(&mut agent, &prepared).await?;
        assert_eq!(receipt.mcp_connected, ["owned"]);

        let failed = target
            .rollback(&mut agent, &receipt)
            .await
            .err()
            .ok_or_else(|| missing("first active cleanup unexpectedly succeeded"))?;
        assert!(matches!(failed, PluginWiringError::RollbackFailed { .. }));
        assert_eq!(failed.cleanup_receipt(), Some(&receipt));
        integrator.invalidate(&registry);
        let replacement = integrator.prepare(&mut registry).await;
        assert!(matches!(
            target.wire_prepared(&mut agent, &replacement).await,
            Err(PluginWiringError::ActiveGeneration { .. })
        ));
        target.rollback(&mut agent, &receipt).await?;
        assert_eq!(
            transport
                .close_attempts
                .load(std::sync::atomic::Ordering::Acquire),
            2
        );
        let next = target.wire_prepared(&mut agent, &replacement).await?;
        target.rollback(&mut agent, &next).await?;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn partial_apply_rollback_failure_returns_retryable_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = create_plugin(temporary.path(), "prepared.test", serde_json::json!([]))?;
        let mut registry = registry(temporary.path());
        registry.install(&InstallSource::Local(source), PluginScope::Local)?;
        let integrator = PluginIntegrator::new();
        let mut prepared = (*integrator.prepare(&mut registry).await).clone();
        let valid = crate::mcp::McpConfigFile::parse(
            r#"{"mcpServers":{"owned":{"command":"unused-test-command"}}}"#,
        )?;
        let invalid = crate::mcp::McpConfigFile::parse(r#"{"mcpServers":{"invalid":{}}}"#)?;
        let plugin = prepared
            .plugins
            .first_mut()
            .ok_or_else(|| missing("prepared plugin missing"))?;
        plugin.mcp = Some(valid.clone());
        let mut failing_plugin = plugin.clone();
        failing_plugin.id = "invalid.test".to_string();
        failing_plugin.skills.clear();
        failing_plugin.hooks = None;
        failing_plugin.mcp = Some(invalid);
        prepared.plugins.push(failing_plugin);

        let transport = Arc::new(RetryCloseTransport {
            close_attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let mcp_transport: Arc<dyn echo_integration::mcp::transport::McpTransport> =
            transport.clone();
        let client = crate::mcp::McpClient::from_transport("owned", mcp_transport)?.await?;
        let config = valid
            .to_server_configs()?
            .pop()
            .ok_or_else(|| missing("valid MCP target missing"))?;
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("prepared-test")
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        agent
            .tools
            .mcp_manager
            .install_prepared_server(
                crate::mcp::McpServerId::plugin("prepared.test", "owned"),
                config,
                client,
            )
            .await?;

        let error = integrator
            .wire_prepared(&mut agent, &prepared)
            .await
            .err()
            .ok_or_else(|| missing("partial apply unexpectedly succeeded"))?;
        assert!(matches!(error, PluginWiringError::RollbackFailed { .. }));
        let receipt = error
            .cleanup_receipt()
            .ok_or_else(|| missing("failed rollback lost cleanup receipt"))?;
        assert_eq!(receipt.mcp_connected, ["owned"]);
        assert_eq!(receipt.skills_loaded, ["example"]);
        assert_eq!(
            transport
                .close_attempts
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
        assert!(agent.mcp_client("owned").is_none());

        integrator.rollback(&mut agent, receipt).await?;
        assert_eq!(
            transport
                .close_attempts
                .load(std::sync::atomic::Ordering::Acquire),
            2
        );
        let mut retry = prepared.clone();
        retry.plugins.retain(|plugin| plugin.id() != "invalid.test");
        if let Some(plugin) = retry.plugins.first_mut() {
            plugin.mcp = None;
        }
        let published = integrator.wire_prepared(&mut agent, &retry).await?;
        assert_eq!(published.generation(), prepared.generation());
        assert!(matches!(
            integrator.rollback(&mut agent, receipt).await,
            Err(PluginWiringError::InvalidReceipt { .. })
        ));
        integrator.rollback(&mut agent, &published).await?;
        Ok(())
    }
}
