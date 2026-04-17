//! A2A HTTP 客户端
//!
//! 用于发现和调用远程 A2A 兼容 Agent。
//!
//! 支持同步和流式两种任务模式：
//! - [`send_task`](A2AClient::send_task) — 同步等待任务完成
//! - [`send_task_streaming`](A2AClient::send_task_streaming) — SSE 流式接收实时事件

use super::types::*;
use crate::error::{ReactError, Result};
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;
use tracing::{debug, info, warn};

/// A2A 客户端 — 发现和调用远程 Agent
pub struct A2AClient {
    client: Client,
}

impl A2AClient {
    /// Create a new A2A client instance.
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
        let url = format!("{}/.well-known/agent.json", base_url.trim_end_matches('/'));

        info!(url = %url, "A2A: 发现远程 Agent");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 发现请求失败: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ReactError::Other(format!(
                "A2A 发现失败: HTTP {}: {}",
                status, body
            )));
        }

        let card: AgentCard = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A Agent Card 解析失败: {}", e)))?;

        info!(
            agent = %card.name,
            skills = card.skills.len(),
            "A2A: 发现 Agent '{}' ({}个技能)",
            card.name,
            card.skills.len()
        );

        Ok(card)
    }

    /// 向远程 Agent 发送任务（同步等待完成）
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
    pub async fn send_task(&self, agent_url: &str, message: &str) -> Result<Option<A2ATask>> {
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
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: METHOD_SEND.to_string(),
            params: A2ATaskParams {
                id: None,
                session_id,
                message: A2AMessage::user_text(message),
            },
        };

        info!(
            url = %agent_url,
            message_len = message.len(),
            "A2A: 发送任务"
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
            "A2A: 任务发送完成"
        );

        Ok(task_response.result)
    }

    /// 向远程 Agent 流式发送任务（SSE）
    ///
    /// 发送 `tasks/sendSubscribe` 请求，解析服务端返回的 SSE 事件流。
    /// 返回 `A2AStreamEvent` 的异步 Stream，调用方可逐事件处理。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::{A2AClient, A2AStreamEvent, TaskState};
    /// use futures::StreamExt;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let mut stream = client
    ///     .send_task_streaming("http://localhost:8080", "翻译'你好'")
    ///     .await?;
    ///
    /// while let Some(event) = stream.next().await {
    ///     match event {
    ///         A2AStreamEvent::StatusUpdate(e) => {
    ///             println!("状态: {}", e.status.state);
    ///             if e.is_final { break; }
    ///         }
    ///         A2AStreamEvent::ArtifactUpdate(e) => {
    ///             for part in &e.artifact.parts {
    ///                 if let echo_agent::a2a::A2APart::Text { text } = part {
    ///                     print!("{}", text);
    ///                 }
    ///             }
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_task_streaming(
        &self,
        agent_url: &str,
        message: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = A2AStreamEvent> + Send>>> {
        self.send_task_streaming_with_session(agent_url, message, None)
            .await
    }

    /// 向远程 Agent 流式发送任务（带会话 ID）
    pub async fn send_task_streaming_with_session(
        &self,
        agent_url: &str,
        message: &str,
        session_id: Option<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = A2AStreamEvent> + Send>>> {
        let request = A2ATaskRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: METHOD_SEND_SUBSCRIBE.to_string(),
            params: A2ATaskParams {
                id: None,
                session_id,
                message: A2AMessage::user_text(message),
            },
        };

        info!(url = %agent_url, "A2A: 发送流式任务");

        let response = self
            .client
            .post(agent_url)
            .header("Accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A 流式请求失败: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ReactError::Other(format!(
                "A2A 流式请求失败: HTTP {}: {}",
                status, body
            )));
        }

        let byte_stream = response.bytes_stream();

        let event_stream = async_stream::stream! {
            use futures::StreamExt;

            let mut buffer = String::new();
            let mut byte_stream = Box::pin(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("A2A SSE: UTF-8 解码失败: {}", e);
                            continue;
                        }
                    },
                    Err(e) => {
                        warn!("A2A SSE: 读取块失败: {}", e);
                        break;
                    }
                };

                buffer.push_str(&chunk);

                while let Some(boundary) = buffer.find("\n\n") {
                    let event_block = buffer[..boundary].to_string();
                    buffer = buffer[boundary + 2..].to_string();

                    for line in event_block.lines() {
                        let line = line.trim();
                        if let Some(data) = line.strip_prefix("data:").map(|s| s.trim_start())
                            && let Some(event) = Self::parse_sse_data(data)
                        {
                            yield event;
                        }
                    }
                }
            }

            // flush remaining
            if !buffer.is_empty() {
                for line in buffer.lines() {
                    let line = line.trim();
                    if let Some(data) = line.strip_prefix("data:").map(|s| s.trim_start())
                        && let Some(event) = Self::parse_sse_data(data) {
                            yield event;
                        }
                }
            }
        };

        Ok(Box::pin(event_stream))
    }

    /// 查询远程任务状态
    pub async fn get_task(&self, agent_url: &str, task_id: &str) -> Result<Option<A2ATask>> {
        let request = A2ATaskRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: METHOD_GET.to_string(),
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

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A error {}: {}",
                error.code, error.message
            )));
        }

        Ok(task_response.result)
    }

    /// 取消远程任务
    pub async fn cancel_task(&self, agent_url: &str, task_id: &str) -> Result<Option<A2ATask>> {
        let request = A2ATaskRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: METHOD_CANCEL.to_string(),
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

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A error {}: {}",
                error.code, error.message
            )));
        }

        Ok(task_response.result)
    }

    // ── 内部辅助 ─────────────────────────────────────────────────────────────

    fn parse_sse_data(data: &str) -> Option<A2AStreamEvent> {
        // SSE data 可能是 A2AStreamResponse（带 jsonrpc 包装）或直接是 A2AStreamEvent
        if let Ok(stream_resp) = serde_json::from_str::<A2AStreamResponse>(data) {
            return stream_resp.result;
        }
        serde_json::from_str::<A2AStreamEvent>(data).ok()
    }
}

impl Default for A2AClient {
    fn default() -> Self {
        Self::new()
    }
}
