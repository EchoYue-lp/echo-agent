//! Ollama Chat API provider
//!
//! Implements [`LlmClient`](echo_core::llm::LlmClient) for Ollama's `/api/chat` endpoint.
//! Streaming uses NDJSON (one JSON object per line, `done: true` terminates).

use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{DeltaMessage, FunctionCall, Message, ToolCall};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct OllamaClient {
    client: Arc<Client>,
    model: String,
    base_url: String,
}

impl OllamaClient {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Client::new()),
            model: model.into(),
            base_url: "http://localhost:11434/api/chat".to_string(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Client::new()),
            model: model.into(),
            base_url: base_url.into(),
        }
    }

    fn convert_request(&self, request: &ChatRequest) -> OllamaRequest {
        let messages: Vec<OllamaMessage> = request
            .messages
            .iter()
            .map(|m| {
                let tool_calls: Option<Vec<OllamaToolCall>> = m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| OllamaToolCall {
                            function: OllamaFunctionCall {
                                name: tc.function.name.clone(),
                                arguments: serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or_default(),
                            },
                        })
                        .collect()
                });

                OllamaMessage {
                    role: m.role.clone(),
                    content: m.content.clone().unwrap_or_default(),
                    tool_calls,
                }
            })
            .collect();

        let tools: Option<Vec<OllamaToolDef>> = request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|t| OllamaToolDef {
                    r#type: "function".to_string(),
                    function: OllamaFunctionDef {
                        name: t.function.name.clone(),
                        description: t.function.description.clone(),
                        parameters: t.function.parameters.clone(),
                    },
                })
                .collect()
        });

        OllamaRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            tools,
            options: request.temperature.map(|t| OllamaOptions {
                temperature: Some(t),
            }),
        }
    }

    fn convert_response(&self, resp: OllamaResponse) -> ChatResponse {
        let msg = &resp.message;

        let tool_calls: Option<Vec<ToolCall>> = msg.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .enumerate()
                .map(|(i, tc)| ToolCall {
                    id: format!("call_{i}"),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tc.function.name.clone(),
                        arguments: serde_json::to_string(&tc.function.arguments)
                            .unwrap_or_default(),
                    },
                })
                .collect()
        });

        let finish_reason = if resp.done {
            if tool_calls.is_some() {
                Some("tool_calls".to_string())
            } else {
                Some("stop".to_string())
            }
        } else {
            None
        };

        let message = Message {
            role: msg.role.clone(),
            content: if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            },
            tool_calls,
            tool_call_id: None,
            name: None,
        };

        ChatResponse {
            message,
            finish_reason,
            raw: Default::default(),
        }
    }
}

impl LlmClient for OllamaClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            let body = self.convert_request(&request);

            let resp = self
                .client
                .post(&self.base_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::NetworkError(e.to_string()))?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(LlmError::ApiError {
                    status: status.as_u16(),
                    message: text,
                }
                .into());
            }

            let ollama_resp: OllamaResponse = resp
                .json()
                .await
                .map_err(|e| LlmError::NetworkError(format!("Response parse error: {e}")))?;

            Ok(self.convert_response(ollama_resp))
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        Box::pin(async move {
            let mut body = self.convert_request(&request);
            body.stream = true;

            let resp = self
                .client
                .post(&self.base_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::NetworkError(e.to_string()))?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(LlmError::ApiError {
                    status: status.as_u16(),
                    message: text,
                }
                .into());
            }

            let byte_stream = resp.bytes_stream();
            let mut buffer = String::new();

            let stream = async_stream::stream! {
                let mut byte_stream = std::pin::pin!(byte_stream);
                while let Some(chunk_result) = byte_stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            yield Err(LlmError::NetworkError(e.to_string()).into());
                            return;
                        }
                    };

                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        match serde_json::from_str::<OllamaResponse>(&line) {
                            Ok(resp) => {
                                let finish = if resp.done {
                                    Some("stop".to_string())
                                } else {
                                    None
                                };

                                let content = if resp.message.content.is_empty() {
                                    None
                                } else {
                                    Some(resp.message.content)
                                };

                                yield Ok(ChatChunk {
                                    delta: DeltaMessage {
                                        role: Some(resp.message.role),
                                        content,
                                        tool_calls: None,
                                    },
                                    finish_reason: finish,
                                });

                                if resp.done {
                                    return;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse Ollama stream line: {e}");
                            }
                        }
                    }
                }
            };

            Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// ── Ollama API types ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Serialize)]
struct OllamaToolDef {
    r#type: String,
    function: OllamaFunctionDef,
}

#[derive(Serialize)]
struct OllamaFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
    #[serde(default)]
    done: bool,
}
