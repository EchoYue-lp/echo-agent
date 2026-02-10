mod client;
pub mod types;

use crate::config::{Config, ModelConfig};
use crate::error::{ReactError, Result};
use crate::llm::client::post;
use crate::llm::types::{ChatCompletionRequest, ChatCompletionResponse, Message, ToolDefinition};
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;

pub fn assemble_req_header(model: &ModelConfig) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();

    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.apikey)
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Authorization header: {}", e)))?,
    );
    header_map.insert(
        "Content-Type",
        "application/json"
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Content-Type header: {}", e)))?,
    );
    Ok(header_map)
}

pub async fn chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
) -> Result<ChatCompletionResponse> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages,
        temperature,
        max_tokens,
        stream,
        tools,
        tool_choice,
    };

    let header_map = assemble_req_header(&model)?;
    post(client, &request_body, header_map, model.baseurl.as_str()).await
}
