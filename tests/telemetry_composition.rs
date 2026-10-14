#![cfg(feature = "telemetry")]

use echo_agent::telemetry::{
    Metrics, TelemetryConfig, init_telemetry, shutdown_telemetry, telemetry_console_filter,
    telemetry_layer,
};
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::{MetricsService, MetricsServiceServer},
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
    trace_service_server::{TraceService, TraceServiceServer},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{Request, Response, Status};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    Layer as _, filter::FilterExt as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const ENDPOINT_SECRET: &str = "telemetry-endpoint-query-must-not-log-3fd291";

#[derive(Clone, Default)]
struct Collector {
    records: Arc<Mutex<Vec<serde_json::Value>>>,
    file: Option<Arc<Mutex<std::fs::File>>>,
    export_barrier: Option<std::path::PathBuf>,
}
impl Collector {
    async fn wait_for_export_ack(&self) -> std::io::Result<()> {
        if let Some(work) = &self.export_barrier
            && work.join("hold-export").exists()
        {
            std::fs::write(
                work.join("export-held"),
                "actual OTLP request received; response is held",
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            while !work.join("release-export").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Ok(())
    }
    fn record<T: serde::Serialize>(&self, kind: &str, request: &T) -> std::io::Result<()> {
        let mut record = serde_json::Map::new();
        record.insert(
            "kind".to_string(),
            serde_json::Value::String(kind.to_string()),
        );
        record.insert(
            "request".to_string(),
            serde_json::to_value(request).map_err(std::io::Error::other)?,
        );
        let record = serde_json::Value::Object(record);
        self.records
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .push(record.clone());
        if let Some(file) = &self.file {
            use std::io::Write as _;
            let mut file = file
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            writeln!(file, "{record}").map_err(|e| std::io::Error::other(e.to_string()))?;
            file.flush()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(())
    }
}
#[tonic::async_trait]
impl TraceService for Collector {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        self.record("traces", request.get_ref())
            .map_err(|error| Status::internal(error.to_string()))?;
        self.wait_for_export_ack()
            .await
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}
#[tonic::async_trait]
impl MetricsService for Collector {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        self.record("metrics", request.get_ref())
            .map_err(|error| Status::internal(error.to_string()))?;
        self.wait_for_export_ack()
            .await
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

#[derive(Clone)]
struct FormatBuffer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for FormatBuffer {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn collector(
    collector: Collector,
    listener: tokio::net::TcpListener,
    stop: tokio_util::sync::CancellationToken,
) -> TestResult {
    tonic::transport::Server::builder()
        .add_service(TraceServiceServer::new(collector.clone()))
        .add_service(MetricsServiceServer::new(collector))
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            stop.cancelled(),
        )
        .await?;
    Ok(())
}

fn isolated(name: &str) -> TestResult {
    if std::env::var("ECHO_TELEMETRY_TEST_CHILD").as_deref() != Ok(name) {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", name, "--nocapture"])
            .env("ECHO_TELEMETRY_TEST_CHILD", name)
            // INFO spans are intentional test inputs; production honors RUST_LOG.
            .env("RUST_LOG", "info")
            .env("OTEL_METRIC_EXPORT_INTERVAL", "60000")
            .env("OTEL_BSP_SCHEDULE_DELAY", "60000")
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "isolated telemetry case failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))
            .into());
        }
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        if name == "incompatible_subscriber_rejection_leaves_metrics_unpublished" {
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .try_init()
                .map_err(std::io::Error::other)?;
            let result = tokio::task::spawn_blocking(|| {
                init_telemetry(TelemetryConfig {
                    enable_console: false,
                    ..TelemetryConfig::default()
                })
            })
            .await?;
            assert!(result.is_err());
            assert!(Metrics::get().is_none());
            return Ok(());
        }
        if name == "failed_exporter_build_does_not_install_standalone_subscriber" {
            let result = tokio::task::spawn_blocking(|| {
                init_telemetry(TelemetryConfig {
                    otlp_endpoint: "not a valid URI :///".to_string(),
                    ..TelemetryConfig::default()
                })
            })
            .await?;
            assert!(result.is_err());
            assert!(Metrics::get().is_none());
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .try_init()
                .map_err(std::io::Error::other)?;
            return Ok(());
        }
        let capture = Collector::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stop = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(collector(capture.clone(), listener, stop.clone()));
        let composed = name == "composed_layer_preserves_context_and_shutdown_flush";
        let formatted = Arc::new(Mutex::new(Vec::new()));
        if composed {
            let writer = {
                let formatted = formatted.clone();
                move || FormatBuffer(formatted.clone())
            };
            tracing_subscriber::registry()
                .with(telemetry_layer())
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_filter(
                            tracing_subscriber::filter::LevelFilter::ERROR
                                .or(telemetry_console_filter()),
                        ),
                )
                .try_init()
                .map_err(std::io::Error::other)?;
        }
        assert!(Metrics::get().is_none());
        assert!(
            !tracing::enabled!(tracing::Level::TRACE),
            "marker must not enable filtered TRACE callsites"
        );
        tracing::error!("startup-diagnostic-unique");
        let before = tracing::info_span!("created-before-init");
        let config = TelemetryConfig {
            otlp_endpoint: format!("http://{address}?api_key={ENDPOINT_SECRET}"),
            service_name: "telemetry-proof-中文🙂".to_string(),
            enable_console: composed,
        };
        let retry_config = config.clone();
        if name == "concurrent_initialization_has_one_winner" {
            let second = config.clone();
            let (first, second) = tokio::join!(
                tokio::task::spawn_blocking(move || init_telemetry(config)),
                tokio::task::spawn_blocking(move || init_telemetry(second))
            );
            let results = [first?, second?];
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        } else {
            tokio::task::spawn_blocking(move || init_telemetry(config)).await??;
        }
        assert!(Metrics::get().is_some());
        assert!(
            !tracing::enabled!(tracing::Level::TRACE),
            "initialization must retain INFO filter"
        );
        assert!(
            tokio::task::spawn_blocking(move || init_telemetry(retry_config))
                .await?
                .is_err()
        );
        tracing::error!("enabled-console-diagnostic-unique");
        tracing::info!("enabled-console-info-unique");
        let parent = tracing::info_span!("real-parent");
        let context = parent.context();
        let parent_id = context.span().span_context().span_id();
        let parent_trace = context.span().span_context().trace_id();
        assert!(context.span().span_context().is_valid());
        let child = tracing::info_span!(parent: &parent,"real-child");
        child.set_parent(context.clone());
        let child_context = child.context();
        assert_eq!(child_context.span().span_context().trace_id(), parent_trace);
        assert!(child_context.span().span_context().is_valid());
        let external_trace = opentelemetry::trace::TraceId::from_bytes([0x21; 16]);
        let external_span = opentelemetry::trace::SpanId::from_bytes([0x31; 8]);
        let external_context = opentelemetry::Context::new().with_remote_span_context(
            opentelemetry::trace::SpanContext::new(
                external_trace,
                external_span,
                opentelemetry::trace::TraceFlags::SAMPLED,
                true,
                opentelemetry::trace::TraceState::default(),
            ),
        );
        let external_child = tracing::info_span!("external-parent-child");
        external_child.set_parent(external_context);
        assert_eq!(
            external_child.context().span().span_context().trace_id(),
            external_trace
        );
        assert!(external_child.context().span().span_context().is_sampled());
        drop(external_child);
        Metrics::record_llm_call("fixture", "real-model", "success");
        Metrics::record_llm_tokens("fixture", "real-model", "input", 17);
        Metrics::record_tool_execution("read_file", "success");
        drop(child);
        drop(parent);
        drop(before);
        tokio::task::spawn_blocking(shutdown_telemetry).await??;
        // The interval is one minute: receipt here is shutdown's actual flush.
        let records = capture
            .records
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .clone();
        let text = serde_json::to_string(&records)?;
        assert!(text.contains("telemetry-proof-中文🙂"));
        assert!(
            !text.contains(ENDPOINT_SECRET),
            "endpoint credential must not enter exported records"
        );
        assert!(
            text.contains("llm.calls")
                && text.contains("llm.tokens")
                && text.contains("tool.executions")
        );
        assert!(text.contains("real-parent") && text.contains("real-child"));
        let traces: Vec<_> = records
            .iter()
            .filter(|r| r.get("kind").and_then(serde_json::Value::as_str) == Some("traces"))
            .collect();
        assert!(!traces.is_empty());
        let spans: Vec<_> = traces
            .iter()
            .flat_map(|r| {
                r.get("request")
                    .and_then(|r| r.get("resourceSpans"))
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .flat_map(|r| {
                r.get("scopeSpans")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .flat_map(|r| {
                r.get("spans")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .collect();
        assert!(
            !spans
                .iter()
                .any(|s| s.get("name").and_then(serde_json::Value::as_str)
                    == Some("created-before-init"))
        );
        let parent = spans
            .iter()
            .find(|s| s.get("name").and_then(serde_json::Value::as_str) == Some("real-parent"))
            .ok_or_else(|| std::io::Error::other("exported parent missing"))?;
        let child = spans
            .iter()
            .find(|s| s.get("name").and_then(serde_json::Value::as_str) == Some("real-child"))
            .ok_or_else(|| std::io::Error::other("exported child missing"))?;
        assert_eq!(child.get("traceId"), parent.get("traceId"));
        assert_eq!(child.get("parentSpanId"), parent.get("spanId"));
        let external = spans
            .iter()
            .find(|s| {
                s.get("name").and_then(serde_json::Value::as_str) == Some("external-parent-child")
            })
            .ok_or_else(|| std::io::Error::other("external child missing"))?;
        assert_eq!(
            external
                .get("parentSpanId")
                .and_then(serde_json::Value::as_str),
            Some(external_span.to_string().as_str())
        );
        assert_eq!(
            external.get("traceId").and_then(serde_json::Value::as_str),
            Some(external_trace.to_string().as_str())
        );
        assert_ne!(parent_id, opentelemetry::trace::SpanId::INVALID);
        // The original shutdown boundary is retained: creating a later span
        // remains safe and does not permit provider replacement/re-init.
        drop(tracing::info_span!("created-after-shutdown"));
        assert!(
            Metrics::get().is_some(),
            "status remains once-initialized after shutdown"
        );
        assert!(
            tokio::task::spawn_blocking(shutdown_telemetry)
                .await?
                .is_err()
        );
        if composed {
            let bytes = formatted
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .clone();
            let formatted = String::from_utf8(bytes)?;
            assert!(
                !formatted.contains(ENDPOINT_SECRET),
                "endpoint credential must not enter configured diagnostics"
            );
            assert_eq!(formatted.matches("startup-diagnostic-unique").count(), 1);
            assert_eq!(
                formatted
                    .matches("enabled-console-diagnostic-unique")
                    .count(),
                1
            );
            assert_eq!(formatted.matches("enabled-console-info-unique").count(), 1);
        }
        stop.cancel();
        server.await??;
        Ok(())
    })
}

#[test]
fn incompatible_subscriber_rejection_leaves_metrics_unpublished() -> TestResult {
    isolated("incompatible_subscriber_rejection_leaves_metrics_unpublished")
}
#[test]
fn failed_exporter_build_does_not_install_standalone_subscriber() -> TestResult {
    isolated("failed_exporter_build_does_not_install_standalone_subscriber")
}
#[test]
fn concurrent_initialization_has_one_winner() -> TestResult {
    isolated("concurrent_initialization_has_one_winner")
}
#[test]
fn composed_layer_preserves_context_and_shutdown_flush() -> TestResult {
    isolated("composed_layer_preserves_context_and_shutdown_flush")
}
#[test]
fn standalone_initialization_keeps_original_lifecycle() -> TestResult {
    isolated("standalone_initialization_keeps_original_lifecycle")
}

/// Fixture utility invoked explicitly by the native SDK scenarios. Normal
/// test discovery does not start a daemon and claims no collector proof here.
#[test]
fn native_sdk_collector_fixture() -> TestResult {
    let Some(work) =
        std::env::var_os("ECHO_TELEMETRY_COLLECTOR_WORK").map(std::path::PathBuf::from)
    else {
        return Ok(());
    };
    std::fs::create_dir_all(&work)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let capture = Collector {
            records: Arc::new(Mutex::new(Vec::new())),
            file: Some(Arc::new(Mutex::new(std::fs::File::create(work.join("otlp.jsonl"))?))),
            export_barrier: Some(work.clone()),
        };
        std::fs::write(work.join("collector.json"),serde_json::to_vec(&serde_json::json!({"endpoint":format!("http://{address}"),"pid":std::process::id()}))?)?;
        let stop = tokio_util::sync::CancellationToken::new();
        let job = tokio::spawn(collector(capture,listener,stop.clone()));
        while !work.join("stop").exists() { tokio::time::sleep(Duration::from_millis(20)).await; }
        stop.cancel(); job.await??;
        Ok(())
    })
}
