//! A2A HTTP client
//!
//! For discovering and invoking remote A2A-compatible Agents.
//!
//! Supports both sync and streaming task modes:
//! - [`send_task`](A2AClient::send_task) — Synchronously wait for task completion
//! - [`send_task_streaming`](A2AClient::send_task_streaming) — Receive real-time events via SSE streaming

use super::types::*;
use crate::error::{ReactError, Result};
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;
use tracing::{debug, info, warn};

/// A2A client — discover and invoke remote Agents
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

    /// Discover a remote Agent: fetch Agent Card
    ///
    /// Fetches the Agent Card from `{base_url}/.well-known/agent.json`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::A2AClient;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let card = client.discover("http://localhost:8080").await?;
    /// println!("Discovered agent: {} - {:?}", card.name, card.description);
    /// for skill in &card.skills {
    ///     println!("  Skill: {} - {:?}", skill.name, skill.description);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn discover(&self, base_url: &str) -> Result<AgentCard> {
        let url = format!("{}/.well-known/agent.json", base_url.trim_end_matches('/'));

        info!(url = %url, "A2A: discovering remote Agent");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A discovery request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ReactError::Other(format!(
                "A2A discovery failed: HTTP {}: {}",
                status, body
            )));
        }

        let card: AgentCard = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A Agent Card parse failed: {}", e)))?;

        info!(
            agent = %card.name,
            skills = card.skills.len(),
            "A2A: discovered Agent '{}' ({} skills)",
            card.name,
            card.skills.len()
        );

        Ok(card)
    }

    /// Send a task to a remote Agent (wait synchronously for completion)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::A2AClient;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let result = client.send_task("http://localhost:8080", "Please translate 'hello' to Chinese").await?;
    /// if let Some(task) = result {
    ///     println!("Task status: {}", task.status.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_task(&self, agent_url: &str, message: &str) -> Result<Option<A2ATask>> {
        self.send_task_with_session(agent_url, message, None).await
    }

    /// Send a task to a remote Agent (with session ID)
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
            "A2A: sending task"
        );

        let response = self
            .client
            .post(agent_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A send task failed: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A response parse failed: {}", e)))?;

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A remote error [{}]: {}",
                error.code, error.message
            )));
        }

        debug!(
            task_id = ?task_response.result.as_ref().map(|t| &t.id),
            "A2A: task send completed"
        );

        Ok(task_response.result)
    }

    /// Send a task to a remote Agent with SSE streaming
    ///
    /// Sends a `tasks/sendSubscribe` request and parses the SSE event stream returned by the server.
    /// Returns an async stream of `A2AStreamEvent` that the caller can process event by event.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::a2a::{A2AClient, A2AStreamEvent, TaskState};
    /// use futures::StreamExt;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let client = A2AClient::new();
    /// let mut stream = client
    ///     .send_task_streaming("http://localhost:8080", "Translate 'hello'")
    ///     .await?;
    ///
    /// while let Some(event) = stream.next().await {
    ///     match event {
    ///         A2AStreamEvent::StatusUpdate(e) => {
    ///             println!("Status: {}", e.status.state);
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

    /// Send a task to a remote Agent with SSE streaming (with session ID)
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

        info!(url = %agent_url, "A2A: sending streaming task");

        let response = self
            .client
            .post(agent_url)
            .header("Accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("A2A streaming request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ReactError::Other(format!(
                "A2A streaming request failed: HTTP {}: {}",
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
                            warn!("A2A SSE: UTF-8 decode failed: {}", e);
                            continue;
                        }
                    },
                    Err(e) => {
                        warn!("A2A SSE: read chunk failed: {}", e);
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

    /// Query remote task status
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
            .map_err(|e| ReactError::Other(format!("A2A task query failed: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A response parse failed: {}", e)))?;

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A error {}: {}",
                error.code, error.message
            )));
        }

        Ok(task_response.result)
    }

    /// Cancel a remote task
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
            .map_err(|e| ReactError::Other(format!("A2A task cancel failed: {}", e)))?;

        let task_response: A2ATaskResponse = response
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("A2A response parse failed: {}", e)))?;

        if let Some(error) = task_response.error {
            return Err(ReactError::Other(format!(
                "A2A error {}: {}",
                error.code, error.message
            )));
        }

        Ok(task_response.result)
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn parse_sse_data(data: &str) -> Option<A2AStreamEvent> {
        // SSE data may be an A2AStreamResponse (with jsonrpc wrapper) or directly an A2AStreamEvent
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
