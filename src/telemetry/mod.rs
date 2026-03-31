//! OpenTelemetry 集成
//!
//! 提供 OTLP 导出配置和初始化函数，与现有 `tracing` 基础设施集成。
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

use crate::error::Result;
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::TracerProvider;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

/// 初始化 OpenTelemetry tracing
///
/// 注册 OTLP exporter + tracing-opentelemetry layer 到全局 subscriber。
/// 如果 `enable_console` 为 true，同时注册 fmt layer。
pub fn init_telemetry(config: TelemetryConfig) -> Result<()> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .map_err(|e| crate::error::ReactError::Other(format!("OTLP exporter error: {e}")))?;

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(opentelemetry_sdk::Resource::new(vec![KeyValue::new(
            "service.name",
            config.service_name,
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

    Ok(())
}

/// 关闭 OpenTelemetry，刷新待发送的 span
pub fn shutdown_telemetry() {
    opentelemetry::global::shutdown_tracer_provider();
}
