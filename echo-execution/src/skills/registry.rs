//! Skill Registry -- central lifecycle manager for both code-based and file-based skills.
//!
//! Replaces the old `SkillManager` with full progressive-disclosure support:
//!
//! | Phase | What happens | Token cost |
//! |-------|-------------|------------|
//! | Discovery | `SKILL.md` frontmatter parsed -> `SkillDescriptor` | ~50-100 per skill |
//! | Catalog | Compact list injected into system prompt | sum of above |
//! | Activation | Full `SKILL.md` body loaded via `activate_skill` tool | <5000 (recommended) |
//! | Resources | Individual files loaded via `read_skill_resource` tool | varies |

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use crate::skills::SkillInfo;
use crate::skills::external::SkillDocument;
use crate::skills::external::prompt_exec::{PromptContext, SkillSource, process_skill_content};
use crate::skills::external::types::{
    SkillContent, SkillDescriptor, SkillResourceEntry, SkillResourceKind, SkillSandboxPolicy,
};
use echo_core::sandbox::SandboxExecutor;

/// Runtime activation authority shared by all views of one agent's Skill registry.
///
/// Catalog and prepared-document data may be copied into a concurrent tool
/// adapter, but activation is an agent-runtime fact and must have one owner.
/// The handle is deliberately public because `echo_agent` and `echo_execution`
/// are separate crates; it is a process-local Rust/Host boundary, not an SDK
/// lifecycle object.
#[derive(Clone)]
pub struct SkillActivationHandle {
    shared: Arc<SkillActivationShared>,
}

struct SkillActivationShared {
    state: std::sync::Mutex<SkillActivationState>,
}

struct SkillActivationState {
    session_id: String,
    epoch: u64,
    generations: HashMap<String, u64>,
    activated: HashMap<String, ActivatedSkill>,
    in_flight: HashMap<String, Arc<ActivationFlight>>,
}

#[derive(Clone)]
struct ActivatedSkill {
    key: Option<ActivationKey>,
    content: Option<SkillContent>,
    sandbox_policy: Option<SkillSandboxPolicy>,
}

#[derive(Clone, PartialEq, Eq)]
struct ActivationKey {
    arguments: Vec<String>,
    source: SkillSource,
}

struct ActivationFlight {
    epoch: u64,
    generation: u64,
    key: ActivationKey,
    abandoned: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    waiters: std::sync::atomic::AtomicUsize,
    result: tokio::sync::OnceCell<std::result::Result<SkillContent, String>>,
}

enum ActivationClaim {
    Cached(Box<SkillContent>),
    Restored,
    Abandoned,
    Conflict,
    Flight(Arc<ActivationFlight>),
}

struct ActivationAttemptGuard {
    activation: SkillActivationHandle,
    name: String,
    flight: Arc<ActivationFlight>,
    completed: bool,
}

impl ActivationAttemptGuard {
    fn new(activation: SkillActivationHandle, name: &str, flight: Arc<ActivationFlight>) -> Self {
        Self {
            activation,
            name: name.to_string(),
            flight,
            completed: false,
        }
    }

    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for ActivationAttemptGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.activation.poison(&self.name, &self.flight);
        }
    }
}

impl SkillActivationState {
    fn new() -> Self {
        Self {
            session_id: format!(
                "session-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .chars()
                    .take(8)
                    .collect::<String>()
            ),
            epoch: 0,
            generations: HashMap::new(),
            activated: HashMap::new(),
            in_flight: HashMap::new(),
        }
    }
}

impl SkillActivationHandle {
    fn new() -> Self {
        Self {
            shared: Arc::new(SkillActivationShared {
                state: std::sync::Mutex::new(SkillActivationState::new()),
            }),
        }
    }

    /// Return the current active Skill names in deterministic order.
    pub fn activated_names(&self) -> Vec<String> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut names = state.activated.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    fn session_id(&self) -> String {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .session_id
            .clone()
    }

