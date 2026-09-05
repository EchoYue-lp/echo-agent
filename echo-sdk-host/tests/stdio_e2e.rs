use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, Error, LineDirection,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::Notify;

const SENTINEL_SECRET: &str = "sdk-host-sentinel-secret";

#[derive(Clone, Copy)]
enum ModelResponse {
    Complete,
    ParkAfterFirstChunk,
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echo-agent-sdk-host"))
}

fn write_config(
    directory: &TempDir,
    endpoint: &str,
    auth_token: Option<&str>,
    api_key_env: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = directory.path().join("host.json");
    let mut model = serde_json::json!({
        "provider": "fixture",
        "name": "fixture-model",
        "base_url": endpoint,
        "api_protocol": "chat_completions"
    });
    if let Some(token) = auth_token {
        model["auth_token"] = serde_json::Value::String(token.to_string());
    }
    let mut document = serde_json::json!({
        "schema_version": 1,
        "default_agent": {
            "model": model,
            "agent": {
                "name": "fixture-agent",
                "system_prompt": "Answer the user directly.",
                "max_iterations": 4,
                "enable_tools": true
            }
        }
    });
    if let Some(name) = api_key_env {
        document["api_key_env"] = serde_json::Value::String(name.to_string());
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
    Ok(path)
}

async fn start_model_server(
    response: ModelResponse,
) -> Result<
    (
        String,
        Arc<Notify>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ),
    Box<dyn std::error::Error>,
> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let request_seen = Arc::new(Notify::new());
    let server_request_seen = request_seen.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = vec![0_u8; 64 * 1024];
        let read = socket.read(&mut request).await?;
        if read == 0 {
            return Err(std::io::Error::other(
                "fixture model received EOF before request bytes",
            ));
        }
        server_request_seen.notify_one();
        match response {
            ModelResponse::Complete => {
                let body = concat!(
                    "data: {\"id\":\"fixture\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"host-ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                );
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await?;
                socket.write_all(body.as_bytes()).await?;
                socket.flush().await?;
                Ok(())
            }
            ModelResponse::ParkAfterFirstChunk => {
                let payload = b"data: {\"id\":\"fixture\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"partial\"},\"finish_reason\":null}]}\n\n";
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                    )
                    .await?;
                socket
                    .write_all(format!("{:X}\r\n", payload.len()).as_bytes())
                    .await?;
                socket.write_all(payload).await?;
                socket.write_all(b"\r\n").await?;
                socket.flush().await?;
                std::future::pending::<std::io::Result<()>>().await
            }
        }
    });
    Ok((
        format!("http://{address}/v1/chat/completions"),
        request_seen,
        server,
    ))
}

fn host_agent(
    config: &Path,
    stdout_lines: Arc<Mutex<Vec<String>>>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
) -> AcpAgent {
    let config_path = config.to_string_lossy().into_owned();
    AcpAgent::new(
        AcpAgentConfig::new(binary())
            .arg("--config")
            .arg(config_path),
    )
    .with_debug(move |line, direction| {
        let target = match direction {
            LineDirection::Stdout => Some(&stdout_lines),
            LineDirection::Stderr => Some(&stderr_lines),
            LineDirection::Stdin => None,
        };
        if let Some(target) = target {
            target
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(line.to_string());
        }
    })
}

