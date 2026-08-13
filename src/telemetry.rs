//! OpenTelemetry Integration
//!
//! Provides OTLP export configuration and initialization functions, integrating with
//! the existing `tracing` infrastructure. Also provides Metrics infrastructure
//! (Counter / Histogram) for recording key indicators such as LLM calls,
//! Token usage, and tool execution.
//!
//! # Usage
//!
//! ```rust,no_run
//! use echo_agent::telemetry::{TelemetryConfig, init_telemetry, shutdown_telemetry};
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! init_telemetry(TelemetryConfig::default())?;
//!
//! // ... Run Agent ...
//!
//! shutdown_telemetry()?;
//! # Ok(())
//! # }
//! ```

use crate::error::{ReactError, Result};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::TracerProvider;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ── Config ───────────────────────────────────────────────────────────────────

/// OpenTelemetry configuration
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (gRPC)
    pub otlp_endpoint: String,
    /// Service name
    pub service_name: String,
    /// Also output to console
    pub enable_console: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: "http://localhost:4317".to_string(),
            service_name: "echo-agent".to_string(),
            enable_console: true,
        }
    }
}

// ── Metrics ──────────────────────────────────────────────────────────────────

/// Global Metrics instance (lazy initialized)
static METRICS: OnceLock<Metrics> = OnceLock::new();
static TELEMETRY_RUNTIME: OnceLock<TelemetryRuntime> = OnceLock::new();
static TELEMETRY_INIT_LOCK: Mutex<()> = Mutex::new(());

struct TelemetryRuntime {
    tracer_provider: TracerProvider,
    meter_provider: SdkMeterProvider,
}

/// Agent runtime metrics
///
/// Provides recording interfaces for core metrics such as LLM calls, Token usage,
/// and tool execution. All metrics are exported via OTLP Metrics exporter.
#[derive(Debug, Clone)]
pub struct Metrics {
    /// LLM call counter (labels: provider, model, status)
    llm_calls: Counter<u64>,
    /// LLM Token usage counter (labels: provider, model, type=input|output)
    llm_tokens: Counter<u64>,
    /// LLM call latency histogram (labels: provider, model)
    llm_latency: Histogram<f64>,
    /// Tool execution counter (labels: tool, status)
    tool_executions: Counter<u64>,
    /// Tool execution latency histogram (labels: tool)
    tool_latency: Histogram<f64>,
}

impl Metrics {
    /// Get global Metrics instance
    pub fn get() -> Option<&'static Metrics> {
        METRICS.get()
    }

    /// Record an LLM call
    pub fn record_llm_call(provider: &str, model: &str, status: &str) {
        if let Some(m) = Self::get() {
            m.llm_calls.add(
                1,
                &[
                    KeyValue::new("provider", provider.to_string()),
                    KeyValue::new("model", model.to_string()),
                    KeyValue::new("status", status.to_string()),
                ],
            );
        }
    }

    /// Record LLM Token usage
    pub fn record_llm_tokens(provider: &str, model: &str, token_type: &str, count: u64) {
        if let Some(m) = Self::get() {
            m.llm_tokens.add(
                count,
                &[
                    KeyValue::new("provider", provider.to_string()),
                    KeyValue::new("model", model.to_string()),
                    KeyValue::new("type", token_type.to_string()),
                ],
            );
        }
    }

    /// Record LLM call latency (milliseconds)
    pub fn record_llm_latency(provider: &str, model: &str, duration_ms: f64) {
        if let Some(m) = Self::get() {
            m.llm_latency.record(
                duration_ms,
                &[
                    KeyValue::new("provider", provider.to_string()),
                    KeyValue::new("model", model.to_string()),
                ],
            );
        }
    }

    /// Record a tool execution
    pub fn record_tool_execution(tool: &str, status: &str) {
        if let Some(m) = Self::get() {
            m.tool_executions.add(
                1,
                &[
                    KeyValue::new("tool", tool.to_string()),
                    KeyValue::new("status", status.to_string()),
                ],
            );
        }
    }

    /// Record tool execution latency (milliseconds)
    pub fn record_tool_latency(tool: &str, duration_ms: f64) {
        if let Some(m) = Self::get() {
            m.tool_latency
                .record(duration_ms, &[KeyValue::new("tool", tool.to_string())]);
        }
    }
}

