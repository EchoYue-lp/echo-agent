//! A2A HTTP service layer.
//!
//! Provides a full A2A HTTP server based on axum, including:
//! - `GET /.well-known/agent.json` — Agent Card discovery endpoint
//! - `POST /` — JSON-RPC task requests (sync + SSE streaming)
//! - `GET /health` — K8s liveness probe
//! - `GET /ready` — K8s readiness probe
//! - Optional JWT Bearer Token auth middleware
//!
//! # Usage
//!
//! ```rust,no_run
//! use echo_agent::a2a::{A2AServer, AgentCard, serve};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let card = AgentCard::builder("my-agent", "http://localhost:3000").build();
//! let agent = ReactAgentBuilder::simple("qwen3-max", "test")?;
//! let server = A2AServer::new(card, agent);
//! serve(server, "0.0.0.0:3000").await?;
//! # Ok(())
//! # }
//! ```

use super::auth::{JwtConfig, jwt_middleware};
use super::server::A2AServer;
use super::types::*;
use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response, sse::Sse},
    routing::get,
};
use futures::StreamExt;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{info, warn};

/// Default request body size limit (1 MB).
pub const DEFAULT_MAX_BODY_BYTES: usize = 1024 * 1024;

/// A2A server shared state.
#[derive(Clone)]
struct AppState {
    server: Arc<A2AServer>,
}

/// Start A2A HTTP server (no auth, default 1 MB body limit).
///
/// Binds to the given address and serves Agent Card discovery, JSON-RPC task handling, health checks, etc.
pub async fn serve(server: A2AServer, bind_addr: &str) -> crate::error::Result<()> {
    serve_inner(
        server,
        bind_addr,
        JwtConfig::disabled(),
        DEFAULT_MAX_BODY_BYTES,
    )
    .await
}

/// Start A2A HTTP server (with JWT auth, default 1 MB body limit).
///
/// All `/` endpoint requests must carry a valid `Authorization: Bearer <token>` header.
/// `/health` and `/ready` endpoints are not subject to authentication.
pub async fn serve_with_auth(
    server: A2AServer,
    bind_addr: &str,
    jwt_config: JwtConfig,
) -> crate::error::Result<()> {
    serve_inner(server, bind_addr, jwt_config, DEFAULT_MAX_BODY_BYTES).await
}

/// Internal unified implementation
async fn serve_inner(
    server: A2AServer,
    bind_addr: &str,
    jwt_config: JwtConfig,
    max_body_bytes: usize,
) -> crate::error::Result<()> {
    // Security warning: log prominently when auth is disabled on non-loopback addresses
    if !jwt_config.is_enabled() {
        let is_loopback = bind_addr.starts_with("127.0.0.1")
            || bind_addr.starts_with("localhost")
            || bind_addr.starts_with("[::1]")
            || bind_addr == "localhost:0";
        if !is_loopback {
            warn!(
                "⚠️  A2A server starting WITHOUT authentication on {} — \
                 this is insecure for non-local addresses! \
                 Use serve_with_auth() or JwtConfig::hs256() to enable JWT authentication.",
                bind_addr
            );
        }
    }

    let state = AppState {
        server: Arc::new(server),
    };

    let jwt_enabled = jwt_config.is_enabled();
    let jwt_state = Arc::new(jwt_config);

    // Auth-protected route group
    let protected = Router::new()
        .route("/.well-known/agent.json", get(agent_card))
        .route("/", get(|| async { "A2A Server" }).post(handle_json_rpc));

    let protected = if jwt_enabled {
        protected.route_layer(middleware::from_fn_with_state(
            jwt_state.clone(),
            jwt_middleware,
        ))
    } else {
        protected
    };

    // Health check endpoints are not subject to authentication
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .merge(protected)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state);

    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| crate::error::ReactError::Other(format!("Bind address failed: {e}")))?;

    info!(
        addr = %bind_addr,
        jwt = %jwt_enabled,
        max_body_bytes = %max_body_bytes,
        "A2A HTTP server started"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| crate::error::ReactError::Other(format!("Server error: {e}")))?;

    info!("A2A HTTP server shut down gracefully");
    Ok(())
}

