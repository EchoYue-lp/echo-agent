mod client;
pub mod types;

use crate::config::{Config, Model};
use crate::error::{LlmError, Result};
use crate::llm::client::post;
use crate::llm::types::{ChatCompletionRequest, Message, ToolDefinition};
use reqwest::header::HeaderMap;
use std::sync::OnceLock;

static MODEL_CONFIG: OnceLock<Config> = OnceLock::new();

pub fn get_model(model: &str) -> Model {
    let config = MODEL_CONFIG.get_or_init(|| {
        Config::load("config.yml").expect("Failed to load config. Please ensure config.yml exists.")
    });

    match model {
        "high" => config.models.high.clone(),
        "middle" => config.models.middle.clone(),
        _ => config.models.default.clone(),
    }
}

pub fn assemble_req_header(model: &str) -> HeaderMap {
    let model = get_model(model);
    let mut header_map = HeaderMap::new();

    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.apikey).parse().unwrap(),
    );
    header_map.insert("Content-Type", "application/json".parse().unwrap());
    header_map
}

pub async fn chat(
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
) -> Result<Message> {
    let model = get_model(model_name);
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages,
        temperature,
        max_tokens,
        stream,
        tools,
        tool_choice,
    };

    let header_map = assemble_req_header(model_name);
    let response = post(&request_body, header_map, model.baseurl.as_str()).await?;
    
    


    // 返回第一个选择的消息
    response
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .ok_or_else(|| LlmError::EmptyResponse.into())
}
