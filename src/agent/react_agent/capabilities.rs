//! ReactAgent 能力配置 API
//!
//! 包含所有"配置型"方法：
//! - 工具注册（`add_tool` / `add_tools` / `add_need_appeal_tool`）
//! - Skill 安装（`add_skill` / `add_skills` / `load_skills_from_dir`）
//! - MCP 连接（`connect_mcp` / `load_mcp_from_file`）
//! - SubAgent 注册、压缩器、回调等

use super::ReactAgent;
use crate::agent::Agent;
use crate::compression::{ContextCompressor, ForceCompressStats};
use crate::error::Result;
use crate::mcp::config_loader::McpServerEntry;
use crate::mcp::{McpClient, McpConfigFile, McpServerConfig};
use crate::skills::external::{LoadSkillResourceTool, SkillLoader};
use crate::skills::{Skill, SkillInfo};
use crate::tools::Tool;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};

impl ReactAgent {
    // ── 工具注册 ──────────────────────────────────────────────────────────────

    /// 注册单个工具。`enable_tool = false` 时自动开启工具能力。
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.config.enable_tool = true;
        self.tool_manager.register(tool);
    }

    /// 批量注册工具。`enable_tool = false` 时自动开启工具能力。
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

    /// 注册需要人工审批的工具：执行前会在控制台弹出 y/n 确认
    pub fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
        if !self.config.enable_human_in_loop {
            warn!(
                agent = %self.config.agent_name,
                tool = %tool.name(),
                "⚠️ human_in_loop 能力已禁用，工具将注册但不会进入人工审批"
            );
            self.add_tool(tool);
            return;
        }
        let tool_name = tool.name().to_string();
        self.add_tool(tool);
        self.human_in_loop
            .write()
            .map_err(|e| {
                warn!("human_in_loop lock poisoned: {}", e);
            })
            .map(|mut guard| guard.mark_need_approval(tool_name))
            .ok();
    }

    // ── 上下文压缩 ────────────────────────────────────────────────────────────

    /// 设置上下文压缩器。
    ///
    /// 配合 `AgentConfig::token_limit` 使用：token 超限时自动在 `think()` 前压缩消息历史。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::compression::compressor::{SlidingWindowCompressor, SummaryCompressor, DefaultSummaryPrompt};
    /// use echo_agent::llm::DefaultLlmClient;
    /// use reqwest::Client;
    /// use std::sync::Arc;
    ///
    /// # fn example(agent: &mut echo_agent::agent::react_agent::ReactAgent) {
    /// agent.set_compressor(SlidingWindowCompressor::new(20));
    ///
    /// let llm = Arc::new(DefaultLlmClient::new(Arc::new(Client::new()), "qwen3-max"));
    /// agent.set_compressor(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8));
    /// # }
    /// ```
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.context.set_compressor(compressor);
    }

    /// 返回当前上下文的（消息条数，估算 token 数）
    pub fn context_stats(&self) -> (usize, usize) {
        (self.context.messages().len(), self.context.token_estimate())
    }

    /// 使用指定压缩器强制压缩上下文（不影响已安装的默认压缩器）
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<ForceCompressStats> {
        self.context.force_compress_with(compressor).await
    }

    /// 返回所有已注册的工具名（含内置工具）
    pub fn list_tools(&self) -> Vec<&str> {
        self.tool_manager.list_tools()
    }

    // ── SubAgent ──────────────────────────────────────────────────────────────

    pub fn register_agent(&mut self, agent: Box<dyn Agent>) {
        if !self.config.enable_subagent {
            warn!(
                agent = %self.config.agent_name,
                subagent = %agent.name(),
                "⚠️ subagent 能力已禁用，忽略子 agent 注册"
            );
            return;
        }
        let name = agent.name().to_string();
        match self.subagents.write() {
            Ok(mut agents) => {
                agents.insert(name, Arc::new(AsyncMutex::new(agent)));
            }
            Err(e) => {
                warn!(
                    agent = %self.config.agent_name,
                    subagent = %name,
                    "⚠️ subagents lock poisoned，无法注册子 agent: {}",
                    e
                );
            }
        }
    }

    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    // ── 基础配置 ──────────────────────────────────────────────────────────────

    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    /// 运行时注册事件回调
    pub fn add_callback(&mut self, callback: Arc<dyn crate::agent::AgentCallback>) {
        self.config.callbacks.push(callback);
    }

    // ── Skill ─────────────────────────────────────────────────────────────────

    /// 扫描指定目录下的所有外部技能（SKILL.md），并将它们安装到 Agent
    ///
    /// # 整体流程
    ///
    /// ```text
    /// 1. 扫描 skills_dir/ 下的每个子目录
    /// 2. 解析 SKILL.md 的 YAML Frontmatter → SkillMeta
    /// 3. 将 meta.instructions 注入 system_prompt
    /// 4. 预加载 load_on_startup: true 的资源并追加到 system_prompt
    /// 5. 注册 LoadSkillResourceTool（LLM 按需调用懒加载其余资源）
    /// 6. 在 SkillManager 中记录元数据
    /// ```
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// agent.load_skills_from_dir("./skills").await?;
    /// ```
    pub async fn load_skills_from_dir(
        &mut self,
        skills_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Vec<String>> {
        let loader = Arc::new(tokio::sync::Mutex::new(SkillLoader::new(skills_dir)));

        let loaded = {
            let mut l = loader.lock().await;
            l.scan().await?
        };

        if loaded.is_empty() {
            tracing::warn!(
                agent = %self.config.agent_name,
                "外部技能目录扫描完毕，未找到任何有效 SKILL.md"
            );
            return Ok(vec![]);
        }

        let mut loaded_names = Vec::new();
        let mut has_resources = false;

        for skill in &loaded {
            let meta = &skill.meta;

            if self.skill_manager.is_installed(&meta.name) {
                tracing::warn!(
                    agent = %self.config.agent_name,
                    skill = %meta.name,
                    "Skill 已安装，跳过"
                );
                continue;
            }

            let prompt_block = meta.to_prompt_block();
            self.config.system_prompt.push_str(&prompt_block);

            {
                let l = loader.lock().await;
                for res_ref in meta.startup_resources() {
                    if l.is_cached(&meta.name, &res_ref.name) {
                        tracing::debug!(
                            "预加载资源 '{}/{}' 已就绪，可通过工具访问",
                            meta.name,
                            res_ref.name
                        );
                    }
                }
            }

            if meta.resources.as_ref().is_some_and(|r| !r.is_empty()) {
                has_resources = true;
            }

            let tool_names = if has_resources {
                vec!["load_skill_resource".to_string()]
            } else {
                vec![]
            };
            self.skill_manager.record(SkillInfo {
                name: meta.name.clone(),
                description: meta.description.clone(),
                tool_names,
                has_prompt_injection: true,
            });

            tracing::info!(
                agent = %self.config.agent_name,
                skill = %meta.name,
                version = %meta.version.as_deref().unwrap_or("?"),
                resources = meta.resources.as_ref().map_or(0, |r| r.len()),
                "🎯 外部 Skill 已加载"
            );

            loaded_names.push(meta.name.clone());
        }

        self.context
            .update_system(self.config.system_prompt.clone());

        if has_resources && self.tool_manager.get_tool("load_skill_resource").is_none() {
            let catalog_desc = {
                let l = loader.lock().await;
                l.resource_catalog()
                    .iter()
                    .map(|(sname, rref)| {
                        format!(
                            "  - {}/{}: {}",
                            sname,
                            rref.name,
                            rref.description.as_deref().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            let tool = LoadSkillResourceTool::new(loader).with_catalog_desc(catalog_desc);
            self.tool_manager.register(Box::new(tool));

            tracing::info!(
                agent = %self.config.agent_name,
                "已注册 load_skill_resource 工具"
            );
        }

        Ok(loaded_names)
    }

    /// 为 Agent 安装一个 Skill
    ///
    /// 安装过程：
    /// 1. 将 Skill 提供的所有工具注册到 ToolManager
    /// 2. 若 Skill 有 system_prompt_injection，追加到 system_prompt
    /// 3. 记录 Skill 元数据到 SkillManager
    ///
    /// # 示例
    /// ```rust
    /// agent.add_skill(Box::new(CalculatorSkill));
    /// agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/workspace")));
    /// ```
    pub fn add_skill(&mut self, skill: Box<dyn Skill>) {
        let name = skill.name().to_string();

        if self.skill_manager.is_installed(&name) {
            warn!(
                agent = %self.config.agent_name,
                skill = %name,
                "⚠️ Skill 已安装，跳过重复注册"
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

        self.skill_manager.record(SkillInfo {
            name: name.clone(),
            description: skill.description().to_string(),
            tool_names,
            has_prompt_injection: has_injection,
        });

        info!(
            agent = %self.config.agent_name,
            skill = %name,
            description = %skill.description(),
            "🎯 Skill 已安装"
        );
    }

    /// 批量安装多个 Skill
    pub fn add_skills(&mut self, skills: Vec<Box<dyn Skill>>) {
        for skill in skills {
            self.add_skill(skill);
        }
    }

    /// 列出所有已安装的 Skill 元数据
    pub fn list_skills(&self) -> Vec<&SkillInfo> {
        self.skill_manager.list()
    }

    /// 查询某个 Skill 是否已安装
    pub fn has_skill(&self, name: &str) -> bool {
        self.skill_manager.is_installed(name)
    }

    /// 已安装的 Skill 数量
    pub fn skill_count(&self) -> usize {
        self.skill_manager.count()
    }

    // ── MCP 连接管理 ──────────────────────────────────────────────────────────

    /// 从外部注入 MCP 工具（应用层管理 MCP 生命周期）
    ///
    /// 适用场景：
    /// - 多 Agent 共享同一组 MCP 连接
    /// - 需要在 Agent 生命周期外管理 MCP 连接
    /// - 测试时注入 Mock MCP 工具
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # async fn example() -> echo_agent::error::Result<()> {
    /// use echo_agent::mcp::McpManager;
    /// use echo_agent::prelude::*;
    ///
    /// // 应用层管理 MCP 连接
    /// let mut mcp_manager = McpManager::new();
    /// let tools = mcp_manager.connect(
    ///     McpServerConfig::stdio("fs", "npx", vec![
    ///         "-y", "@modelcontextprotocol/server-filesystem", "/tmp"
    ///     ])
    /// ).await?;
    ///
    /// // 将工具注入到多个 Agent
    /// let mut agent1 = ReactAgent::new(AgentConfig::standard("qwen3-max", "a1", "助手"));
    /// let mut agent2 = ReactAgent::new(AgentConfig::standard("qwen3-max", "a2", "助手"));
    ///
    /// agent1.register_mcp_tools(tools.clone());
    /// agent2.register_mcp_tools(tools);
    ///
    /// // MCP 连接由应用层管理，可在 Agent 生命周期外关闭
    /// // mcp_manager.close_all().await;
    /// # Ok(())
    /// # }
    /// ```
    pub fn register_mcp_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.add_tools(tools);
    }

    /// 从 McpServerConfig 连接单个 MCP 服务端，并将其工具自动注册到 Agent。
    ///
    /// - 将连接生命周期绑定到 Agent（Agent 释放时自动关闭）
    /// - 返回 `Arc<McpClient>`，可进一步访问资源、提示词等
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # async fn example() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::react_agent::ReactAgent;
    /// use echo_agent::agent::config::AgentConfig;
    /// use echo_agent::mcp::McpServerConfig;
    /// use echo_agent::mcp::server_config::TransportConfig;
    ///
    /// let mut agent = ReactAgent::new(AgentConfig::default());
    /// let client = agent.connect_mcp_from_config(McpServerConfig {
    ///     name: "my-server".to_string(),
    ///     transport: TransportConfig::Stdio {
    ///         command: "java".to_string(),
    ///         args: vec!["-jar".to_string(), "server.jar".to_string(), "stdio".to_string()],
    ///         env: vec![],
    ///     },
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
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
            "🔌 MCP 服务端已连接"
        );
        Ok(client.clone())
    }

    /// 从 json MCP 配置连接单个 MCP 服务端，并将其工具自动注册到 Agent。
    ///
    /// - 将连接生命周期绑定到 Agent（Agent 释放时自动关闭）
    /// - 返回 `Arc<McpClient>`，可进一步访问资源、提示词等
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # async fn example() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::react_agent::ReactAgent;
    /// use echo_agent::agent::config::AgentConfig;
    /// use echo_agent::mcp::McpServerConfig;
    /// use echo_agent::mcp::server_config::TransportConfig;
    ///
    /// let name = "json_str";
    ///
    ///  let config = r#"{
    ///       "command": "npx",
    ///       "args": ["-y", "@modelcontextprotocol/server-github"],
    ///       "env": {
    ///         "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_your_token_here"
    ///       },
    ///       "disabled": true
    ///     }"#;
    ///
    /// let mut agent = ReactAgent::new(AgentConfig::default());
    /// let client = agent.connect_mcp_from_json(name,config).await?;
    /// #Ok(())
    /// # }
    /// ```
    pub async fn connect_mcp_from_json(
        &mut self,
        name: &str,
        json_config_str: &str,
    ) -> crate::error::Result<Arc<McpClient>> {
        let entry = serde_json::from_str::<McpServerEntry>(json_config_str)?;
        let config = entry.to_server_config(name)?;
        self.connect_mcp_from_config(config).await
    }

    /// 从 `mcp.json` / `mcp.yaml` 配置文件批量连接所有 MCP 服务端，并注册工具。
    ///
    /// 连接失败的服务端打印警告并跳过，不影响其他服务端。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # async fn example() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::react_agent::ReactAgent;
    /// use echo_agent::agent::config::AgentConfig;
    ///
    /// let mut agent = ReactAgent::new(AgentConfig::default());
    /// let clients = agent.load_mcp_from_file("examples/mcp1.json").await?;
    /// println!("已连接 {} 个 MCP 服务端", clients.len());
    /// # Ok(())
    /// # }
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
                        "⚠️ MCP 服务端连接失败，已跳过"
                    );
                }
            }
        }
        Ok(clients)
    }

    /// 获取已连接的指定 MCP 服务端客户端
    pub fn mcp_client(&self, name: &str) -> Option<&Arc<McpClient>> {
        self.mcp_manager.get_client(name)
    }

    /// 列出所有已连接的 MCP 服务端名称
    pub fn list_mcp_servers(&self) -> Vec<&str> {
        self.mcp_manager.server_names()
    }
}
