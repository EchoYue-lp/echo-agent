//! A2A HTTP 服务端
//!
//! 提供符合 A2A 协议的 HTTP 端点：
//! - `GET /.well-known/agent.json` — 返回 Agent Card
//! - `POST /` — 处理 JSON-RPC 任务请求
//!
//! 支持的 JSON-RPC 方法：
//! - `tasks/send`          — 同步发送任务并等待完成
//! - `tasks/sendSubscribe` — 流式发送任务，通过 SSE 接收实时事件
//! - `tasks/get`           — 查询任务状态
//! - `tasks/cancel`        — 取消任务

use super::types::*;
use crate::agent::Agent;
use crate::error::Result;
use echo_core::agent::AgentEvent;
use echo_core::agent::CancellationToken;
use futures::StreamExt;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

/// A2A 服务端
///
/// 将一个 Agent 暴露为 A2A 协议兼容的 HTTP 服务。
///
/// # 状态机
///
/// ```text
/// submitted → working → [input-required] → completed / failed
///                      ↑___________________↓
/// ```
pub struct A2AServer {
    card: AgentCard,
    agent: Arc<Mutex<Box<dyn Agent>>>,
    tasks: Arc<RwLock<HashMap<String, A2ATask>>>,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
}

impl A2AServer {
    /// Create a new A2A server instance.
    pub fn new(card: AgentCard, agent: impl Agent + 'static) -> Self {
        Self {
            card,
            agent: Arc::new(Mutex::new(Box::new(agent))),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create an A2A server instance from a boxed agent.
    pub fn from_boxed(card: AgentCard, agent: Box<dyn Agent>) -> Self {
        Self {
            card,
            agent: Arc::new(Mutex::new(agent)),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取 Agent Card（用于 `/.well-known/agent.json`）
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// 返回 Agent Card 的 JSON 字符串
    pub fn agent_card_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.card)
            .map_err(|e| crate::error::ReactError::Other(format!("Agent Card 序列化失败: {}", e)))
    }

    // ── 请求分发 ─────────────────────────────────────────────────────────────

    /// 处理 A2A JSON-RPC 请求（同步方法）
    ///
    /// 支持 `tasks/send`、`tasks/get`、`tasks/cancel`。
    /// 对于 `tasks/sendSubscribe`，请使用 [`handle_request_stream`]。
    pub async fn handle_request(&self, request_json: &str) -> String {
        let request: A2ATaskRequest = match serde_json::from_str(request_json) {
            Ok(req) => req,
            Err(e) => {
                return serde_json::to_string(&A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: None,
                    result: None,
                    error: Some(A2AError {
                        code: ERROR_CODE_PARSE as i32,
                        message: format!("Parse error: {}", e),
                    }),
                })
                .unwrap_or_default();
            }
        };

        let response = match request.method.as_str() {
            METHOD_SEND => self.handle_task_send(&request).await,
            METHOD_GET => self.handle_task_get(&request).await,
            METHOD_CANCEL => self.handle_task_cancel(&request).await,
            METHOD_SEND_SUBSCRIBE => A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id),
                result: None,
                error: Some(A2AError {
                    code: ERROR_CODE_METHOD_NOT_FOUND as i32,
                    message:
                        "tasks/sendSubscribe requires SSE transport; use handle_request_stream()"
                            .to_string(),
                }),
            },
            _ => A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id),
                result: None,
                error: Some(A2AError {
                    code: ERROR_CODE_METHOD_NOT_FOUND as i32,
                    message: format!("Method not found: {}", request.method),
                }),
            },
        };

        serde_json::to_string(&response).unwrap_or_default()
    }

    /// 处理流式请求 `tasks/sendSubscribe`
    ///
    /// 返回 SSE 事件流，每个事件序列化为一行 JSON（`data: <json>\n\n`）。
    /// 调用方负责将流写入 HTTP 响应体（Content-Type: text/event-stream）。
    ///
    /// # 事件序列示例
    ///
    /// ```text
    /// data: {"type":"status","taskId":"...","status":{"state":"submitted"},"final":false}
    /// data: {"type":"status","taskId":"...","status":{"state":"working"},"final":false}
    /// data: {"type":"artifact","taskId":"...","artifact":{"index":0,"parts":[...],"append":true},"final":false}
    /// data: {"type":"status","taskId":"...","status":{"state":"completed",...},"final":true}
    /// ```
    pub async fn handle_request_stream(
        &self,
        request_json: &str,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = A2AStreamEvent> + Send + '_>>> {
        let request: A2ATaskRequest = serde_json::from_str(request_json)
            .map_err(|e| crate::error::ReactError::Other(format!("Parse error: {}", e)))?;

        if request.method != METHOD_SEND_SUBSCRIBE {
            return Err(crate::error::ReactError::Other(format!(
                "handle_request_stream only supports tasks/sendSubscribe, got '{}'",
                request.method
            )));
        }

        let task_id = request
            .params
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let session_id = request.params.session_id.clone();
        let input_text = request.params.message.text_content();
        let user_message = request.params.message.clone();

        info!(task_id = %task_id, "A2A Stream: 收到流式任务请求");

        // submitted
        let initial_task = A2ATask {
            id: task_id.clone(),
            session_id: session_id.clone(),
            status: A2ATaskStatus::new(TaskState::Submitted),
            history: vec![user_message.clone()],
            artifacts: Vec::new(),
        };
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), initial_task);
        }

        // Create and store cancellation token
        let cancel_token = CancellationToken::new();
        self.cancel_tokens
            .write()
            .await
            .insert(task_id.clone(), cancel_token.clone());

        let tasks = self.tasks.clone();
        let agent = self.agent.clone();
        let cancel_tokens = self.cancel_tokens.clone();

        let stream = async_stream::stream! {
            // Event 1: submitted
            yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                task_id: task_id.clone(),
                status: A2ATaskStatus::new(TaskState::Submitted),
                is_final: false,
            });

            // Event 2: working
            Self::update_task_state(&tasks, &task_id, TaskState::Working, None).await;
            yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                task_id: task_id.clone(),
                status: A2ATaskStatus::new(TaskState::Working),
                is_final: false,
            });

            // Execute agent with streaming
            let mut agent_guard = agent.lock().await;
            let stream_result = agent_guard.execute_stream(&input_text).await;

            match stream_result {
                Ok(mut event_stream) => {
                    let mut accumulated_text = String::new();
                    let mut artifact_index: usize = 0;
                    let mut first_chunk = true;

                    while let Some(event_result) = event_stream.next().await {
                        if cancel_token.is_cancelled() {
                            let status = A2ATaskStatus::new(TaskState::Canceled);
                            Self::update_task_state(&tasks, &task_id, TaskState::Canceled, Some(&status)).await;
                            yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                                task_id: task_id.clone(),
                                status,
                                is_final: true,
                            });
                            cancel_tokens.write().await.remove(&task_id);
                            return;
                        }
                        match event_result {
                            Ok(AgentEvent::Token(token)) => {
                                accumulated_text.push_str(&token);
                                yield A2AStreamEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
                                    task_id: task_id.clone(),
                                    artifact: A2AArtifact {
                                        name: if first_chunk { Some("output".to_string()) } else { None },
                                        index: Some(artifact_index),
                                        parts: vec![A2APart::Text { text: token }],
                                        append: !first_chunk,
                                    },
                                    is_final: false,
                                });
                                first_chunk = false;
                            }
                            Ok(AgentEvent::ToolCall { name, .. }) => {
                                yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                                    task_id: task_id.clone(),
                                    status: A2ATaskStatus::with_message(
                                        TaskState::Working,
                                        A2AMessage::agent_text(format!("调用工具: {name}")),
                                    ),
                                    is_final: false,
                                });
                            }
                            Ok(AgentEvent::ToolResult { name, output }) => {
                                yield A2AStreamEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
                                    task_id: task_id.clone(),
                                    artifact: A2AArtifact {
                                        name: Some(format!("tool_result:{name}")),
                                        index: Some(artifact_index),
                                        parts: vec![A2APart::Text { text: output }],
                                        append: false,
                                    },
                                    is_final: false,
                                });
                                artifact_index += 1;
                                first_chunk = true;
                            }
                            Ok(AgentEvent::FinalAnswer(answer)) => {
                                accumulated_text = answer;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(task_id = %task_id, error = %e, "A2A Stream: 事件流错误");
                                let status = A2ATaskStatus::with_message(
                                    TaskState::Failed,
                                    A2AMessage::agent_text(format!("执行失败: {e}")),
                                );
                                Self::update_task_state(&tasks, &task_id, TaskState::Failed, Some(&status)).await;
                                yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                                    task_id: task_id.clone(),
                                    status,
                                    is_final: true,
                                });
                                return;
                            }
                        }
                    }

                    // completed
                    let result_message = A2AMessage::agent_text(&accumulated_text);

                    let completed_status = A2ATaskStatus::with_message(
                        TaskState::Completed,
                        result_message.clone(),
                    );

                    {
                        let mut store = tasks.write().await;
                        if let Some(task) = store.get_mut(&task_id) {
                            task.status = completed_status.clone();
                            task.history.push(result_message);
                            if task.artifacts.is_empty() && !accumulated_text.is_empty() {
                                task.artifacts.push(A2AArtifact {
                                    name: Some("output".to_string()),
                                    index: Some(0),
                                    parts: vec![A2APart::Text { text: accumulated_text }],
                                    append: false,
                                });
                            }
                        }
                    }

                    cancel_tokens.write().await.remove(&task_id);

                    info!(task_id = %task_id, "✅ A2A Stream: 任务完成");

                    yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                        task_id: task_id.clone(),
                        status: completed_status,
                        is_final: true,
                    });
                }
                Err(e) => {
                    warn!(task_id = %task_id, error = %e, "A2A Stream: Agent 执行失败");
                    let status = A2ATaskStatus::with_message(
                        TaskState::Failed,
                        A2AMessage::agent_text(format!("执行失败: {e}")),
                    );
                    Self::update_task_state(&tasks, &task_id, TaskState::Failed, Some(&status)).await;
                    yield A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
                        task_id: task_id.clone(),
                        status,
                        is_final: true,
                    });
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// 将流式事件格式化为 SSE `data:` 行
    ///
    /// 方便在 HTTP 框架中直接使用：
    /// ```rust
    /// use echo_agent::a2a::{A2AServer, A2AStreamEvent, A2ATaskStatus, TaskState, TaskStatusUpdateEvent};
    ///
    /// let event = A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
    ///     task_id: "task-1".to_string(),
    ///     status: A2ATaskStatus::new(TaskState::Working),
    ///     is_final: false,
    /// });
    /// let sse = A2AServer::format_sse_event(&event, "req-1");
    /// assert!(sse.starts_with("data: "));
    /// ```
    pub fn format_sse_event(event: &A2AStreamEvent, request_id: &str) -> String {
        let response = A2AStreamResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: request_id.to_string(),
            result: Some(event.clone()),
            error: None,
        };
        format!(
            "data: {}\n\n",
            serde_json::to_string(&response).unwrap_or_default()
        )
    }

    // ── tasks/send ───────────────────────────────────────────────────────────

    async fn handle_task_send(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = request
            .params
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let input_text = request.params.message.text_content();

        info!(task_id = %task_id, input_len = input_text.len(), "A2A: 收到任务请求");

        // submitted → working
        let task = A2ATask {
            id: task_id.clone(),
            session_id: request.params.session_id.clone(),
            status: A2ATaskStatus::new(TaskState::Submitted),
            history: vec![request.params.message.clone()],
            artifacts: Vec::new(),
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task);
        }

        // Create and store cancellation token
        let cancel_token = CancellationToken::new();
        self.cancel_tokens
            .write()
            .await
            .insert(task_id.clone(), cancel_token.clone());

        Self::update_task_state(&self.tasks, &task_id, TaskState::Working, None).await;

        // working → completed / failed
        let mut agent = self.agent.lock().await;
        match agent.execute(&input_text).await {
            Ok(output) => {
                info!(task_id = %task_id, "A2A: 任务执行完成");

                let result_message = A2AMessage::agent_text(&output);
                let artifact = A2AArtifact {
                    name: Some("output".to_string()),
                    index: Some(0),
                    parts: vec![A2APart::Text {
                        text: output.clone(),
                    }],
                    append: false,
                };

                let completed_task = A2ATask {
                    id: task_id.clone(),
                    session_id: request.params.session_id.clone(),
                    status: A2ATaskStatus::with_message(
                        TaskState::Completed,
                        result_message.clone(),
                    ),
                    history: vec![request.params.message.clone(), result_message],
                    artifacts: vec![artifact],
                };

                {
                    let mut tasks = self.tasks.write().await;
                    tasks.insert(task_id.clone(), completed_task.clone());
                }

                self.cancel_tokens.write().await.remove(&task_id);

                A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id.clone()),
                    result: Some(completed_task),
                    error: None,
                }
            }
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "A2A: 任务执行失败");

                let failed_task = A2ATask {
                    id: task_id.clone(),
                    session_id: request.params.session_id.clone(),
                    status: A2ATaskStatus::with_message(
                        TaskState::Failed,
                        A2AMessage::agent_text(format!("执行失败: {}", e)),
                    ),
                    history: vec![request.params.message.clone()],
                    artifacts: Vec::new(),
                };

                {
                    let mut tasks = self.tasks.write().await;
                    tasks.insert(task_id.clone(), failed_task);
                }

                self.cancel_tokens.write().await.remove(&task_id);

                A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id.clone()),
                    result: None,
                    error: Some(A2AError {
                        code: ERROR_CODE_TASK_FAILED as i32,
                        message: format!("Task execution failed: {}", e),
                    }),
                }
            }
        }
    }

    // ── tasks/get ────────────────────────────────────────────────────────────

    async fn handle_task_get(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = match &request.params.id {
            Some(id) => id.clone(),
            None => {
                return A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id.clone()),
                    result: None,
                    error: Some(A2AError {
                        code: ERROR_CODE_INVALID_PARAMS as i32,
                        message: "Missing task id".to_string(),
                    }),
                };
            }
        };

        let tasks = self.tasks.read().await;
        match tasks.get(&task_id) {
            Some(task) => A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id.clone()),
                result: Some(task.clone()),
                error: None,
            },
            None => A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id.clone()),
                result: None,
                error: Some(A2AError {
                    code: ERROR_CODE_TASK_NOT_FOUND as i32,
                    message: format!("Task not found: {}", task_id),
                }),
            },
        }
    }

    // ── tasks/cancel ─────────────────────────────────────────────────────────

    async fn handle_task_cancel(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = match &request.params.id {
            Some(id) => id.clone(),
            None => {
                return A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id.clone()),
                    result: None,
                    error: Some(A2AError {
                        code: ERROR_CODE_INVALID_PARAMS as i32,
                        message: "Missing task id".to_string(),
                    }),
                };
            }
        };

        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&task_id) {
            if task.status.state.is_terminal() {
                return A2ATaskResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id.clone()),
                    result: None,
                    error: Some(A2AError {
                        code: ERROR_CODE_TERMINAL_STATE as i32,
                        message: format!(
                            "Task '{}' is in terminal state '{}' and cannot be canceled",
                            task_id, task.status.state
                        ),
                    }),
                };
            }

            task.status = A2ATaskStatus::new(TaskState::Canceled);

            // Cancel the token if it exists
            if let Some(token) = self.cancel_tokens.read().await.get(&task_id) {
                token.cancel();
            }

            A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id.clone()),
                result: Some(task.clone()),
                error: None,
            }
        } else {
            A2ATaskResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(request.id.clone()),
                result: None,
                error: Some(A2AError {
                    code: ERROR_CODE_TASK_NOT_FOUND as i32,
                    message: format!("Task not found: {}", task_id),
                }),
            }
        }
    }

    // ── 内部辅助 ─────────────────────────────────────────────────────────────

    async fn update_task_state(
        tasks: &Arc<RwLock<HashMap<String, A2ATask>>>,
        task_id: &str,
        state: TaskState,
        status: Option<&A2ATaskStatus>,
    ) {
        let mut store = tasks.write().await;
        if let Some(task) = store.get_mut(task_id) {
            if !task.status.state.can_transition_to(state) {
                warn!(from = ?task.status.state, to = ?state, "A2A: 非法状态转换，跳过");
                return;
            }
            task.status = status.cloned().unwrap_or_else(|| A2ATaskStatus::new(state));
        }
    }

    /// 清理已完成/失败/取消的任务（由调用方定期调用）
    pub async fn cleanup_completed_tasks(&self, max_age_secs: u64) {
        let mut tasks = self.tasks.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        tasks.retain(|_, task| {
            if !task.status.state.is_terminal() {
                return true;
            }
            // Keep task if we can't determine age (no timestamp) or if it's recent
            if let Some(ts_str) = &task.status.timestamp {
                // Try parsing as ISO 8601; if parsing fails, keep the task
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    let task_secs = ts.timestamp() as u64;
                    return now.saturating_sub(task_secs) < max_age_secs;
                }
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::react::builder::ReactAgentBuilder;

    fn make_request(method: &str, message: &str, task_id: Option<&str>) -> String {
        let params = if let Some(id) = task_id {
            serde_json::json!({
                "id": id,
                "message": { "role": "user", "parts": [{"type": "text", "text": message}] }
            })
        } else {
            serde_json::json!({
                "message": { "role": "user", "parts": [{"type": "text", "text": message}] }
            })
        };

        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "test-req-1",
            "method": method,
            "params": params
        })
        .to_string()
    }

    #[test]
    fn test_task_state_in_response() {
        let status = A2ATaskStatus::new(TaskState::Submitted);
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"state\":\"submitted\""));

        let status = A2ATaskStatus::new(TaskState::InputRequired);
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"state\":\"input-required\""));
    }

    #[test]
    fn test_cancel_terminal_state_rejected() {
        let task = A2ATask {
            id: "t1".into(),
            session_id: None,
            status: A2ATaskStatus::new(TaskState::Completed),
            history: vec![],
            artifacts: vec![],
        };
        assert!(task.status.state.is_terminal());
        assert!(!task.status.state.can_transition_to(TaskState::Canceled));
    }

    #[tokio::test]
    async fn test_handle_task_get_not_found() {
        let card = AgentCard::builder("test", "http://localhost").build();
        let agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("test_agent")
            .system_prompt("test")
            .build()
            .unwrap();
        let server = A2AServer::new(card, agent);

        let req = make_request("tasks/get", "", Some("nonexistent"));
        let resp_json = server.handle_request(&req).await;
        let resp: A2ATaskResponse = serde_json::from_str(&resp_json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32001);
    }

    #[tokio::test]
    async fn test_handle_unknown_method() {
        let card = AgentCard::builder("test", "http://localhost").build();
        let agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("test_agent")
            .system_prompt("test")
            .build()
            .unwrap();
        let server = A2AServer::new(card, agent);

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "unknown/method",
            "params": { "message": { "role": "user", "parts": [{"type":"text","text":"hi"}] } }
        })
        .to_string();
        let resp_json = server.handle_request(&req).await;
        let resp: A2ATaskResponse = serde_json::from_str(&resp_json).unwrap();
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_handle_parse_error() {
        let card = AgentCard::builder("test", "http://localhost").build();
        let agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("test_agent")
            .system_prompt("test")
            .build()
            .unwrap();
        let server = A2AServer::new(card, agent);

        let resp_json = server.handle_request("not json").await;
        let resp: A2ATaskResponse = serde_json::from_str(&resp_json).unwrap();
        assert_eq!(resp.error.unwrap().code, -32700);
    }
}
