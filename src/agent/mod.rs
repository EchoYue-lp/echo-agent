use crate::error::Result;
use crate::tools::Tool;
use async_trait::async_trait;

pub mod react_agent;

/// 一个 agent 应该有：系统提示词、可调用的工具
#[async_trait]
pub trait Agent: Send + Sync {
    /// agent 的名称
    fn name(&self) -> &str;

    /// 模型名称
    fn model_name(&self) -> &str;

    /// 设置模型
    fn set_model(&mut self, model_name: &str);

    /// 系统提示词
    fn system_prompt(&self) -> &str;

    /// 添加工具
    fn add_tool(&mut self, tool: Box<dyn Tool>);
    
    /// 添加需要人工审批的 tool
    fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>);

    /// 可调用的工具
    fn list_tools(&self) -> Vec<&str>;

    /// 添加subagent
    fn register_agent(&mut self, agent: Box<dyn Agent>);

    /// 列出所有的子agent
    fn list_subagent(&self) -> Vec<&str>;

    /// 核心执行方法
    async fn execute(&mut self, task: &str) -> Result<String>;
}
