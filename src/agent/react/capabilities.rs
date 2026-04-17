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

    /// 设置上下文压缩器
    ///
    /// # 参数
    /// * `compressor` - 实现了 `ContextCompressor` trait 的压缩器实例
    ///
    /// # 说明
    /// 上下文压缩器用于在 token 数量超过限制时自动压缩对话历史，
    /// 移除不重要的消息以减少 token 消耗。
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.context.set_compressor(compressor);
    }

    /// 获取上下文统计信息
    ///
    /// # 返回值
    /// 返回元组 `(消息数量, 估计的 token 数量)`
    pub fn context_stats(&self) -> (usize, usize) {
        (self.context.messages().len(), self.context.token_estimate())
    }

    /// 使用指定压缩器强制压缩上下文
    ///
    /// # 参数
    /// * `compressor` - 用于压缩的压缩器实例
    ///
    /// # 返回值
    /// 返回压缩统计信息，包含压缩前后的 token 数量等
    ///
    /// # 说明
    /// 该方法会绕过自动压缩阈值，立即使用指定的压缩器压缩上下文，
    /// 通常用于手动控制压缩时机或测试压缩效果。
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<ForceCompressStats> {
        self.context.force_compress_with(compressor).await
    }

    /// 列出所有已注册的工具名称
    ///
    /// # 返回值
    /// 已注册工具的名称列表
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

    /// 批量注册子代理
    ///
    /// # 参数
    /// * `agents` - 子代理实例列表
    ///
    /// # 说明
    /// 每个子代理会自动包装为默认的 Sync 模式 `SubagentDefinition`。
    /// 如需更精细的控制，请使用 `register_subagent_with_definition()` 方法。
    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    // ── Basic config ─────────────────────────────────────────────────────────

    /// 运行时设置 LLM 模型名称
    ///
    /// # 参数
    /// * `model_name` - 新的 LLM 模型名称
    ///
    /// # 说明
    /// 该方法允许在运行时动态切换 Agent 使用的 LLM 模型。
    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    /// 添加 Agent 回调
    ///
    /// # 参数
    /// * `callback` - 实现了 `AgentCallback` trait 的回调实例
    ///
    /// # 说明
    /// 回调会在 Agent 执行过程中触发不同事件时被调用，用于监控、日志记录等。
    /// 与配置构建器的 `with_callback` 不同，此方法可在运行时动态添加回调。
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

    /// 注册 MCP（Model Context Protocol）工具
    ///
    /// # 参数
    /// * `tools` - MCP 工具实例列表
    ///
    /// # 说明
    /// 将 MCP 工具注册到 Agent 的工具管理器中，使其可用于工具调用。
    pub fn register_mcp_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.add_tools(tools);
    }

    #[cfg(feature = "mcp")]
    /// 根据 MCP 服务器配置连接 MCP 服务器
    ///
    /// # 参数
    /// * `config` - MCP 服务器配置
    ///
    /// # 返回值
    /// 返回连接成功后创建的 `McpClient` 实例
    ///
    /// # 说明
    /// 该方法会根据配置连接到 MCP 服务器，获取服务器提供的工具，
    /// 并将其注册到 Agent 的工具管理器中。
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
    /// 从 JSON 字符串配置连接 MCP 服务器
    ///
    /// # 参数
    /// * `name` - MCP 服务器名称
    /// * `json_config_str` - JSON 格式的 MCP 服务器配置字符串
    ///
    /// # 返回值
    /// 返回连接成功后创建的 `McpClient` 实例
    ///
    /// # 说明
    /// 该方法解析 JSON 格式的配置字符串，转换为 MCP 服务器配置，
    /// 然后调用 `connect_mcp_from_config` 进行连接。
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
    /// 从配置文件加载并连接多个 MCP 服务器
    ///
    /// # 参数
    /// * `path` - MCP 配置文件路径（支持 `.json` 或 `.yaml` 格式）
    ///
    /// # 返回值
    /// 返回成功连接的 `McpClient` 实例列表
    ///
    /// # 说明
    /// 1. 解析 MCP 配置文件（支持 JSON 或 YAML 格式）
    /// 2. 为每个服务器配置调用 `connect_mcp_from_config`
    /// 3. 收集所有成功连接的客户端
    /// 4. 连接失败的服务器会被跳过并记录警告日志
    ///
    /// # 示例
    /// ```rust
    /// let mut agent = ReactAgent::new(...);
    /// let clients = agent.load_mcp_from_file("mcp_servers.json").await?;
    /// ```
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
    /// 获取已连接的 MCP 客户端
    ///
    /// # 参数
    /// * `name` - MCP 服务器名称
    ///
    /// # 返回值
    /// 返回对应名称的 MCP 客户端引用，如果未找到则返回 `None`
    ///
    /// # 说明
    /// 该方法用于获取已连接的 MCP 客户端，以便直接调用客户端的方法。
    /// 客户端通过 `connect_mcp_from_config` 或 `load_mcp_from_file` 连接。
    pub fn mcp_client(&self, name: &str) -> Option<&Arc<McpClient>> {
        self.mcp_manager.get_client(name)
    }

    #[cfg(feature = "mcp")]
    /// 列出所有已连接的 MCP 服务器名称
    ///
    /// # 返回值
    /// 已连接的 MCP 服务器名称列表
    ///
    /// # 说明
    /// 该方法返回当前所有已成功连接的 MCP 服务器的名称，
    /// 可用于展示连接状态或供用户选择特定的服务器。
    pub fn list_mcp_servers(&self) -> Vec<&str> {
        self.mcp_manager.server_names()
    }

    #[cfg(feature = "mcp")]
    /// 断开与指定 MCP 服务器的连接
    ///
    /// # 参数
    /// * `name` - 要断开的 MCP 服务器名称
    ///
    /// # 返回值
    /// `true` 表示成功断开连接，`false` 表示未找到该名称的服务器
    ///
    /// # 说明
    /// 该方法会断开与指定 MCP 服务器的连接，并移除相关的工具注册。
    /// 断开连接后，该服务器提供的工具将不再可用。
    pub async fn disconnect_mcp(&mut self, name: &str) -> bool {
        self.mcp_manager.disconnect(name).await
    }

    // ── System Prompt ────────────────────────────────────────────────────────

    /// 设置 Agent 的系统提示词
    ///
    /// # 参数
    /// * `prompt` - 新的系统提示词
    ///
    /// # 说明
    /// 该方法会更新 Agent 的系统提示词，并同步更新上下文管理器中的系统提示。
    /// 系统提示词定义了 Agent 的角色、行为规范和任务指令。
    ///
    /// # 注意
    /// 设置系统提示词会覆盖之前的所有系统提示内容。
    /// 如果之前通过技能注入添加了提示词，也会被覆盖。
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt.clone();
        self.context.update_system(prompt);
        tracing::info!(
            agent = %self.config.agent_name,
            "System prompt updated"
        );
    }
}
