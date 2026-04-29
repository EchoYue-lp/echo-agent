//! OpenTelemetry 集成
//!
//! 提供 OTLP 导出配置和初始化函数，与现有 `tracing` 基础设施集成。
//! 同时提供 Metrics 基础设施（Counter / Histogram），用于记录 LLM 调用、
//! Token 用量、工具执行等关键指标。
//!
//! # 使用方式
//!
//! ```rust,no_run
//! use echo_agent::telemetry::{TelemetryConfig, init_telemetry, shutdown_telemetry};
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! init_telemetry(TelemetryConfig::default())?;
//!
//! // ... 运行 Agent ...
//!
//! shutdown_telemetry();
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
use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ── Config ───────────────────────────────────────────────────────────────────

/// OpenTelemetry 配置
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (gRPC)
    pub otlp_endpoint: String,
    /// 服务名称
    pub service_name: String,
    /// 同时输出到控制台
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

/// 全局 Metrics 实例（延迟初始化）
static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Agent 运行指标
///
/// 提供 LLM 调用、Token 用量、工具执行等核心指标的记录接口。
/// 所有指标通过 OTLP Metrics exporter 导出。
#[derive(Debug, Clone)]
pub struct Metrics {
    /// LLM 调用计数器 (labels: provider, model, status)
    llm_calls: Counter<u64>,
    /// LLM Token 用量计数器 (labels: provider, model, type=input|output)
    llm_tokens: Counter<u64>,
    /// LLM 调用延迟直方图 (labels: provider, model)
    llm_latency: Histogram<f64>,
    /// 工具执行计数器 (labels: tool, status)
    tool_executions: Counter<u64>,
    /// 工具执行延迟直方图 (labels: tool)
    tool_latency: Histogram<f64>,
}

impl Metrics {
    /// 获取全局 Metrics 实例
    pub fn get() -> Option<&'static Metrics> {
        METRICS.get()
    }

    /// 记录一次 LLM 调用
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

    /// 记录 LLM Token 用量
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

    /// 记录 LLM 调用延迟（毫秒）
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

    /// 记录一次工具执行
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

    /// 记录工具执行延迟（毫秒）
    pub fn record_tool_latency(tool: &str, duration_ms: f64) {
        if let Some(m) = Self::get() {
            m.tool_latency
                .record(duration_ms, &[KeyValue::new("tool", tool.to_string())]);
        }
    }
}

// ── Init / Shutdown ──────────────────────────────────────────────────────────

/// 初始化 OpenTelemetry tracing + metrics
///
/// 注册 OTLP exporter + tracing-opentelemetry layer 到全局 subscriber，
/// 同时初始化 MeterProvider 和全局 Metrics。
///
/// 如果 `enable_console` 为 true，同时注册 fmt layer。
pub fn init_telemetry(config: TelemetryConfig) -> Result<()> {
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
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .init();
    }

    // ── Metrics ──
    match init_metrics(&config) {
        Ok(()) => {
            tracing::info!(endpoint = %config.otlp_endpoint, "Metrics initialized");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Metrics initialization failed (non-fatal), continuing without metrics");
        }
    }

    Ok(())
}

/// 初始化 Metrics (MeterProvider + 全局 Instruments)
fn init_metrics(config: &TelemetryConfig) -> Result<()> {
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

    METRICS
        .set(metrics)
        .map_err(|_| ReactError::Other("Metrics already initialized".to_string()))?;

    Ok(())
}

/// 关闭 OpenTelemetry，刷新待发送的 span 和 metrics
pub fn shutdown_telemetry() {
    opentelemetry::global::shutdown_tracer_provider();
}
