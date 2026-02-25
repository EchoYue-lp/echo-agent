/// Agent 角色：区分编排者和执行者
#[derive(Debug, Clone, PartialEq)]
pub enum AgentRole {
    /// 编排者：负责任务规划、分配和协调子 agent，不持有具体业务工具
    Orchestrator,
    /// 执行者：专注于具体任务执行，只携带业务工具，不持有任务管理/子 agent 调度能力
    Worker,
}

impl Default for AgentRole {
    fn default() -> Self {
        AgentRole::Worker
    }
}

pub struct AgentConfig {
    /// 模型名称
    pub(crate) model_name: String,
    /// 系统提示词
    pub(crate) system_prompt: String,
    /// 是否启用详细日志
    verbose: bool,
    /// agent 名称
    pub(crate) agent_name: String,
    /// 最大迭代次数
    pub(crate) max_iterations: usize,
    /// 可使用的工具（为空表示不限制）
    pub(crate) allowed_tools: Vec<String>,
    /// agent 角色
    pub(crate) role: AgentRole,
    /// 是否允许注册并调用业务工具（如数学、天气等）
    pub(crate) enable_tool: bool,
    /// 是否启用任务能力（plan/create_task/update_task）
    pub(crate) enable_task: bool,
    /// 是否启用 human-in-loop 工具
    pub(crate) enable_human_in_loop: bool,
    /// 是否启用 subagent 调度能力（agent_tool）
    pub(crate) enable_subagent: bool,
}

impl AgentConfig {
    pub fn new(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
            system_prompt: system_prompt.to_string(),
            verbose: false,
            agent_name: agent_name.to_string(),
            max_iterations: 10,
            allowed_tools: Vec::new(),
            role: AgentRole::default(),
            enable_tool: false,
            enable_task: false,
            enable_human_in_loop: false,
            enable_subagent: false,
        }
    }

    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    pub fn enable_tool(mut self, enabled: bool) -> Self {
        self.enable_tool = enabled;
        self
    }

    pub fn enable_task(mut self, enabled: bool) -> Self {
        self.enable_task = enabled;
        self
    }

    pub fn enable_human_in_loop(mut self, enabled: bool) -> Self {
        self.enable_human_in_loop = enabled;
        self
    }

    pub fn enable_subagent(mut self, enabled: bool) -> Self {
        self.enable_subagent = enabled;
        self
    }

    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools.extend(tools);
        self
    }

    pub fn get_allowed_tools(&self) -> &Vec<String> {
        &self.allowed_tools
    }

    pub fn is_tool_enabled(&self) -> bool {
        self.enable_tool
    }

    pub fn is_task_enabled(&self) -> bool {
        self.enable_task
    }

    pub fn is_human_in_loop_enabled(&self) -> bool {
        self.enable_human_in_loop
    }

    pub fn is_subagent_enabled(&self) -> bool {
        self.enable_subagent
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    pub fn agent_name(mut self, agent_name: &str) -> Self {
        self.agent_name = agent_name.to_string();
        self
    }

    pub fn model_name(mut self, model_name: &str) -> Self {
        self.model_name = model_name.to_string();
        self
    }

    pub fn system_prompt(mut self, system_prompt: &str) -> Self {
        self.system_prompt = system_prompt.to_string();
        self
    }
}