    fn is_activated(&self, name: &str) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .activated
            .contains_key(name)
    }

    fn activated_count(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .activated
            .len()
    }

    fn active_sandbox_policy(&self, name: &str) -> Option<SkillSandboxPolicy> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .activated
            .get(name)
            .and_then(|activation| activation.sandbox_policy.clone())
    }

    fn mark_activated(&self, name: &str, sandbox_policy: Option<SkillSandboxPolicy>) -> bool {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.activated.contains_key(name) || state.in_flight.contains_key(name) {
            return false;
        }
        state.activated.insert(
            name.to_string(),
            ActivatedSkill {
                key: None,
                content: None,
                sandbox_policy,
            },
        );
        true
    }

    fn reset(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.epoch = state.epoch.wrapping_add(1);
        state.generations.clear();
        state.activated.clear();
        state.in_flight.clear();
    }

    fn retire(&self, name: &str) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::retire_name(&mut state, name);
    }

    fn retire_name(state: &mut SkillActivationState, name: &str) {
        Self::advance_generation(state, name);
        state.activated.remove(name);
        state.in_flight.remove(name);
    }

    fn advance_generation(state: &mut SkillActivationState, name: &str) {
        let generation = state.generations.entry(name.to_string()).or_default();
        *generation = generation.wrapping_add(1);
    }

    fn restore(&self, skills: Vec<(String, Option<SkillSandboxPolicy>)>) -> Vec<String> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.epoch = state.epoch.wrapping_add(1);
        state.generations.clear();
        state.activated.clear();
        state.in_flight.clear();
        let mut restored = Vec::with_capacity(skills.len());
        for (name, sandbox_policy) in skills {
            state.activated.insert(
                name.clone(),
                ActivatedSkill {
                    key: None,
                    content: None,
                    sandbox_policy,
                },
            );
            restored.push(name);
        }
        restored.sort();
        restored.dedup();
        restored
    }

    fn claim(&self, name: &str, key: ActivationKey) -> ActivationClaim {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(flight) = state.in_flight.get(name) {
            if flight.abandoned.load(std::sync::atomic::Ordering::SeqCst) {
                return ActivationClaim::Abandoned;
            }
            return if flight.key == key {
                #[cfg(test)]
                flight
                    .waiters
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ActivationClaim::Flight(Arc::clone(flight))
            } else {
                ActivationClaim::Conflict
            };
        }
        if let Some(active) = state.activated.get(name) {
            match (&active.key, &active.content) {
                (Some(active_key), Some(content)) if active_key == &key => {
                    return ActivationClaim::Cached(Box::new(content.clone()));
                }
                (None, None) => return ActivationClaim::Restored,
                _ => Self::advance_generation(&mut state, name),
            }
        }
        let flight = Arc::new(ActivationFlight {
            epoch: state.epoch,
            generation: state.generations.get(name).copied().unwrap_or_default(),
            key,
            abandoned: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            waiters: std::sync::atomic::AtomicUsize::new(0),
            result: tokio::sync::OnceCell::new(),
        });
        state
            .in_flight
            .insert(name.to_string(), Arc::clone(&flight));
        ActivationClaim::Flight(flight)
    }

    fn publish(
        &self,
        name: &str,
        flight: &Arc<ActivationFlight>,
        content: SkillContent,
        sandbox_policy: Option<SkillSandboxPolicy>,
    ) -> std::result::Result<(), String> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current_generation = state.generations.get(name).copied().unwrap_or_default();
        let owns_flight = state
            .in_flight
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, flight));
        if state.epoch != flight.epoch || current_generation != flight.generation || !owns_flight {
            return Err(format!(
                "Skill '{name}' activation was retired before publication"
            ));
        }
        state.in_flight.remove(name);
        state.activated.insert(
            name.to_string(),
            ActivatedSkill {
                key: Some(flight.key.clone()),
                content: Some(content),
                sandbox_policy,
            },
        );
        Ok(())
    }

    fn abandon(&self, name: &str, flight: &Arc<ActivationFlight>) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state
            .in_flight
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            state.in_flight.remove(name);
        }
    }

    fn poison(&self, name: &str, flight: &Arc<ActivationFlight>) {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state
            .in_flight
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            flight
                .abandoned
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

// -- SkillRegistry --

/// Central skill lifecycle manager.
///
/// Tracks both code-based skills (registered via [`Skill`](crate::skills::Skill) trait)
/// and file-based skills (discovered from `SKILL.md` files). Provides:
///
/// - **Catalog generation** for system-prompt injection (Tier 1)
/// - **Activation tracking** with deduplication (Tier 2)
/// - **Resource access** from activated skill directories (Tier 3)
pub struct SkillRegistry {
    /// File-based skills: name -> descriptor (Tier 1 metadata)
    descriptors: HashMap<String, SkillDescriptor>,

    /// Frozen SKILL.md documents supplied by a prepared plugin generation.
    /// Non-plugin discovery intentionally remains lazy and filesystem-backed.
    prepared_documents: HashMap<String, SkillDocument>,

    /// Shared runtime activation authority.
    activation: SkillActivationHandle,

    /// Code-based skills: name -> info (registered via `add_skill`)
    code_skills: HashMap<String, SkillInfo>,

    /// Optional sandbox manager used when activating local skills with inline commands.
    sandbox: Option<Arc<dyn SandboxExecutor>>,

    /// Reverse index: source tag (e.g. `"plugin:my-plugin"`) -> skill names
    /// registered under that source. Lets `unregister_by_source` remove
    /// exactly one plugin's skills on disable/uninstall without scanning all
    /// descriptors. Skills without a `source` are absent from this map.
    by_source: HashMap<String, HashSet<String>>,

    /// Plugin variable context keyed by skill name.
    plugin_variables: HashMap<String, echo_core::plugin::PluginVariables>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            descriptors: HashMap::new(),
            prepared_documents: HashMap::new(),
            activation: SkillActivationHandle::new(),
            code_skills: HashMap::new(),
            sandbox: None,
            by_source: HashMap::new(),
            plugin_variables: HashMap::new(),
        }
    }

    /// Return the canonical process-local activation authority.
    pub fn activation_handle(&self) -> SkillActivationHandle {
        self.activation.clone()
    }

    /// Construct an empty definition view that shares this registry's runtime
    /// activation authority.
    ///
    /// Callers may populate descriptors and prepared documents for concurrent
    /// resource tools without creating another activation set or sandbox-policy
    /// authority.
    pub fn activation_view(&self) -> Self {
        let mut registry = Self::new();
        registry.activation = self.activation.clone();
        registry
    }

    // -- File-based skills (progressive disclosure) --

    /// Register a discovered file-based skill descriptor.
    pub fn register_descriptor(&mut self, descriptor: SkillDescriptor) {
        self.prepared_documents.remove(&descriptor.name);
        self.insert_descriptor(descriptor);
    }

    fn insert_descriptor(&mut self, descriptor: SkillDescriptor) {
        let replacing = self.descriptors.contains_key(&descriptor.name);
        // Validate paths during registration
        for warning in descriptor.validate_paths() {
            warn!("Skill '{}': {}", descriptor.name, warning);
        }
        // If a caller replaces a descriptor directly, remove the old reverse
        // index first. Discovery normally rejects duplicate names, but this
        // keeps the public registry internally consistent for all callers.
        if let Some(previous_source) = self
            .descriptors
            .get(&descriptor.name)
            .and_then(|previous| previous.source.as_deref())
            .map(str::to_string)
            && let Some(names) = self.by_source.get_mut(&previous_source)
        {
            names.remove(&descriptor.name);
            if names.is_empty() {
                self.by_source.remove(&previous_source);
            }
        }
        // Track source provenance so disable/uninstall can remove exactly
        // this group later (P1-reload).
        if let Some(src) = descriptor.source.as_deref() {
            self.by_source
                .entry(src.to_string())
                .or_default()
                .insert(descriptor.name.clone());
        }
        let name = descriptor.name.clone();
        self.descriptors.insert(name.clone(), descriptor);
        if replacing {
            self.activation.retire(&name);
        }
    }

    /// Remove all skills registered under a given source tag (e.g.
    /// `"plugin:my-plugin"`), returning the number removed.
    ///
    /// Used by the plugin runtime on disable/uninstall so the agent's
    /// SkillRegistry doesn't keep a disabled plugin's skills. Each removed
    /// skill is also deactivated and purged from runtime bookkeeping
    /// via `remove_descriptor`.
    pub fn unregister_by_source(&mut self, source: &str) -> usize {
        self.unregister_names_by_source(source).len()
    }

    /// Remove and return all skill names registered by one source.
    pub fn unregister_names_by_source(&mut self, source: &str) -> Vec<String> {
        let names = match self.by_source.remove(source) {
            Some(set) => set,
            None => return Vec::new(),
        };
        let mut removed = Vec::new();
        for name in names {
            if self.remove_descriptor(&name) {
                removed.push(name);
            }
        }
        removed.sort();
        removed
    }

    /// Tag already-registered skills with a source so they can be group-unloaded
    /// later via `unregister_by_source`.
    ///
    /// This is the post-load entry point for the plugin integrator: it calls
    /// `load_skills_from_dir` (which discovers + registers descriptors without
    /// knowing the source), then tags the returned names with the plugin
    /// source. Skills already tagged under a different source are skipped to
    /// avoid cross-plugin contamination.
    pub fn tag_source(&mut self, names: &[String], source: &str) {
        self.tag_source_with_variables(names, source, None);
    }

    /// Tag skills with their source and plugin substitution context.
    pub fn tag_source_with_variables(
        &mut self,
        names: &[String],
        source: &str,
        variables: Option<&echo_core::plugin::PluginVariables>,
    ) {
        for name in names {
            let mut activation_input_changed = false;
            if let Some(desc) = self.descriptors.get_mut(name) {
                // Only tag if not already owned by another source.
                if desc.source.is_none() {
                    desc.source = Some(source.to_string());
                    self.by_source
                        .entry(source.to_string())
                        .or_default()
                        .insert(name.clone());
                }
                if desc.source.as_deref() == Some(source)
                    && let Some(variables) = variables
                {
                    self.plugin_variables
                        .insert(name.clone(), variables.clone());
                    activation_input_changed = true;
                }
            }
            if activation_input_changed {
                self.activation.retire(name);
            }
        }
    }

    /// Register one validated Skill from an immutable prepared generation.
    pub fn register_prepared(&mut self, document: SkillDocument) {
        let descriptor = document.descriptor().clone();
        let name = descriptor.name.clone();
        self.prepared_documents.insert(name, document);
        self.insert_descriptor(descriptor);
    }

    /// Remove one file-based descriptor and all of its activation metadata.
    pub fn remove_descriptor(&mut self, name: &str) -> bool {
        let removed_descriptor = self.descriptors.remove(name);
        if let Some(source) = removed_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.source.as_deref())
            .map(str::to_string)
            && let Some(names) = self.by_source.get_mut(&source)
        {
            names.remove(name);
            if names.is_empty() {
                self.by_source.remove(&source);
            }
        }
        self.prepared_documents.remove(name);
        self.plugin_variables.remove(name);
        if removed_descriptor.is_some() {
            self.activation.retire(name);
        }
        removed_descriptor.is_some()
    }

    /// Attach a sandbox manager used for inline command execution during activation.
    pub fn set_sandbox_manager(&mut self, manager: Arc<dyn SandboxExecutor>) {
        self.sandbox = Some(manager);
    }

    /// Get a descriptor by name.
    pub fn get_descriptor(&self, name: &str) -> Option<&SkillDescriptor> {
        self.descriptors.get(name)
    }

    /// List all discovered file-based skill descriptors.
    pub fn list_descriptors(&self) -> Vec<&SkillDescriptor> {
        let mut descs: Vec<&SkillDescriptor> = self.descriptors.values().collect();
        descs.sort_by_key(|d| &d.name);
        descs
    }

    /// Number of discovered file-based skills.
    pub fn descriptor_count(&self) -> usize {
        self.descriptors.len()
    }

    /// Generate the skill catalog text for system-prompt injection.
    ///
    /// Returns `None` if no file-based skills are available (caller should
    /// omit the catalog section entirely per spec).
    pub fn catalog_prompt(&self) -> Option<String> {
        if self.descriptors.is_empty() {
            return None;
        }

        let mut lines = Vec::with_capacity(self.descriptors.len() + 4);
        lines.push(
            "The following skills provide specialized instructions for specific tasks.\n\
             When a task matches a skill's description, call the `activate_skill` tool \
             with the skill's name to load its full instructions."
                .to_string(),
        );
        lines.push(String::new());

        let mut names: Vec<&String> = self.descriptors.keys().collect();
        names.sort();
        for name in names {
            if let Some(desc) = self.descriptors.get(name) {
                lines.push(desc.catalog_line());
            }
        }

        Some(lines.join("\n"))
    }

    /// List all available skill names (for `activate_skill` tool enum constraint).
    pub fn available_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.descriptors.keys().cloned().collect();
        names.sort();
        names
    }

    // -- Activation tracking --

    /// Mark a skill as activated. Returns `false` if already activated (dedup).
    pub fn mark_activated(&self, name: &str) -> bool {
        let sandbox_policy = self
            .descriptors
            .get(name)
            .and_then(|descriptor| descriptor.sandbox.clone())
            .filter(SkillSandboxPolicy::is_constraining);
        self.activation.mark_activated(name, sandbox_policy)
    }

    /// Clear session-local activation and sandbox policy state without
    /// unregistering descriptors.
    ///
    /// Runtime checkpoint identities may share one registry catalog while
    /// retaining independent model contexts. Switching identities must not
    /// carry activated skills or their sandbox policy into the next context.
    pub fn reset_activation_state(&self) {
        self.activation.reset();
    }

    /// Check whether a skill has been activated in this session.
    pub fn is_activated(&self, name: &str) -> bool {
        self.activation.is_activated(name)
    }

    /// Collect the union of `allowed_tools` from all currently activated skills.
    ///
    /// Returns `None` if no activated skill restricts tools (empty = unrestricted),
    /// meaning the agent may use any tool. Returns `Some(set)` when at least one
    /// activated skill declares an `allowed-tools` whitelist — in that case,
    /// only tools matching an entry in the returned set are permitted.
    pub fn active_skill_allowed_tools(&self) -> Option<HashSet<String>> {
        let activated = self.activation.activated_names();
        let mut allowed = HashSet::new();
        let mut any_restricted = false;

        for name in &activated {
            if let Some(desc) = self.descriptors.get(name)
                && !desc.allowed_tools.is_empty()
            {
                any_restricted = true;
                for tool in &desc.allowed_tools {
                    allowed.insert(tool.clone());
                }
            }
        }

        if any_restricted {
            Some(allowed)
        } else {
            None // No activated skill restricts tools → unrestricted
        }
    }

    /// Number of activated skills.
    pub fn activated_count(&self) -> usize {
        self.activation.activated_count()
    }

    /// Return all activated skill names as a sorted Vec.
    pub fn activated_names(&self) -> Vec<String> {
        self.activation.activated_names()
    }

    /// Replace activation state from a durable checkpoint in one atomic step.
    ///
    /// Unknown names are ignored because a checkpoint cannot recreate missing
    /// definitions. Sandbox policies are rebuilt from the currently installed
    /// descriptors rather than trusted as a second persisted authority.
    pub fn restore_activation_state(&self, names: &[String]) -> Vec<String> {
        let skills = names
            .iter()
            .filter_map(|name| {
                self.descriptors.get(name).map(|descriptor| {
                    let policy = descriptor
                        .sandbox
                        .clone()
                        .filter(SkillSandboxPolicy::is_constraining);
                    (name.clone(), policy)
                })
            })
            .collect();
        self.activation.restore(skills)
    }

    /// Activate a skill: read its full content from disk, execute inline
    /// commands, and substitute variables.
    ///
    /// Returns the structured `SkillContent` or an error.
    /// Automatically marks the skill as activated.
    pub async fn activate(&self, name: &str) -> echo_core::error::Result<SkillContent> {
        self.activate_with_args(name, &[], SkillSource::Local).await
    }

    /// Activate a skill with user-provided arguments and source context.
    ///
    /// This is the full activation path that:
    /// 1. Recursively activates dependencies first (if any)
    /// 2. Reads the `SKILL.md` body
    /// 3. Substitutes variables (`${SKILL_DIR}`, `${SESSION_ID}`, `${ARGUMENTS}`, etc.)
    /// 4. Executes inline commands (`` !`cmd` `` and `` ```! cmd ``` ``),
    ///    using the configured sandbox path when available, or the direct fallback
    ///    with minimal env + best-effort timeout termination otherwise
    /// 5. Enumerates bundled resources
    /// 6. Stores sandbox policy if declared
    pub async fn activate_with_args(
        &self,
        name: &str,
        args: &[String],
        source: SkillSource,
    ) -> echo_core::error::Result<SkillContent> {
        self.validate_activation_dependencies(name)?;
        if !self.descriptors.contains_key(name) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Skill '{name}' not found in catalog"
            )));
        }
        let key = ActivationKey {
            arguments: args.to_vec(),
            source,
        };
        let flight = match self.activation.claim(name, key) {
            ActivationClaim::Cached(content) => return Ok(*content),
            ActivationClaim::Restored => {
                return Err(echo_core::error::ReactError::Other(format!(
                    "Skill '{name}' was restored as active; reset activation state before re-activating it"
                )));
            }
            ActivationClaim::Abandoned => {
                return Err(echo_core::error::ReactError::Other(format!(
                    "Skill '{name}' activation was cancelled with unknown side-effect settlement; reset activation state before retrying"
                )));
            }
            ActivationClaim::Conflict => {
                return Err(echo_core::error::ReactError::Other(format!(
                    "Skill '{name}' is already activating or active with different arguments"
                )));
            }
            ActivationClaim::Flight(flight) => flight,
        };

        let outcome = flight
            .result
            .get_or_init(|| async {
                if flight
                    .abandoned
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    return Err(format!(
                        "Skill '{name}' activation was cancelled with unknown side-effect settlement; reset activation state before retrying"
                    ));
                }
                let mut guard = ActivationAttemptGuard::new(
                    self.activation.clone(),
                    name,
                    Arc::clone(&flight),
                );
                let result = self
                    .compute_activation(name, args, source, &flight)
                    .await
                    .map_err(|error| error.to_string());
                guard.complete();
                result
            })
            .await
            .clone();
        if outcome.is_err() && !flight.abandoned.load(std::sync::atomic::Ordering::SeqCst) {
            self.activation.abandon(name, &flight);
        }
        outcome.map_err(echo_core::error::ReactError::Other)
    }

    async fn compute_activation(
        &self,
        name: &str,
        args: &[String],
        source: SkillSource,
        flight: &Arc<ActivationFlight>,
    ) -> echo_core::error::Result<SkillContent> {
        let deps_activated = self.activate_dependencies(name, source).await?;
        let descriptor = self.descriptors.get(name).cloned().ok_or_else(|| {
            echo_core::error::ReactError::Other(format!("Skill '{name}' not found in catalog"))
        })?;
        let location = descriptor.location.clone();
        let skill_dir = location
            .parent()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(format!(
                    "Cannot determine skill directory from '{}'",
                    location.display()
                ))
            })?;
        let document = match self.prepared_documents.get(name) {
            Some(document) => document.clone(),
            None => {
                let raw_content = tokio::fs::read_to_string(&location)
                    .await
                    .map_err(|error| {
                        echo_core::error::ReactError::Other(format!(
                            "Failed to read SKILL.md at '{}': {error}",
                            location.display()
                        ))
                    })?;
                SkillDocument::parse_at(&raw_content, location)?
            }
        };
        let mut raw_instructions = document.instructions().to_string();
        if let Some(variables) = self.plugin_variables.get(name) {
            raw_instructions = variables.substitute(&raw_instructions);
        }
        let context = PromptContext {
            skill_dir: skill_dir.display().to_string(),
            session_id: self.activation.session_id(),
            arguments: args.to_vec(),
            shell: descriptor.shell.clone(),
            source,
            sandbox: self.sandbox.clone(),
            ..Default::default()
        };
        let instructions = process_skill_content(&raw_instructions, &context).await;
        let resources = enumerate_resources(&skill_dir).await;
        let instructions = if deps_activated.is_empty() {
            instructions
        } else {
            format!(
                "<skill-dependencies>\nActivated dependencies: {}\n</skill-dependencies>\n\n{}",
                deps_activated.join(", "),
                instructions
            )
        };
        let content = SkillContent {
            descriptor: descriptor.clone(),
            instructions,
            resources,
        };
        let sandbox_policy = descriptor
            .sandbox
            .filter(SkillSandboxPolicy::is_constraining);
        self.activation
            .publish(name, flight, content.clone(), sandbox_policy)
            .map_err(echo_core::error::ReactError::Other)?;
        Ok(content)
    }

    fn validate_activation_dependencies(&self, name: &str) -> echo_core::error::Result<()> {
        fn visit(
            registry: &SkillRegistry,
            name: &str,
            visiting: &mut HashSet<String>,
            visited: &mut HashSet<String>,
            path: &mut Vec<String>,
        ) -> echo_core::error::Result<()> {
            if visited.contains(name) {
                return Ok(());
            }
            if !visiting.insert(name.to_string()) {
                path.push(name.to_string());
                let cycle = path.join(" -> ");
                path.pop();
                return Err(echo_core::error::ReactError::Other(format!(
                    "Circular skill dependency detected: {cycle}"
                )));
            }
            path.push(name.to_string());
            if let Some(descriptor) = registry.descriptors.get(name) {
                for dependency in &descriptor.depends_on {
                    if registry.descriptors.contains_key(dependency) {
                        visit(registry, dependency, visiting, visited, path)?;
                    }
                }
            }
            path.pop();
            visiting.remove(name);
            visited.insert(name.to_string());
            Ok(())
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        visit(self, name, &mut visiting, &mut visited, &mut Vec::new())
    }

    /// Recursively activate unmet dependencies for a skill.
    ///
    /// Returns the list of dependency names that were newly activated.
    /// Missing dependencies are logged as warnings but don't block activation.
    async fn activate_dependencies(
        &self,
        name: &str,
        source: SkillSource,
    ) -> echo_core::error::Result<Vec<String>> {
        let deps = self
            .descriptors
            .get(name)
            .map(|d| d.depends_on.clone())
            .unwrap_or_default();

        if deps.is_empty() {
            return Ok(Vec::new());
        }

        let mut activated = Vec::new();
        for dep in &deps {
            if self.activation.is_activated(dep) {
                continue;
            }
            if !self.descriptors.contains_key(dep) {
                warn!(
                    "Skill '{}' depends on '{}' which is not available; skipping",
                    name, dep
                );
                continue;
            }
            // Recursive activation (deps of deps)
            match Box::pin(self.activate_with_args(dep, &[], source)).await {
                Ok(_) => activated.push(dep.clone()),
                Err(e) => {
                    warn!(
                        "Failed to activate dependency '{}' of '{}': {}",
                        dep, name, e
                    );
                }
            }
        }
        Ok(activated)
    }

    /// Get the active sandbox policy for an activated skill.
    pub fn get_active_sandbox_policy(&self, skill_name: &str) -> Option<SkillSandboxPolicy> {
        self.activation.active_sandbox_policy(skill_name)
    }

    /// Get the full dependency tree for a skill (recursive, depth-first).
    pub fn get_dependency_tree(&self, name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.collect_dependencies(name, &mut result, &mut visited);
        result
    }

    fn collect_dependencies(
        &self,
        name: &str,
        result: &mut Vec<String>,
        visited: &mut HashSet<String>,
    ) {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());

        if let Some(descriptor) = self.descriptors.get(name) {
            for dep in &descriptor.depends_on {
                self.collect_dependencies(dep, result, visited);
                if !result.contains(dep) {
                    result.push(dep.clone());
                }
            }
        }
    }

    // -- Code-based skills --

    /// Record a code-based skill that was installed via `add_skill`.
    pub fn record_code_skill(&mut self, info: SkillInfo) {
        self.code_skills.insert(info.name.clone(), info);
    }

    /// Check if a code-based skill is installed.
    pub fn has_code_skill(&self, name: &str) -> bool {
        self.code_skills.contains_key(name)
    }

    /// Get a code-based skill's info.
    pub fn get_code_skill(&self, name: &str) -> Option<&SkillInfo> {
        self.code_skills.get(name)
    }

    /// List all installed code-based skills.
    pub fn list_code_skills(&self) -> Vec<&SkillInfo> {
        let mut infos: Vec<&SkillInfo> = self.code_skills.values().collect();
        infos.sort_by_key(|i| &i.name);
        infos
    }

    // -- Unified queries --

    /// Check if a skill (code-based or file-based) is installed/discovered.
    pub fn is_installed(&self, name: &str) -> bool {
        self.code_skills.contains_key(name) || self.descriptors.contains_key(name)
    }

    /// Total number of skills (code + file-based).
    pub fn count(&self) -> usize {
        self.code_skills.len() + self.descriptors.len()
    }

    /// List all installed skills as `SkillInfo` (unified view).
    pub fn list(&self) -> Vec<&SkillInfo> {
        self.list_code_skills()
    }

    /// Core methodology skills that are injected directly into the system
    /// prompt at session start (not just listed in the catalog).
    pub const DEFAULT_BASELINE_SKILLS: &'static [&'static str] = &[
        "brainstorming",
        "systematic-debugging",
        "verification-before-completion",
        "writing-plans",
    ];

    /// Inject baseline methodology skill bodies into the system prompt.
    ///
    /// Reads the SKILL.md body from disk for each enabled baseline skill
    /// whose metadata.category == "methodology", strips the YAML frontmatter,
    /// and appends the body wrapped in a `<skill>` tag.
    pub fn inject_methodology_baseline(
        &self,
        system_prompt: &mut String,
        enabled_baseline: &[&str],
    ) {
        for desc in self.descriptors.values() {
            let category = desc
                .metadata
                .get("category")
                .map(|s| s.as_str())
                .unwrap_or("");
            if category != "methodology" {
                continue;
            }
            if !enabled_baseline.iter().any(|name| *name == desc.name) {
                continue;
            }
            let skill_file =
                if desc.location.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                    desc.location.clone()
                } else {
                    desc.location.join("SKILL.md")
                };
            let document = match self.prepared_documents.get(&desc.name) {
                Some(document) => document.clone(),
                None => {
                    let Ok(content) = std::fs::read_to_string(&skill_file) else {
                        continue;
                    };
                    let Ok(document) = SkillDocument::parse_at(&content, &skill_file) else {
                        continue;
                    };
                    document
                }
            };
            let raw_body = document.instructions().trim();
            let substituted;
            let body = if let Some(variables) = self.plugin_variables.get(&desc.name) {
                substituted = variables.substitute(raw_body);
                substituted.trim()
            } else {
                raw_body
            };
            if body.is_empty() {
                continue;
            }
            system_prompt.push_str("\n\n<skill name=\"");
            system_prompt.push_str(&desc.name);
            system_prompt.push_str("\">\n");
            system_prompt.push_str(body);
            system_prompt.push_str("\n</skill>");
        }
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// -- Shared registry handle (for tools that need concurrent access) --