/// Wait for SIGINT or SIGTERM signal for graceful shutdown
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = signal::ctrl_c().await {
            warn!(%error, "Failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                warn!(%error, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received SIGINT signal, shutting down gracefully...");
        }
        _ = terminate => {
            info!("Received SIGTERM signal, shutting down gracefully...");
        }
    }
}

// ── Endpoint handlers ─────────────────────────────────────────────────────────

/// `GET /.well-known/agent.json`
async fn agent_card(State(state): State<AppState>) -> Response {
    match state.server.agent_card_json() {
        Ok(json) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({"error": format!("{e}")}).to_string(),
        )
            .into_response(),
    }
}

/// `GET /health` — K8s liveness probe
///
/// Returns 200 as long as the process is alive.
async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"status": "ok"}).to_string(),
    )
}

/// `GET /ready` — K8s readiness probe
///
/// Returns 200 to indicate the service is ready to accept requests.
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let _ = state.server.agent_card();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"status": "ready"}).to_string(),
    )
}

/// `POST /` — JSON-RPC handler
async fn handle_json_rpc(State(state): State<AppState>, body: String) -> Response {
    let method = parse_method(&body);

    if method.as_deref() == Some(METHOD_SEND_SUBSCRIBE) {
        handle_sse(state, &body).await
    } else {
        handle_sync(state, &body).await
    }
}

/// Synchronous JSON-RPC response
async fn handle_sync(state: AppState, body: &str) -> Response {
    let response_json = state.server.handle_request(body).await;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        response_json,
    )
        .into_response()
}

/// SSE streaming response (tasks/sendSubscribe)
async fn handle_sse(state: AppState, body: &str) -> Response {
    let server = state.server.clone();
    let body_owned = body.to_string();

    match A2AServer::handle_request_stream(&server, &body_owned).await {
        Ok(stream) => {
            let sse_stream = stream.map(move |event| {
                let sse_data = A2AServer::format_sse_event(&event, "req-stream");
                Ok::<axum::response::sse::Event, std::convert::Infallible>(
                    axum::response::sse::Event::default().data(sse_data),
                )
            });

            Sse::new(sse_stream).into_response()
        }
        Err(e) => {
            warn!(error = %e, "A2A SSE: stream processing failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({"error": format!("{e}")}).to_string(),
            )
                .into_response()
        }
    }
}

