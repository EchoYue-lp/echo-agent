//! ReactAgent capability configuration API
//!
//! - Tool registration (`add_tool` / `add_tools` / `add_need_appeal_tool`)
//! - Skill installation (`add_skill` / `add_skills` / `discover_skills` / `load_skills_from_dir`)
//! - MCP connections (`connect_mcp` / `load_mcp_from_file`)
//! - SubAgent, compressor, callbacks, etc.

use super::ReactAgent;
use crate::agent::Agent;
use crate::compression::{ContextCompressor, ForceCompressStats};
use crate::error::Result;
#[cfg(feature = "mcp")]
use crate::mcp::McpServerEntry;
#[cfg(feature = "mcp")]
use crate::mcp::{McpClient, McpConfigFile, McpServerConfig};
use crate::skills::external::activate_tool::ActivateSkillTool;
use crate::skills::external::loader::{DiscoveryScope, SkillLoader};
use crate::skills::external::resource_tool::ReadSkillResourceTool;
use crate::skills::external::run_script_tool::RunSkillScriptTool;
use crate::skills::registry::SharedRegistry;
use crate::skills::{Skill, SkillInfo};
use crate::tools::Tool;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

impl ReactAgent {
    // ── Tool registration ────────────────────────────────────────────────────

    /// Register a single tool. Automatically enables tool capability.
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.config.enable_tool = true;
        self.tool_manager.register(tool);
    }

    /// Register multiple tools. Automatically enables tool capability.
    pub fn add_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        if tools.is_empty() {
            return;
        }
        self.config.enable_tool = true;
        let allowed = &self.config.allowed_tools;
        if allowed.is_empty() {
            self.tool_manager.register_tools(tools);
        } else {
            for tool in tools {
                if allowed.contains(&tool.name().to_string()) {
                    self.tool_manager.register(tool);
                }
            }
        }
    }

    /// Remove a tool by name. Returns the removed tool if it existed.
    ///
    /// Supports dynamic tool management in the ReAct loop — for example,
    /// switching from search tools to execution tools mid-task.
    pub fn remove_tool(&mut self, name: &str) -> Option<Box<dyn Tool>> {
        self.tool_manager.unregister(name)
    }

    /// Replace an existing tool with a new one of the same name.
    ///
    /// If a tool with the same name exists, it is removed and the new tool is registered.
    /// Returns the old tool if it was replaced.
    pub fn replace_tool(&mut self, tool: Box<dyn Tool>) -> Option<Box<dyn Tool>> {
        let name = tool.name().to_string();
        let old = self.tool_manager.unregister(&name);
        self.tool_manager.register(tool);
        old
    }

    /// Register a tool that requires human approval before execution.
    pub fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
        #[cfg(feature = "human-loop")]
        if self.config.enable_human_in_loop {
            let tool_name = tool.name().to_string();
            self.add_tool(tool);

            // 优先添加到 PermissionService 规则
            if let Some(service) = &self.permission_service {
                use echo_core::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
                let rule = PermissionRule {
                    matcher: RuleMatcher::Pattern {
                        pattern: tool_name.clone(),
                    },
                    behavior: echo_core::tools::permission::RuleBehavior::Ask {
                        suggestions: vec!["允许".to_string(), "拒绝".to_string()],
                    },
                    source: RuleSource::Session,
                    description: Some(format!("工具 {} 需要人工审批", tool_name)),
                };
                // 同步添加规则（PermissionService 内部用 RwLock，这里用 block_on）
                let service = service.clone();
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async { service.add_rule(rule).await });
            } else {
                // 回退到旧的 HumanApprovalManager
                #[allow(deprecated)]
                self.human_in_loop
                    .write()
                    .map_err(|e| {
                        warn!("human_in_loop lock poisoned: {}", e);
                    })
                    .map(|mut guard| guard.mark_need_approval(tool_name))
                    .ok();
            }
            return;
        }
        #[cfg(not(feature = "human-loop"))]
        let _ = &self.config.enable_human_in_loop;
        warn!(
            agent = %self.config.agent_name,
            tool = %tool.name(),
            "human_in_loop disabled, tool registered without approval requirement"
        );
        self.add_tool(tool);
    }

    // ── Context compression ──────────────────────────────────────────────────

    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.context.set_compressor(compressor);
    }

    pub fn context_stats(&self) -> (usize, usize) {
        (self.context.messages().len(), self.context.token_estimate())
    }

    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<ForceCompressStats> {
        self.context.force_compress_with(compressor).await
    }

    pub fn list_tools(&self) -> Vec<&str> {
        self.tool_manager.list_tools()
    }

    // ── SubAgent ─────────────────────────────────────────────────────────────

    /// Register a subagent with the subagent registry.
    ///
    /// The agent is automatically wrapped in a default Sync-mode `SubagentDefinition`.
    /// For more control, use `register_subagent_with_definition()`.
    pub fn register_agent(&mut self, agent: Box<dyn Agent>) {
        if !self.config.enable_subagent {
            warn!(
                agent = %self.config.agent_name,
                subagent = %agent.name(),
                "subagent capability disabled, ignoring registration"
            );
            return;
        }
        let name = agent.name().to_string();
        let def = crate::agent::subagent::SubagentDefinition::simple_sync(&name);
        if self.subagent_registry.register_sync(def, agent) {
            info!(agent = %self.config.agent_name, subagent = %name, "Subagent registered");
        }
    }

    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    // ── Basic config ─────────────────────────────────────────────────────────

    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    pub fn add_callback(&mut self, callback: Arc<dyn crate::agent::AgentCallback>) {
        self.config.callbacks.push(callback);
    }

    // ── Skills (code-based) ──────────────────────────────────────────────────

    /// Install a code-based skill (eager: tools + prompt injected immediately).
    pub fn add_skill(&mut self, skill: Box<dyn Skill>) {
        let name = skill.name().to_string();

        if self.skill_registry.is_installed(&name) {
            warn!(
                agent = %self.config.agent_name,
                skill = %name,
                "Skill already installed, skipping"
            );
            return;
        }

        let tools = skill.tools();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        for tool in tools {
            self.tool_manager.register(tool);
        }

        let has_injection = skill.system_prompt_injection().is_some();
        if let Some(injection) = skill.system_prompt_injection() {
            self.config.system_prompt.push_str(&injection);
            self.context
                .update_system(self.config.system_prompt.clone());
        }

        self.skill_registry.record_code_skill(SkillInfo {
            name: name.clone(),
            description: skill.description().to_string(),
            tool_names,
            has_prompt_injection: has_injection,
        });

        info!(
            agent = %self.config.agent_name,
            skill = %name,
            description = %skill.description(),
            "Skill installed"
        );
    }

    /// Install multiple code-based skills.
    pub fn add_skills(&mut self, skills: Vec<Box<dyn Skill>>) {
        for skill in skills {
            self.add_skill(skill);
        }
    }

    // ── Skills (file-based, progressive disclosure) ──────────────────────────

    /// Discover file-based skills from the given scopes.
    ///
    /// Implements the agentskills.io progressive disclosure model:
    /// 1. Parse `SKILL.md` frontmatter (name + description) from each scope
    /// 2. Build a compact **catalog** and inject it into the system prompt
    /// 3. Register `activate_skill` and `read_skill_resource` tools
    ///
    /// The LLM can then decide which skills to activate on demand.
    pub async fn discover_skills(&mut self, scopes: &[DiscoveryScope]) -> Result<Vec<String>> {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover(scopes).await?;

        if descriptors.is_empty() {
            info!(
                agent = %self.config.agent_name,
                "No skills found during discovery"
            );
            return Ok(vec![]);
        }

        let mut names = Vec::new();

        // Build a shared registry for the progressive disclosure tools.
        // This is separate from `self.skill_registry` (which tracks code-based skills).
        // The shared registry holds descriptors + activation state, accessed by
        // both ActivateSkillTool and ReadSkillResourceTool during async execution.
        let tool_registry = {
            let mut reg = crate::skills::SkillRegistry::new();
            for desc in descriptors {
                if self.skill_registry.is_installed(&desc.name) {
                    warn!(
                        agent = %self.config.agent_name,
                        skill = %desc.name,
                        "Skill already installed, skipping duplicate"
                    );
                    continue;
                }

                // Register hooks from frontmatter if present
                if let Some(hooks_def) = &desc.hooks {
                    let skill_dir = desc
                        .location
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let mut hook_reg = self.hook_registry.write().await;
                    hook_reg.register(&desc.name, &skill_dir, hooks_def.clone());
                }

                names.push(desc.name.clone());
                self.skill_registry.register_descriptor(desc.clone());
                reg.register_descriptor(desc);
            }
            reg
        };

        if names.is_empty() {
            return Ok(names);
        }

        // Inject compact catalog into system prompt
        if let Some(catalog) = self.skill_registry.catalog_prompt() {
            self.config
                .system_prompt
                .push_str(&format!("\n\n{}", catalog));
            self.context
                .update_system(self.config.system_prompt.clone());
        }

        // Register progressive disclosure tools with shared registry
        let available_names = self.skill_registry.available_names();
        if self.tool_manager.get_tool("activate_skill").is_none() {
            let shared: SharedRegistry = Arc::new(RwLock::new(tool_registry));

            self.tool_manager.register(Box::new(ActivateSkillTool::new(
                shared.clone(),
                available_names,
            )));
            self.tool_manager
                .register(Box::new(ReadSkillResourceTool::new(shared.clone())));

            let mut script_tool = RunSkillScriptTool::new(shared);
            if let Some(ref manager) = self.sandbox_manager {
                script_tool = script_tool.with_sandbox_manager(manager.clone());
            }
            self.tool_manager.register(Box::new(script_tool));

            // Protect activated skill content from context compaction.
            // Messages containing <skill_content will survive compression passes.
            self.context
                .add_protected_marker("<skill_content".to_string());
        }

        info!(
            agent = %self.config.agent_name,
            count = names.len(),
            skills = ?names,
            "Skills discovered and catalog injected"
        );

        Ok(names)
    }

    /// Backward-compatible: discover skills from a single directory.
    ///
    /// Equivalent to `discover_skills(&[DiscoveryScope::Custom(path)])`.
    pub async fn load_skills_from_dir(
        &mut self,
        skills_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Vec<String>> {
        self.discover_skills(&[DiscoveryScope::Custom(skills_dir.into())])
            .await
    }

    /// List all installed code-based skills.
    pub fn list_skills(&self) -> Vec<&SkillInfo> {
        self.skill_registry.list()
    }

    /// Check if a skill (code or file-based) is installed.
    pub fn has_skill(&self, name: &str) -> bool {
        self.skill_registry.is_installed(name)
    }

    /// Total number of installed skills (code + file-based).
    pub fn skill_count(&self) -> usize {
        self.skill_registry.count()
    }

    /// Get the shared skill registry handle (for external tool access).
    pub fn skill_registry(&self) -> &crate::skills::SkillRegistry {
        &self.skill_registry
    }

    /// Get the shared hook registry handle.
    pub fn hook_registry(&self) -> &Arc<tokio::sync::RwLock<crate::skills::hooks::HookRegistry>> {
        &self.hook_registry
    }

    // ── MCP ──────────────────────────────────────────────────────────────────

    pub fn register_mcp_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.add_tools(tools);
    }

    #[cfg(feature = "mcp")]
    pub async fn connect_mcp_from_config(
        &mut self,
        config: McpServerConfig,
    ) -> crate::error::Result<Arc<McpClient>> {
        let name = config.name.clone();
        let tools = self.mcp_manager.connect(config).await?;
        let count = tools.len();
        self.add_tools(tools);
        let client = {
            let mgr = &self.mcp_manager;
            mgr.get_client(&name).ok_or_else(|| {
                crate::error::ReactError::Agent(crate::error::AgentError::InitializationFailed(
                    format!("MCP client '{}' not found after connection", name),
                ))
            })?
        };
        tracing::info!(
            agent = %self.config.agent_name,
            server = %name,
            tools = count,
            "MCP server connected"
        );
        Ok(client.clone())
    }

    #[cfg(feature = "mcp")]
    pub async fn connect_mcp_from_json(
        &mut self,
        name: &str,
        json_config_str: &str,
    ) -> crate::error::Result<Arc<McpClient>> {
        let entry = serde_json::from_str::<McpServerEntry>(json_config_str)?;
        let config = entry.to_server_config(name)?;
        self.connect_mcp_from_config(config).await
    }

    #[cfg(feature = "mcp")]
    pub async fn load_mcp_from_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> crate::error::Result<Vec<Arc<McpClient>>> {
        let config = McpConfigFile::from_file(path)?;
        let server_configs = config.to_server_configs()?;
        let mut clients = Vec::new();
        for server_config in server_configs {
            let name = server_config.name.clone();
            match self.connect_mcp_from_config(server_config).await {
                Ok(client) => clients.push(client),
                Err(e) => {
                    tracing::warn!(
                        agent = %self.config.agent_name,
                        server = %name,
                        error = %e,
                        "MCP server connection failed, skipping"
                    );
                }
            }
        }
        Ok(clients)
    }

    #[cfg(feature = "mcp")]
    pub fn mcp_client(&self, name: &str) -> Option<&Arc<McpClient>> {
        self.mcp_manager.get_client(name)
    }

    #[cfg(feature = "mcp")]
    pub fn list_mcp_servers(&self) -> Vec<&str> {
        self.mcp_manager.server_names()
    }

    #[cfg(feature = "mcp")]
    pub async fn disconnect_mcp(&mut self, name: &str) -> bool {
        self.mcp_manager.disconnect(name).await
    }

    // ── System Prompt ────────────────────────────────────────────────────────

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt.clone();
        self.context.update_system(prompt);
        tracing::info!(
            agent = %self.config.agent_name,
            "System prompt updated"
        );
    }
}