/// Thread-safe handle to a `SkillRegistry`, shared between the agent and tools.
pub type SharedRegistry = Arc<RwLock<SkillRegistry>>;

/// Create a new shared registry from an existing one.
pub fn shared_registry(registry: SkillRegistry) -> SharedRegistry {
    Arc::new(RwLock::new(registry))
}

/// Enumerate resource files in `scripts/`, `references/`, `assets/` under a skill dir.
async fn enumerate_resources(skill_dir: &std::path::Path) -> Vec<SkillResourceEntry> {
    let mut resources = Vec::new();

    let dirs = [
        ("scripts", SkillResourceKind::Script),
        ("references", SkillResourceKind::Reference),
        ("assets", SkillResourceKind::Asset),
    ];

    for (dir_name, kind) in &dirs {
        let dir_path = skill_dir.join(dir_name);
        if !dir_path.is_dir() {
            continue;
        }
        if let Ok(mut entries) = tokio::fs::read_dir(&dir_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file()
                    && let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                {
                    resources.push(SkillResourceEntry {
                        relative_path: format!("{}/{}", dir_name, file_name),
                        kind: *kind,
                    });
                }
            }
        }
    }

    // Also enumerate top-level text resources that are not SKILL.md.
    if let Ok(mut entries) = tokio::fs::read_dir(skill_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name != "SKILL.md"
                && (name.ends_with(".md")
                    || name.ends_with(".txt")
                    || name.ends_with(".yaml")
                    || name.ends_with(".yml")
                    || name.ends_with(".json"))
            {
                resources.push(SkillResourceEntry {
                    relative_path: name.to_string(),
                    kind: SkillResourceKind::Other,
                });
            }
        }
    }

    resources.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    resources
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::sandbox::{ExecutionResult, IsolationLevel, SandboxCommand};
    use futures::future::BoxFuture;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct BlockingSandbox {
        calls: AtomicUsize,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl BlockingSandbox {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            }
        }
    }

    impl SandboxExecutor for BlockingSandbox {
        fn name(&self) -> &str {
            "blocking-skill-test"
        }

        fn isolation_level(&self) -> IsolationLevel {
            IsolationLevel::Process
        }

        fn is_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn execute(
            &self,
            _command: SandboxCommand,
        ) -> BoxFuture<'_, echo_core::error::Result<ExecutionResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                self.release.notified().await;
                Ok(ExecutionResult {
                    exit_code: 0,
                    stdout: "once".to_string(),
                    stderr: String::new(),
                    duration: std::time::Duration::from_millis(1),
                    sandbox_type: self.name().to_string(),
                    timed_out: false,
                    cancelled: false,
                    output_truncated: false,
                    stdout_bytes: 4,
                    stderr_bytes: 0,
                })
            })
        }
    }

    fn make_descriptor(name: &str, desc: &str) -> SkillDescriptor {
        SkillDescriptor {
            source: None,
            name: name.into(),
            description: desc.into(),
            location: PathBuf::from(format!("/skills/{}/SKILL.md", name)),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        }
    }

    fn inline_descriptor(root: &std::path::Path, name: &str) -> SkillDescriptor {
        SkillDescriptor {
            location: root.join(name).join("SKILL.md"),
            ..make_descriptor(name, "Inline activation test")
        }
    }

    fn write_inline_skill(root: &std::path::Path, name: &str) -> echo_core::error::Result<()> {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Inline activation test\n---\n\nResult ${{ARGUMENTS}}: !`count-once`\n"
            ),
        )?;
        Ok(())
    }

    fn skill_test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "echo-skill-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn activation_dependency_validation_rejects_cycles() {
        let mut registry = SkillRegistry::new();
        let mut a = make_descriptor("a", "A");
        a.depends_on = vec!["b".to_string()];
        let mut b = make_descriptor("b", "B");
        b.depends_on = vec!["a".to_string()];
        registry.register_descriptor(a);
        registry.register_descriptor(b);

        let error = registry
            .validate_activation_dependencies("a")
            .err()
            .unwrap_or_else(|| echo_core::error::ReactError::Other("missing error".to_string()));
        assert!(error.to_string().contains("a -> b -> a"));
        assert!(registry.activated_names().is_empty());
    }

    #[test]
    fn activation_dependency_validation_rejects_self_dependency() {
        let mut registry = SkillRegistry::new();
        let mut skill = make_descriptor("self", "Self");
        skill.depends_on = vec!["self".to_string()];
        registry.register_descriptor(skill);

        let error = registry
            .validate_activation_dependencies("self")
            .err()
            .unwrap_or_else(|| echo_core::error::ReactError::Other("missing error".to_string()));
        assert!(error.to_string().contains("self -> self"));
        assert!(registry.activated_names().is_empty());
    }

    #[test]
    fn test_registry_new() {
        let reg = SkillRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.catalog_prompt().is_none());
    }

    #[test]
    fn reset_activation_state_preserves_catalog() {
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(make_descriptor("code-review", "Review code quality"));
        assert!(registry.mark_activated("code-review"));
        registry.reset_activation_state();
        assert!(registry.activated_names().is_empty());
        assert_eq!(registry.available_names(), vec!["code-review".to_string()]);
    }

    #[test]
    fn test_register_descriptor() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("code-review", "Review code quality"));

        assert_eq!(reg.descriptor_count(), 1);
        assert!(reg.get_descriptor("code-review").is_some());
        assert!(reg.is_installed("code-review"));
    }

    #[test]
    fn source_group_unload_returns_exact_skill_names() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("plugin-a-one", "one"));
        reg.register_descriptor(make_descriptor("plugin-a-two", "two"));
        reg.register_descriptor(make_descriptor("baseline", "baseline"));
        reg.tag_source(
            &["plugin-a-one".to_string(), "plugin-a-two".to_string()],
            "plugin:a",
        );

        assert_eq!(
            reg.unregister_names_by_source("plugin:a"),
            vec!["plugin-a-one".to_string(), "plugin-a-two".to_string()]
        );
        assert!(reg.get_descriptor("plugin-a-one").is_none());
        assert!(reg.get_descriptor("plugin-a-two").is_none());
        assert!(reg.get_descriptor("baseline").is_some());
    }

    #[test]
    fn replacing_descriptor_updates_source_reverse_index() {
        let mut reg = SkillRegistry::new();
        let mut first = make_descriptor("shared", "first");
        first.source = Some("plugin:a".to_string());
        reg.register_descriptor(first);
        let mut replacement = make_descriptor("shared", "replacement");
        replacement.source = Some("plugin:b".to_string());
        reg.register_descriptor(replacement);

        assert_eq!(reg.unregister_by_source("plugin:a"), 0);
        assert_eq!(reg.unregister_by_source("plugin:b"), 1);
        assert!(reg.get_descriptor("shared").is_none());
    }

    #[test]
    fn test_catalog_prompt() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("code-review", "Review code"));
        reg.register_descriptor(make_descriptor("data-analysis", "Analyze data"));

        let catalog = reg.catalog_prompt().unwrap();
        assert!(catalog.contains("activate_skill"));
        assert!(catalog.contains("- code-review: Review code"));
        assert!(catalog.contains("- data-analysis: Analyze data"));
    }

    #[test]
    fn test_activation_tracking() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("test", "Test skill"));

        assert!(!reg.is_activated("test"));
        assert!(reg.mark_activated("test"));
        assert!(reg.is_activated("test"));
        assert!(!reg.mark_activated("test")); // dedup
        assert_eq!(reg.activated_count(), 1);
    }

    #[test]
    fn activation_state_is_shared_across_registry_views_and_reset_is_idempotent() {
        let mut primary = SkillRegistry::new();
        primary.register_descriptor(make_descriptor("shared", "Shared skill"));

        let mut progressive = primary.activation_view();
        progressive.register_descriptor(make_descriptor("shared", "Shared skill"));

        assert!(primary.mark_activated("shared"));
        assert!(progressive.is_activated("shared"));
        assert!(!progressive.mark_activated("shared"));

        progressive.reset_activation_state();
        assert!(!primary.is_activated("shared"));
        primary.reset_activation_state();
        assert!(primary.activated_names().is_empty());
    }

    #[test]
    fn removing_a_definition_clears_shared_activation_state() {
        let mut primary = SkillRegistry::new();
        primary.register_descriptor(make_descriptor("shared", "Shared skill"));
        let mut progressive = primary.activation_view();
        progressive.register_descriptor(make_descriptor("shared", "Shared skill"));
        assert!(primary.mark_activated("shared"));

        assert!(progressive.remove_descriptor("shared"));
        assert!(!primary.is_activated("shared"));
        assert!(primary.activated_names().is_empty());
    }

    #[tokio::test]
    async fn concurrent_repeated_activation_executes_inline_command_once()
    -> echo_core::error::Result<()> {
        let root = skill_test_root("single-flight");
        write_inline_skill(&root, "shared")?;
        let sandbox = Arc::new(BlockingSandbox::new());
        let descriptor = inline_descriptor(&root, "shared");
        let mut primary = SkillRegistry::new();
        primary.register_descriptor(descriptor.clone());
        primary.set_sandbox_manager(sandbox.clone());
        let mut progressive = primary.activation_view();
        progressive.register_descriptor(descriptor);
        progressive.set_sandbox_manager(sandbox.clone());
        let primary = Arc::new(primary);
        let progressive = Arc::new(progressive);

        let entered = sandbox.entered.notified();
        let first_registry = Arc::clone(&primary);
        let first = tokio::spawn(async move { first_registry.activate("shared").await });
        entered.await;
        let second_registry = Arc::clone(&progressive);
        let second = tokio::spawn(async move { second_registry.activate("shared").await });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let joined = {
                    let state = primary
                        .activation
                        .shared
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    state
                        .in_flight
                        .get("shared")
                        .is_some_and(|flight| flight.waiters.load(Ordering::SeqCst) >= 1)
                };
                if joined {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
        sandbox.release.notify_one();

        let first_content = first
            .await
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))??;
        let second_content = second
            .await
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))??;
        let repeated_content = progressive.activate("shared").await?;
        assert_eq!(sandbox.calls.load(Ordering::SeqCst), 1);
        assert_eq!(first_content.instructions, second_content.instructions);
        assert_eq!(second_content.instructions, repeated_content.instructions);
        assert!(primary.is_activated("shared"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn parameterized_activation_executes_once_per_distinct_key()
    -> echo_core::error::Result<()> {
        let root = skill_test_root("parameterized-key");
        write_inline_skill(&root, "parameterized")?;
        let sandbox = Arc::new(BlockingSandbox::new());
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(inline_descriptor(&root, "parameterized"));
        registry.set_sandbox_manager(sandbox.clone());

        sandbox.release.notify_one();
        let first = registry
            .activate_with_args("parameterized", &["first".to_string()], SkillSource::Local)
            .await?;
        sandbox.release.notify_one();
        let second = registry
            .activate_with_args("parameterized", &["second".to_string()], SkillSource::Local)
            .await?;
        let repeated = registry
            .activate_with_args("parameterized", &["second".to_string()], SkillSource::Local)
            .await?;

        assert_eq!(sandbox.calls.load(Ordering::SeqCst), 2);
        assert_eq!(second.instructions, repeated.instructions);
        assert_ne!(first.instructions, second.instructions);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn different_activation_key_cannot_overlap_in_flight_effect()
    -> echo_core::error::Result<()> {
        let root = skill_test_root("parameterized-conflict");
        write_inline_skill(&root, "parameterized")?;
        let sandbox = Arc::new(BlockingSandbox::new());
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(inline_descriptor(&root, "parameterized"));
        registry.set_sandbox_manager(sandbox.clone());
        let registry = Arc::new(registry);

        let entered = sandbox.entered.notified();
        let first_registry = Arc::clone(&registry);
        let first = tokio::spawn(async move {
            first_registry
                .activate_with_args("parameterized", &["first".to_string()], SkillSource::Local)
                .await
        });
        entered.await;
        let conflict = registry
            .activate_with_args("parameterized", &["second".to_string()], SkillSource::Local)
            .await
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(
                    "different activation key overlapped in-flight effect".to_string(),
                )
            })?;
        assert!(conflict.to_string().contains("different arguments"));
        assert_eq!(sandbox.calls.load(Ordering::SeqCst), 1);
        sandbox.release.notify_one();
        first
            .await
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))??;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_activation_requires_reset_before_effect_retry()
    -> echo_core::error::Result<()> {
        let root = skill_test_root("cancelled-flight");
        write_inline_skill(&root, "cancelled-skill")?;
        let sandbox = Arc::new(BlockingSandbox::new());
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(inline_descriptor(&root, "cancelled-skill"));
        registry.set_sandbox_manager(sandbox.clone());
        let registry = Arc::new(registry);

        let entered = sandbox.entered.notified();
        let activating_registry = Arc::clone(&registry);
        let activation =
            tokio::spawn(async move { activating_registry.activate("cancelled-skill").await });
        entered.await;
        activation.abort();
        assert!(activation.await.is_err());

        let retry = registry
            .activate("cancelled-skill")
            .await
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(
                    "cancelled activation replayed without reset".to_string(),
                )
            })?;
        assert!(retry.to_string().contains("unknown side-effect settlement"));
        assert_eq!(sandbox.calls.load(Ordering::SeqCst), 1);

        registry.reset_activation_state();
        sandbox.release.notify_one();
        registry.activate("cancelled-skill").await?;
        assert_eq!(sandbox.calls.load(Ordering::SeqCst), 2);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn reset_fences_in_flight_activation_publication() -> echo_core::error::Result<()> {
        let root = skill_test_root("reset-fence");
        write_inline_skill(&root, "reset-skill")?;
        let sandbox = Arc::new(BlockingSandbox::new());
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(inline_descriptor(&root, "reset-skill"));
        registry.set_sandbox_manager(sandbox.clone());
        let registry = Arc::new(registry);

        let entered = sandbox.entered.notified();
        let activating_registry = Arc::clone(&registry);
        let activation =
            tokio::spawn(async move { activating_registry.activate("reset-skill").await });
        entered.await;
        registry.reset_activation_state();
        sandbox.release.notify_one();

        let error = activation
            .await
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(
                    "retired activation unexpectedly succeeded".to_string(),
                )
            })?;
        assert!(error.to_string().contains("retired before publication"));
        assert!(!registry.is_activated("reset-skill"));
        assert!(registry.get_active_sandbox_policy("reset-skill").is_none());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn remove_fences_in_flight_activation_from_another_view() -> echo_core::error::Result<()>
    {
        let root = skill_test_root("remove-fence");
        write_inline_skill(&root, "removed-skill")?;
        let descriptor = inline_descriptor(&root, "removed-skill");
        let sandbox = Arc::new(BlockingSandbox::new());
        let mut primary = SkillRegistry::new();
        primary.register_descriptor(descriptor.clone());
        let mut progressive = primary.activation_view();
        progressive.register_descriptor(descriptor);
        progressive.set_sandbox_manager(sandbox.clone());
        let progressive = Arc::new(progressive);

        let entered = sandbox.entered.notified();
        let activating_registry = Arc::clone(&progressive);
        let activation =
            tokio::spawn(async move { activating_registry.activate("removed-skill").await });
        entered.await;
        assert!(primary.remove_descriptor("removed-skill"));
        sandbox.release.notify_one();

        let error = activation
            .await
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(
                    "removed activation unexpectedly succeeded".to_string(),
                )
            })?;
        assert!(error.to_string().contains("retired before publication"));
        assert!(!progressive.is_activated("removed-skill"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn test_code_skills() {
        let mut reg = SkillRegistry::new();
        reg.record_code_skill(SkillInfo {
            name: "calculator".into(),
            description: "Math operations".into(),
            tool_names: vec!["add".into(), "subtract".into()],
            has_prompt_injection: true,
        });

        assert!(reg.has_code_skill("calculator"));
        assert!(reg.is_installed("calculator"));
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn test_available_names() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("b-skill", "B"));
        reg.register_descriptor(make_descriptor("a-skill", "A"));

        let names = reg.available_names();
        assert_eq!(names, vec!["a-skill", "b-skill"]);
    }

    #[test]
    fn test_mixed_skills() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("file-skill", "File-based"));
        reg.record_code_skill(SkillInfo {
            name: "code-skill".into(),
            description: "Code-based".into(),
            tool_names: vec![],
            has_prompt_injection: false,
        });

        assert_eq!(reg.count(), 2);
        assert!(reg.is_installed("file-skill"));
        assert!(reg.is_installed("code-skill"));
        assert!(!reg.is_installed("missing"));
    }

    fn make_descriptor_with_tools(name: &str, allowed: Vec<&str>) -> SkillDescriptor {
        let mut desc = make_descriptor(name, &format!("{} skill", name));
        desc.allowed_tools = allowed.into_iter().map(String::from).collect();
        desc
    }

    #[test]
    fn test_active_skill_allowed_tools_empty_when_no_skills_activated() {
        let reg = SkillRegistry::new();
        assert!(
            reg.active_skill_allowed_tools().is_none(),
            "No activated skills → unrestricted"
        );
    }

    #[test]
    fn test_active_skill_allowed_tools_empty_when_no_restrictions() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("open-skill", "No tool restrictions"));
        reg.mark_activated("open-skill");

        assert!(
            reg.active_skill_allowed_tools().is_none(),
            "Activated skill with empty allowed_tools → unrestricted"
        );
    }

    #[test]
    fn test_active_skill_allowed_tools_returns_whitelist() {
        let mut reg = SkillRegistry::new();

        // Medical skill: only research tools, no shell
        reg.register_descriptor(make_descriptor_with_tools(
            "evidence-medicine",
            vec!["Read", "Write", "Edit", "WebSearch", "PubMedSearch"],
        ));
        reg.mark_activated("evidence-medicine");

        let allowed = reg
            .active_skill_allowed_tools()
            .expect("Should return Some when skill restricts tools");

        assert!(allowed.contains("Read"));
        assert!(allowed.contains("PubMedSearch"));
        assert!(!allowed.contains("Bash"));
        assert!(!allowed.contains("Shell"));
    }

    #[test]
    fn test_active_skill_allowed_tools_union_of_multiple_skills() {
        let mut reg = SkillRegistry::new();

        reg.register_descriptor(make_descriptor_with_tools(
            "coding",
            vec!["Bash(*)", "Read", "Write", "Edit", "Glob", "Grep"],
        ));
        reg.register_descriptor(make_descriptor_with_tools(
            "git-workflow",
            vec!["Bash(git:*)", "Read", "Glob"],
        ));

        // Activate both skills
        reg.mark_activated("coding");
        reg.mark_activated("git-workflow");

        let allowed = reg
            .active_skill_allowed_tools()
            .expect("Should return union of allowed tools");

        // Should contain tools from both skills
        assert!(allowed.contains("Bash(*)"));
        assert!(allowed.contains("Bash(git:*)"));
        assert!(allowed.contains("Read"));
        assert!(allowed.contains("Grep"));
    }

    #[test]
    fn test_tool_matcher_rejects_disallowed_tool() {
        // Simulate the permission check that happens in stream_channel.rs
        let allowed: HashSet<String> = ["Read", "Write", "Edit", "WebSearch", "PubMedSearch"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let tool_name = "Bash";
        let permitted = allowed
            .iter()
            .any(|matcher| crate::skills::external::types::tool_matcher(matcher, tool_name));

        assert!(
            !permitted,
            "Bash should NOT be permitted by evidence-medicine's allowed-tools"
        );
    }

    #[test]
    fn test_tool_matcher_accepts_allowed_tool() {
        let allowed: HashSet<String> = ["Read", "Write", "Edit", "WebSearch", "PubMedSearch"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let tool_name = "PubMedSearch";
        let permitted = allowed
            .iter()
            .any(|matcher| crate::skills::external::types::tool_matcher(matcher, tool_name));

        assert!(
            permitted,
            "PubMedSearch should be permitted by evidence-medicine's allowed-tools"
        );
    }

    #[test]
    fn test_tool_matcher_glob_pattern() {
        let allowed: HashSet<String> = ["Bash(*)", "Read"].iter().map(|s| s.to_string()).collect();

        // Bash(*) should match Bash(git:status)
        let permitted = allowed.iter().any(|matcher| {
            crate::skills::external::types::tool_matcher(matcher, "Bash(git:status)")
        });
        assert!(permitted, "Bash(*) should match Bash(git:status)");

        // Bash(*) should NOT match Read
        let is_read = allowed
            .iter()
            .any(|matcher| crate::skills::external::types::tool_matcher(matcher, "Read"));
        assert!(is_read, "Read should be directly permitted");
    }
}
