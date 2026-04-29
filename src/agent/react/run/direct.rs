//! 直接执行入口（run_direct / run_chat_direct）

use super::super::ReactAgent;
use crate::error::Result;
use tracing::{Instrument, debug, info, info_span};

impl ReactAgent {
    /// 直接执行（无规划）：重置/恢复上下文，然后进入 ReAct 循环
    pub(crate) async fn run_direct(&self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        self.restore_thread_context().await;

        info!(agent = %agent, "🧠 Agent 开始执行任务");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.tools.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "执行详情"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(task)
            .instrument(info_span!("agent_execute", agent.name = %agent, agent.model = %model))
            .await
    }

    /// 多轮对话：不重置上下文，直接追加消息后进入 ReAct 循环
    pub(crate) async fn run_chat_direct(&self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        info!(agent = %agent, "💬 Agent 多轮对话中");
        debug!(
            agent = %agent,
            message = %message,
            tools = ?self.tools.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "对话详情"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(message)
            .instrument(info_span!("agent_chat", agent.name = %agent, agent.model = %model))
            .await
    }
}
