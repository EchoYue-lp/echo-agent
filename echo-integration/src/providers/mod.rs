pub mod anthropic;
pub mod anthropic_cache;
pub(crate) mod client;
pub mod config;
pub(crate) mod native_message;
pub mod openai;
pub mod responses;
pub mod thinking_translate;

pub use anthropic::AnthropicClient;
pub use anthropic_cache::AnthropicCachePlan;
pub use config::{LlmConfig, SourcedLlmConfig, resolve_protocol_endpoint};
pub use openai::OpenAiClient;
pub use responses::ResponsesClient;

// 三协议都通过各自实际请求构造器运行同一连续增长合同，不接触外部模型。
#[cfg(test)]
fn assert_cache_wire_growth(
    adapt: impl Fn(&echo_core::llm::ChatRequest) -> echo_core::error::Result<serde_json::Value>,
) -> echo_core::error::Result<()> {
    use echo_core::llm::cache::wire::{wire_prompt_fingerprint, wire_prompt_prefix_diff};
    use echo_core::llm::{ChatRequest, types::Message};
    let mut request = ChatRequest {
        messages: vec![
            Message::system("fixed rules".into()),
            Message::user("original request".into()),
        ],
        tools: Some(vec![echo_core::llm::types::ToolDefinition {
            tool_type: "function".into(),
            function: echo_core::llm::types::FunctionSpec {
                name: "lookup".into(),
                description: "original description".into(),
                parameters: serde_json::json!({"type":"object","properties":{}}),
                strict: None,
            },
        }]),
        ..Default::default()
    };
    let mut previous = None;
    for version in 1..=3 {
        request.messages.push(Message::user(format!(
            "<echo-agent-context-projection-v2>\nmarker=\"facts\"\nplacement=tail\nversion=\"{version}\"\n\nlatest facts {version}"
        )));
        let body = adapt(&request)?;
        if let Some(previous) = previous {
            let diff = wire_prompt_prefix_diff(&previous, &body)?;
            assert!(diff.is_append_only, "{diff:?}");
            assert!(diff.common_messages > 0);
            assert_eq!(
                wire_prompt_fingerprint(&previous)?.system_hash,
                wire_prompt_fingerprint(&body)?.system_hash
            );
        }
        let mut changed = request.clone();
        if let Some(tool) = changed.tools.as_mut().and_then(|tools| tools.first_mut()) {
            tool.function.description = "changed description".into();
        }
        let negative = adapt(&changed)?;
        let diff = wire_prompt_prefix_diff(&body, &negative)?;
        assert!(!diff.is_append_only);
        assert!(
            diff.first_difference
                .as_deref()
                .is_some_and(|path| path.starts_with("tools"))
        );
        previous = Some(body);
        request
            .messages
            .push(Message::assistant("model output".into()));
    }
    Ok(())
}
