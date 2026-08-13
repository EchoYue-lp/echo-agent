//! Direct execution entry points (run_direct / run_chat_direct)

use super::super::ReactAgent;
use crate::error::Result;
use tracing::{Instrument, debug, info, info_span};

impl ReactAgent {
    /// Direct execution (no planning): reset/restore context, then enter ReAct loop
    pub(crate) async fn run_direct(&self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        self.restore_thread_context().await?;

        info!(agent = %agent, "🧠 Agent starting task execution");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.tools.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "Execution details"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(task)
            .instrument(info_span!("agent_execute", agent.name = %agent, agent.model = %model))
            .await
    }

    /// Multi-turn conversation: do not reset context, append message then enter ReAct loop
    pub(crate) async fn run_chat_direct(&self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        self.restore_chat_context_if_cold().await?;

        info!(agent = %agent, "💬 Agent in multi-turn conversation");
        debug!(
            agent = %agent,
            message = %message,
            tools = ?self.tools.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "Conversation details"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(message)
            .instrument(info_span!("agent_chat", agent.name = %agent, agent.model = %model))
            .await
    }
}
