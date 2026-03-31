//! A2A HTTP 服务端
//!
//! 提供符合 A2A 协议的 HTTP 端点：
//! - `GET /.well-known/agent.json` — 返回 Agent Card
//! - `POST /` — 处理 JSON-RPC 任务请求（tasks/send、tasks/get 等）

use super::types::*;
use crate::agent::Agent;
use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// A2A 服务端
///
/// 将一个 Agent 暴露为 A2A 协议兼容的 HTTP 服务。
pub struct A2AServer {
    /// Agent Card 描述
    card: AgentCard,
    /// 底层 Agent 实例
    agent: Arc<Mutex<Box<dyn Agent>>>,
    /// 任务存储（任务 ID → 任务状态）
    tasks: Arc<Mutex<HashMap<String, A2ATask>>>,
}

impl A2AServer {
    pub fn new(card: AgentCard, agent: impl Agent + 'static) -> Self {
        Self {
            card,
            agent: Arc::new(Mutex::new(Box::new(agent))),
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn from_boxed(card: AgentCard, agent: Box<dyn Agent>) -> Self {
        Self {
            card,
            agent: Arc::new(Mutex::new(agent)),
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 获取 Agent Card（用于 `/.well-known/agent.json`）
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// 返回 Agent Card 的 JSON 字符串
    pub fn agent_card_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.card).map_err(|e| {
            crate::error::ReactError::Other(format!("Agent Card 序列化失败: {}", e))
        })
    }

    /// 处理 A2A JSON-RPC 请求
    ///
    /// 支持的方法：
    /// - `tasks/send`: 发送任务并等待结果
    /// - `tasks/get`: 查询任务状态
    /// - `tasks/cancel`: 取消任务
    pub async fn handle_request(&self, request_json: &str) -> String {
        let request: A2ATaskRequest = match serde_json::from_str(request_json) {
            Ok(req) => req,
            Err(e) => {
                return serde_json::to_string(&A2ATaskResponse {
                    jsonrpc: "2.0".to_string(),
                    id: "unknown".to_string(),
                    result: None,
                    error: Some(A2AError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    }),
                })
                .unwrap_or_default();
            }
        };

        let response = match request.method.as_str() {
            "tasks/send" => self.handle_task_send(&request).await,
            "tasks/get" => self.handle_task_get(&request).await,
            "tasks/cancel" => self.handle_task_cancel(&request).await,
            _ => A2ATaskResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: None,
                error: Some(A2AError {
                    code: -32601,
                    message: format!("Method not found: {}", request.method),
                }),
            },
        };

        serde_json::to_string(&response).unwrap_or_default()
    }

    /// 处理 tasks/send 请求
    async fn handle_task_send(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = request
            .params
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let input_text = request.params.message.text_content();

        info!(
            task_id = %task_id,
            input_len = input_text.len(),
            "📨 A2A: 收到任务请求"
        );

        // 创建任务记录
        let task = A2ATask {
            id: task_id.clone(),
            session_id: request.params.session_id.clone(),
            status: A2ATaskStatus {
                state: "working".to_string(),
                message: None,
            },
            history: vec![request.params.message.clone()],
            artifacts: Vec::new(),
        };

        {
            let mut tasks = self.tasks.lock().await;
            tasks.insert(task_id.clone(), task);
        }

        // 执行 Agent
        let mut agent = self.agent.lock().await;
        match agent.execute(&input_text).await {
            Ok(output) => {
                info!(task_id = %task_id, "✅ A2A: 任务执行完成");

                let result_message = A2AMessage::agent_text(&output);
                let artifact = A2AArtifact {
                    name: Some("output".to_string()),
                    parts: vec![A2APart::Text {
                        text: output.clone(),
                    }],
                };

                let completed_task = A2ATask {
                    id: task_id.clone(),
                    session_id: request.params.session_id.clone(),
                    status: A2ATaskStatus {
                        state: "completed".to_string(),
                        message: Some(result_message.clone()),
                    },
                    history: vec![request.params.message.clone(), result_message],
                    artifacts: vec![artifact],
                };

                {
                    let mut tasks = self.tasks.lock().await;
                    tasks.insert(task_id.clone(), completed_task.clone());
                }

                A2ATaskResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: Some(completed_task),
                    error: None,
                }
            }
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "❌ A2A: 任务执行失败");

                let failed_task = A2ATask {
                    id: task_id.clone(),
                    session_id: request.params.session_id.clone(),
                    status: A2ATaskStatus {
                        state: "failed".to_string(),
                        message: Some(A2AMessage::agent_text(format!("执行失败: {}", e))),
                    },
                    history: vec![request.params.message.clone()],
                    artifacts: Vec::new(),
                };

                {
                    let mut tasks = self.tasks.lock().await;
                    tasks.insert(task_id.clone(), failed_task);
                }

                A2ATaskResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(A2AError {
                        code: -32000,
                        message: format!("Task execution failed: {}", e),
                    }),
                }
            }
        }
    }

    /// 处理 tasks/get 请求
    async fn handle_task_get(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = match &request.params.id {
            Some(id) => id.clone(),
            None => {
                return A2ATaskResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(A2AError {
                        code: -32602,
                        message: "Missing task id".to_string(),
                    }),
                };
            }
        };

        let tasks = self.tasks.lock().await;
        match tasks.get(&task_id) {
            Some(task) => A2ATaskResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: Some(task.clone()),
                error: None,
            },
            None => A2ATaskResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(A2AError {
                    code: -32001,
                    message: format!("Task not found: {}", task_id),
                }),
            },
        }
    }

    /// 处理 tasks/cancel 请求
    async fn handle_task_cancel(&self, request: &A2ATaskRequest) -> A2ATaskResponse {
        let task_id = match &request.params.id {
            Some(id) => id.clone(),
            None => {
                return A2ATaskResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(A2AError {
                        code: -32602,
                        message: "Missing task id".to_string(),
                    }),
                };
            }
        };

        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(&task_id) {
            task.status = A2ATaskStatus {
                state: "canceled".to_string(),
                message: None,
            };
            A2ATaskResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: Some(task.clone()),
                error: None,
            }
        } else {
            A2ATaskResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(A2AError {
                    code: -32001,
                    message: format!("Task not found: {}", task_id),
                }),
            }
        }
    }
}