// ── Init / Shutdown ──────────────────────────────────────────────────────────

/// Initialize OpenTelemetry tracing + metrics
///
/// Registers OTLP exporter + tracing-opentelemetry layer to the global subscriber,
/// and initializes MeterProvider and global Metrics.
///
/// If `enable_console` is true, also registers a fmt layer.
pub fn init_telemetry(config: TelemetryConfig) -> Result<()> {
    let _init_guard = TELEMETRY_INIT_LOCK.lock().map_err(|error| {
        ReactError::Other(format!("telemetry initialization lock poisoned: {error}"))
    })?;
    if TELEMETRY_RUNTIME.get().is_some() {
        return Err(ReactError::Other(
            "Telemetry is already initialized".to_string(),
        ));
    }
    // ── Tracing ──
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .map_err(|e| ReactError::Other(format!("OTLP exporter error: {e}")))?;

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(opentelemetry_sdk::Resource::new(vec![KeyValue::new(
            "service.name",
            config.service_name.clone(),
        )]))
        .build();

    let tracer = provider.tracer("echo-agent");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let (metrics, meter_provider) = build_metrics(&config)?;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if config.enable_console {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_target(false);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .with(fmt_layer)
            .try_init()
            .map_err(|error| {
                ReactError::Other(format!("tracing subscriber init failed: {error}"))
            })?;
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .try_init()
            .map_err(|error| {
                ReactError::Other(format!("tracing subscriber init failed: {error}"))
            })?;
    }

    // Publish globals only after exporters and the subscriber are all ready.
    METRICS
        .set(metrics)
        .map_err(|_| ReactError::Other("Metrics already initialized".to_string()))?;
    TELEMETRY_RUNTIME
        .set(TelemetryRuntime {
            tracer_provider: provider,
            meter_provider,
        })
        .map_err(|_| ReactError::Other("Telemetry is already initialized".to_string()))?;
    tracing::info!(endpoint = %config.otlp_endpoint, "Metrics initialized");

    Ok(())
}

/// Initialize Metrics (MeterProvider + global Instruments)
fn build_metrics(config: &TelemetryConfig) -> Result<(Metrics, SdkMeterProvider)> {
    let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .map_err(|e| ReactError::Other(format!("OTLP metrics exporter error: {e}")))?;

    let reader =
        PeriodicReader::builder(metrics_exporter, opentelemetry_sdk::runtime::Tokio).build();

    let meter_provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(opentelemetry_sdk::Resource::new(vec![KeyValue::new(
            "service.name",
            config.service_name.clone(),
        )]))
        .build();

    let meter = meter_provider.meter("echo-agent");

    let metrics = Metrics {
        llm_calls: meter
            .u64_counter("llm.calls")
            .with_description("Number of LLM API calls")
            .build(),
        llm_tokens: meter
            .u64_counter("llm.tokens")
            .with_description("Number of tokens consumed")
            .with_unit("tokens")
            .build(),
        llm_latency: meter
            .f64_histogram("llm.latency")
            .with_description("LLM API call latency in milliseconds")
            .with_unit("ms")
            .build(),
        tool_executions: meter
            .u64_counter("tool.executions")
            .with_description("Number of tool executions")
            .build(),
        tool_latency: meter
            .f64_histogram("tool.latency")
            .with_description("Tool execution latency in milliseconds")
            .with_unit("ms")
            .build(),
    };

    Ok((metrics, meter_provider))
}

/// Shutdown OpenTelemetry, flush pending spans and metrics
pub fn shutdown_telemetry() -> Result<()> {
    let runtime = TELEMETRY_RUNTIME
        .get()
        .ok_or_else(|| ReactError::Other("Telemetry is not initialized".to_string()))?;
    runtime
        .meter_provider
        .shutdown()
        .map_err(|error| ReactError::Other(format!("metrics shutdown failed: {error}")))?;
    runtime
        .tracer_provider
        .shutdown()
        .map_err(|error| ReactError::Other(format!("tracing shutdown failed: {error}")))?;
    Ok(())
}
