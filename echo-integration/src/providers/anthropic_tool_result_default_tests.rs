//! 仅adapter flags完整控制；真实publisher→HTTP由根consumer控制，不能以手工None冒充。

use super::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::TcpListener;

fn invalid(reason: impl Into<String>) -> LlmError {
    LlmError::InvalidResponse(reason.into())
}

fn tool_request(flag: Option<bool>) -> ChatRequest {
    let mut assistant = Message::assistant(String::new());
    assistant.tool_calls = Some(vec![ToolCall {
        provider_source: None,
        id: "call-original".into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "lookup".into(),
            arguments: "{}".into(),
        },
    }]);
    let mut result = Message::tool_result(
        "call-original".into(),
        "lookup".into(),
        "实际结果中文😀".into(),
    );
    result.tool_result_is_error = flag;
    ChatRequest::new(vec![Message::user("request".into()), assistant, result])
}

fn deepseek_fixture_client(address: std::net::SocketAddr) -> Result<AnthropicClient> {
    let mut client = AnthropicClient::with_base_url(
        format!(
            "http://api.deepseek.com:{}/anthropic/v1/messages",
            address.port()
        ),
        "fixture-only",
        "deepseek-v4-pro",
    );
    client.client = Arc::new(
        Client::builder()
            .no_proxy()
            .resolve("api.deepseek.com", address)
            .build()
            .map_err(|error| invalid(error.to_string()))?,
    );
    Ok(client)
}

#[test]
fn tool_result_default_native_false_true_and_canonical_unknown_are_distinct() -> Result<()> {
    let native = AnthropicClient::new("fixture-only", "claude-sonnet-4-5");
    for flag in [None, Some(false), Some(true)] {
        let request = tool_request(flag);
        let before = serde_json::to_value(request.messages.last())
            .map_err(|error| invalid(error.to_string()))?;
        let wire = serde_json::to_value(native.convert_request(&request)?)
            .map_err(|error| invalid(error.to_string()))?;
        assert_eq!(
            wire.pointer("/messages/2/content/0/is_error").cloned(),
            flag.map(Value::Bool)
        );
        assert_eq!(
            serde_json::to_value(request.messages.last())
                .map_err(|error| invalid(error.to_string()))?,
            before
        );
    }
    Ok(())
}

#[tokio::test]
async fn tool_result_default_deepseek_false_and_unknown_reach_real_http_in_both_modes() -> Result<()>
{
    use super::compatibility_tests::{encode_events, read_request, sse_events, write_response};
    for streaming in [false, true] {
        for flag in [None, Some(false)] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                let request = read_request(&mut socket).await?;
                let body = json!({"id":"default-result","model":"deepseek-v4-pro","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}});
                if streaming {
                    write_response(
                        &mut socket,
                        "text/event-stream",
                        &encode_events(&sse_events(&body)?),
                    )
                    .await?;
                } else {
                    write_response(&mut socket, "application/json", &body.to_string()).await?;
                }
                Ok::<_, echo_core::error::ReactError>(request)
            });
            let client = deepseek_fixture_client(address)?;
            assert!(
                client.capabilities().tool_support,
                "fix must not disable tool capability"
            );
            let request = tool_request(flag);
            let canonical = serde_json::to_value(request.messages.last())
                .map_err(|error| invalid(error.to_string()))?;
            let result = async {
                if streaming {
                    let mut stream = client.chat_stream(request.clone()).await?;
                    let mut finish = None;
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk?;
                        if chunk.finish_reason.is_some() {
                            assert!(finish.is_none());
                            finish = chunk.finish_reason;
                        }
                    }
                    assert_eq!(finish.as_deref(), Some("stop"));
                } else {
                    assert_eq!(
                        client.chat(request.clone()).await?.finish_reason.as_deref(),
                        Some("stop")
                    );
                }
                Ok::<_, echo_core::error::ReactError>(())
            }
            .await;
            if let Err(error) = result {
                server.abort();
                let _settled = server.await;
                return Err(error);
            }
            let sent = tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .map_err(|_| invalid("default-result HTTP fixture did not settle"))?
                .map_err(|error| invalid(error.to_string()))??;
            assert!(
                sent.pointer("/messages/2/content/0/is_error").is_none(),
                "false uses explicit ordinary nonerror default representation, not unsupported wire flag"
            );
            assert_eq!(
                sent.pointer("/messages/2/content/0/content"),
                Some(&json!("实际结果中文😀"))
            );
            assert_eq!(
                serde_json::to_value(request.messages.last())
                    .map_err(|error| invalid(error.to_string()))?,
                canonical
            );
            assert_eq!(
                canonical.get("tool_result_is_error").cloned(),
                flag.map(Value::Bool)
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn tool_result_default_deepseek_true_fails_before_any_http_connection() -> Result<()> {
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let client = deepseek_fixture_client(listener.local_addr()?)?;
        let request = tool_request(Some(true));
        let error = if streaming {
            client.chat_stream(request.clone()).await.err()
        } else {
            client.chat(request.clone()).await.err()
        }
        .ok_or_else(|| invalid("true error marker was silently accepted"))?;
        assert!(error.to_string().contains("is_error"));
        assert!(
            matches!(&error,echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(),LlmError::InvalidResponse(_)))
        );
        assert!(!echo_core::error::AgentFailure::try_from(&error)?.retryable);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "true must be rejected before next HTTP/model connection"
        );
        assert_eq!(
            serde_json::to_value(request.messages.last())
                .map_err(|error| invalid(error.to_string()))?
                .get("tool_result_is_error"),
            Some(&json!(true))
        );
    }
    Ok(())
}
