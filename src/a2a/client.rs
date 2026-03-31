//! A2A HTTP 客户端
//!
//! 用于发现和调用远程 A2A 兼容 Agent。

use super::types::*;
use crate::error::{ReactError, Result};
use reqwest::Client;
use tracing::{debug, info};

/// A2A 客户端 — 发现和调用远程 Agent
pub struct A2AClient {
    client: Client,
}

impl A2AClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// 发现远程 Agent：获取 Agent Card
    ///
    /// 从 `{base_url}/.well-known/agent.json` 获取 Agent Card。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::A2AClient;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let card = client.discover("http://localhost:8080").await?;
    /// println!("发现 Agent: {} - {:?}", card.name, card.description);
    /// for skill in &card.skills {
    ///     println!("  技能: {} - {:?}", skill.name, skill.description);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn discover(&self, base_url: &str) -> Result<AgentCard> {
        let url = format!(
            "{}/.well-known/agent.json",
            base_url.trim_end_matches('/')
        );

        info!(url = %url, "🔍 A2A: 发现远程 Agent");

        let response = self.client.get(&url).send().await.map_err(|e| {
            ReactError::Other(format!("A2A 发现请求失败: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(ReactError::Other(format!(
                "A2A 发现失败: HTTP {}",
                response.status()
            )));
        }

        let card: AgentCard = response.json().await.map_err(|e| {
            ReactError::Other(format!("A2A Agent Card 解析失败: {}", e))
        })?;

        info!(
            agent = %card.name,
            skills = card.skills.len(),
            "✅ A2A: 发现 Agent '{}' ({}个技能)",
            card.name,
            card.skills.len()
        );

        Ok(card)
    }

    /// 向远程 Agent 发送任务
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::A2AClient;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let result = client.send_task("http://localhost:8080", "请翻译'你好'为英文").await?;
    /// if let Some(task) = result {
    ///     println!("任务状态: {}", task.status.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_task(
        &self,
        agent_url: &str,
        message: &str,
    ) -> Result<Option<A2ATask>> {
        self.send_task_with_session(agent_url, message, None).await
    }

    /// 向远程 Agent 发送任务（带会话 ID）
    pub async fn send_task_with_session(
        &self,
        agent_url: &str,
        message: &str,
        session_id: Option<String>,
    ) -> Result<Option<A2ATask>> {
        let request = A2ATaskRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "tasks/send".to_string(),
            params: A2ATaskParams {
                id: None,
                session_id,
                message: A2AMessage::user_text(message),
            },
        };

        info!(
            url = %agent_url,
            message_len = message.len(),
            "📨 A2A: 发送任务"
        );

        let response = self
            .client
            .post(agent_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 任务发送失败: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 响应解析失败: {}", e)))?;

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A 远程错误 [{}]: {}",
                error.code, error.message
            )));
        }

        debug!(
            task_id = ?task_response.result.as_ref().map(|t| &t.id),
            "✅ A2A: 任务发送完成"
        );

        Ok(task_response.result)
    }

    /// 查询远程任务状态
    pub async fn get_task(
        &self,
        agent_url: &str,
        task_id: &str,
    ) -> Result<Option<A2ATask>> {
        let request = A2ATaskRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "tasks/get".to_string(),
            params: A2ATaskParams {
                id: Some(task_id.to_string()),
                session_id: None,
                message: A2AMessage::user_text(""),
            },
        };

        let response = self
            .client
            .post(agent_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 任务查询失败: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 响应解析失败: {}", e)))?;

        Ok(task_response.result)
    }

    /// 取消远程任务
    pub async fn cancel_task(
        &self,
        agent_url: &str,
        task_id: &str,
    ) -> Result<Option<A2ATask>> {
        let request = A2ATaskRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "tasks/cancel".to_string(),
            params: A2ATaskParams {
                id: Some(task_id.to_string()),
                session_id: None,
                message: A2AMessage::user_text(""),
            },
        };

        let response = self
            .client
            .post(agent_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 任务取消失败: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 响应解析失败: {}", e)))?;

        Ok(task_response.result)
    }
}

impl Default for A2AClient {
    fn default() -> Self {
        Self::new()
    }
}