fn assert_protocol_stdout(lines: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if lines.is_empty() {
        return Err(std::io::Error::other("Host produced no ACP stdout messages").into());
    }
    for line in lines {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
            return Err(std::io::Error::other("Host stdout line is not JSON-RPC 2.0").into());
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_built_host_completes_standard_acp_prompt() -> Result<(), Box<dyn std::error::Error>>
{
    let (endpoint, _request_seen, server) = start_model_server(ModelResponse::Complete).await?;
    let directory = tempfile::tempdir()?;
    let config = write_config(&directory, &endpoint, Some(SENTINEL_SECRET), None)?;
    let stdout_lines = Arc::new(Mutex::new(Vec::new()));
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let updates = Arc::new(Mutex::new(Vec::<v1::SessionNotification>::new()));
    let captured_updates = updates.clone();

    let client = Client
        .builder()
        .on_receive_notification(
            async move |notification: v1::SessionNotification, _connection: ConnectionTo<Agent>| {
                captured_updates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            host_agent(&config, stdout_lines.clone(), stderr_lines.clone()),
            async move |connection| {
                let initialized = connection
                    .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
                let session = connection
                    .send_request(v1::NewSessionRequest::new(
                        std::env::current_dir().map_err(Error::into_internal_error)?,
                    ))
                    .block_task()
                    .await?
                    .session_id;
                let response = connection
                    .send_request(v1::PromptRequest::new(
                        session,
                        vec![v1::ContentBlock::Text(v1::TextContent::new("hello"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, v1::StopReason::EndTurn);
                Ok(())
            },
        );
    tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .map_err(|_| std::io::Error::other("Host success E2E timed out"))??;
    server.await??;

    let stdout = stdout_lines
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_protocol_stdout(&stdout)?;
    assert!(stdout.iter().all(|line| !line.contains(SENTINEL_SECRET)));
    drop(stdout);
    assert!(
        stderr_lines
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .all(|line| !line.contains(SENTINEL_SECRET))
    );
    assert!(updates.lock().unwrap_or_else(|error| error.into_inner()).iter().any(
        |notification| matches!(
            &notification.update,
            v1::SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, v1::ContentBlock::Text(text) if text.text.contains("host-ok"))
        )
    ));
    Ok(())
}

#[tokio::test]
async fn source_built_host_cancels_parked_model_request() -> Result<(), Box<dyn std::error::Error>>
{
    let (endpoint, request_seen, server) =
        start_model_server(ModelResponse::ParkAfterFirstChunk).await?;
    let directory = tempfile::tempdir()?;
    let config = write_config(&directory, &endpoint, None, None)?;
    let stdout_lines = Arc::new(Mutex::new(Vec::new()));
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));

    let client = Client
        .builder()
        .on_receive_notification(
            async move |_notification: v1::SessionNotification,
                        _connection: ConnectionTo<Agent>| { Ok(()) },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            host_agent(&config, stdout_lines.clone(), stderr_lines.clone()),
            async move |connection| {
                connection
                    .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(v1::NewSessionRequest::new(
                        std::env::current_dir().map_err(Error::into_internal_error)?,
                    ))
                    .block_task()
                    .await?
                    .session_id;
                let prompt = connection.send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "wait for cancellation",
                    ))],
                ));
                tokio::time::timeout(Duration::from_secs(5), request_seen.notified())
                    .await
                    .map_err(|_| Error::internal_error().data("Host sent no model request"))?;
                connection.send_notification(v1::CancelNotification::new(session))?;
                let response = prompt.block_task().await?;
                assert_eq!(response.stop_reason, v1::StopReason::Cancelled);
                Ok(())
            },
        );
    tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .map_err(|_| std::io::Error::other("Host cancellation E2E timed out"))??;
    server.abort();
    let _ = server.await;
    let stdout = stdout_lines
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_protocol_stdout(&stdout)?;
    assert!(stdout.iter().all(|line| !line.contains(SENTINEL_SECRET)));
    drop(stdout);
    assert!(
        stderr_lines
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .all(|line| !line.contains(SENTINEL_SECRET))
    );
    Ok(())
}

#[tokio::test]
async fn stdin_eof_exits_cleanly_without_stdout_noise() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let config = write_config(
        &directory,
        "http://127.0.0.1:9/v1/chat/completions",
        None,
        None,
    )?;
    let mut child = tokio::process::Command::new(binary())
        .arg("--config")
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdin.take());
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .map_err(|_| std::io::Error::other("Host did not exit after stdin EOF"))??;
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_configuration_fails_without_stdout_or_secret()
-> Result<(), Box<dyn std::error::Error>> {
    let missing = tokio::process::Command::new(binary()).output().await?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--config"));

    let directory = tempfile::tempdir()?;
    let conflict = write_config(
        &directory,
        "http://127.0.0.1:9/v1/chat/completions",
        Some(SENTINEL_SECRET),
        Some("SDK_HOST_OTHER_SECRET"),
    )?;
    let output = tokio::process::Command::new(binary())
        .arg("--config")
        .arg(conflict)
        .arg("--check-config")
        .output()
        .await?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mutually exclusive"));
    assert!(!stderr.contains(SENTINEL_SECRET));
    assert!(stderr.chars().count() <= 1100);
    Ok(())
}