/// Quickly extract the method field from a JSON-RPC request body
fn parse_method(body: &str) -> Option<String> {
    let body = body.trim();

    if let Some(idx) = body.find("\"method\":") {
        let rest = &body[idx + "\"method\":".len()..].trim();
        if let Some(quote_start) = rest.find('"') {
            let after_quote = &rest[quote_start + 1..];
            if let Some(quote_end) = after_quote.find('"') {
                return Some(after_quote[..quote_end].to_string());
            }
        }
    } else if let Some(idx) = body.find("\"method\"") {
        let rest = &body[idx + "\"method\"".len()..].trim();
        if let Some(after_colon) = rest.strip_prefix(':') {
            let after_colon = after_colon.trim();
            if let Some(quote_start) = after_colon.find('"') {
                let after_quote = &after_colon[quote_start + 1..];
                if let Some(quote_end) = after_quote.find('"') {
                    return Some(after_quote[..quote_end].to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ── Deterministic unit tests ──────────────────────────────────────────────

    #[test]
    fn test_parse_method_tasks_send() {
        let body = r#"{"jsonrpc":"2.0","id":"1","method":"tasks/send","params":{}}"#;
        assert_eq!(parse_method(body), Some("tasks/send".to_string()));
    }

    #[test]
    fn test_parse_method_send_subscribe() {
        let body = r#"{"jsonrpc":"2.0","id":"1","method":"tasks/sendSubscribe","params":{}}"#;
        assert_eq!(parse_method(body), Some("tasks/sendSubscribe".to_string()));
    }

    #[test]
    fn test_parse_method_tasks_get() {
        let body = r#"{"jsonrpc":"2.0","id":"1","method":"tasks/get","params":{}}"#;
        assert_eq!(parse_method(body), Some("tasks/get".to_string()));
    }

    #[test]
    fn test_parse_method_invalid_json() {
        assert_eq!(parse_method("not json"), None);
    }

    #[test]
    fn test_parse_method_empty() {
        assert_eq!(parse_method(""), None);
    }

    #[test]
    fn test_parse_method_no_method_field() {
        let body = r#"{"jsonrpc":"2.0","id":"1","params":{}}"#;
        assert_eq!(parse_method(body), None);
    }

    #[test]
    fn test_parse_method_empty_method_value() {
        let body = r#"{"jsonrpc":"2.0","method":"","params":{}}"#;
        assert_eq!(parse_method(body), Some("".to_string()));
    }

    #[test]
    fn test_parse_method_whitespace_around_colon() {
        let body = r#"{"jsonrpc":"2.0","method" : "tasks/send","params":{}}"#;
        assert_eq!(parse_method(body), Some("tasks/send".to_string()));
    }

    #[test]
    fn test_parse_method_method_not_first_field() {
        let body = r#"{"jsonrpc":"2.0","params":{},"method":"tasks/get","id":"1"}"#;
        assert_eq!(parse_method(body), Some("tasks/get".to_string()));
    }

    #[test]
    fn test_parse_method_nested_object_with_method_key() {
        // "method" inside nested params should not be extracted
        let body = r#"{"jsonrpc":"2.0","method":"tasks/send","params":{"method":"inner"}}"#;
        assert_eq!(parse_method(body), Some("tasks/send".to_string()));
    }

    // ── Proptest property-based tests ─────────────────────────────────────────

    /// Helper: build a minimal JSON-RPC-like body with a method field
    fn json_body(method: &str) -> String {
        format!("{{\"method\":\"{}\"}}", method)
    }

    proptest! {
        /// parse_method never panics on arbitrary strings
        #[test]
        fn parse_method_never_panics(s in "\\PC*") {
            let _ = parse_method(&s);
        }

        /// parse_method correctly extracts valid method names from well-formed JSON
        #[test]
        fn parse_method_extracts_valid_method(
            method in "[a-zA-Z0-9_/\\.]+"
        ) {
            let body = json_body(&method);
            let result = parse_method(&body);
            assert_eq!(result, Some(method));
        }

        /// parse_method handles whitespace around colon
        #[test]
        fn parse_method_handles_whitespace_before_colon(
            spaces_before in "[ ]*",
            spaces_after in "[ ]*",
            method in "[a-zA-Z0-9_]+"
        ) {
            let body = format!(
                "{{\"jsonrpc\":\"2.0\",\"method\"{}:{}{}\"{}\",\"params\":{{}}}}",
                spaces_before, spaces_after, spaces_after, method
            );
            let result = parse_method(&body);
            assert_eq!(result, Some(method));
        }

        /// parse_method returns None for random non-JSON strings
        #[test]
        fn parse_method_returns_none_for_random_strings(
            s in "[^\\{\\}\"m]*"
        ) {
            let result = parse_method(&s);
            assert_eq!(result, None);
        }

        /// parse_method handles deeply nested JSON without panic
        #[test]
        fn parse_method_deeply_nested_no_panic(
            depth in 1usize..20usize
        ) {
            let inner = (0..depth).fold("\"value\"".to_string(), |acc, _| {
                format!("{{\"nested\": {}}}", acc)
            });
            let body = format!("{{\"method\":\"test\", \"data\": {}}}", inner);
            let _ = parse_method(&body);
        }

        /// parse_method handles unicode method names without panic
        #[test]
        fn parse_method_unicode_methods_no_panic(
            method in "[\\p{L}\\p{N}_/]+"
        ) {
            let body = format!("{{\"method\":\"{}\",\"id\":\"1\"}}", method);
            let result = parse_method(&body);
            let _ = result;
        }
    }
}
