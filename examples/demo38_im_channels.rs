//! im_channels.rs —— 多 IM 平台接入演示
//!
//! 同时启动 QQ Bot 和飞书通道，将消息转交给 Agent 处理。
//!
//! 会话管理由框架 SessionHandler 提供：
//! - 关键词重置：发送 "重置对话"、"新对话"、"/reset" 等触发
//! - 超时重置：会话空闲超过 SESSION_TIMEOUT_MINUTES（默认 60 分钟）后自动重置
//!
//! 使用方法：
//! ```bash
//! export QQ_APP_ID="your-qq-app-id"
//! export QQ_CLIENT_SECRET="your-qq-client-secret"
//! export FEISHU_APP_ID="your-feishu-app-id"
//! export FEISHU_APP_SECRET="your-feishu-app-secret"
//! # 可选：会话超时时间（分钟），默认 60
//! export SESSION_TIMEOUT_MINUTES="60"
//!
//! cargo run --example demo38_im_channels --features channels
//! ```

use async_trait::async_trait;
use echo_agent::agent::Agent;
use echo_agent::llm::LlmClient;
use echo_channels::prelude::*;
use echo_providers::LlmConfig;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 默认会话超时（分钟）
const DEFAULT_SESSION_TIMEOUT_MINUTES: u64 = 60;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,echo_channels=info,im_channels=info".into()),
        )
        .init();

    println!("{}", "═".repeat(62));
    println!("      Echo Agent × IM Channels");
    println!("{}", "═".repeat(62));
    println!();

    // 1. 读取会话超时配置
    let timeout_minutes = std::env::var("SESSION_TIMEOUT_MINUTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SESSION_TIMEOUT_MINUTES);

    println!("  会话超时: {} 分钟", timeout_minutes);

    // 2. 创建 LLM 客户端
    let llm_client = create_llm_client()?;

    // 3. 创建 ChannelManager
    let mut manager = ChannelManager::new();

    // 4. 注册 QQ Bot（如果配置了）
    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("QQ_APP_ID"),
        std::env::var("QQ_CLIENT_SECRET"),
    ) {
        let qq_config = QqConfig {
            app_id,
            client_secret: secret,
        };
        manager.register(Box::new(QqChannel::new(qq_config)?));
        println!("  [+] 已注册 QQ Bot 通道");
    } else {
        println!("  [-] 跳过 QQ Bot（未配置 QQ_APP_ID / QQ_CLIENT_SECRET）");
    }

    // 5. 注册飞书（如果配置了）
    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("FEISHU_APP_ID"),
        std::env::var("FEISHU_APP_SECRET"),
    ) {
        let feishu_config = FeishuConfig::new_long_poll(app_id, secret);
        manager.register(Box::new(FeishuChannel::new(feishu_config)?));
        println!("  [+] 已注册飞书通道（长连接模式）");
    } else {
        println!("  [-] 跳过飞书（未配置 FEISHU_APP_ID / FEISHU_APP_SECRET）");
    }

    if manager.is_empty() {
        println!("\n  没有可用的通道，请配置至少一个 IM 平台的环境变量。");
        return Ok(());
    }

    println!("\n  共 {} 个通道待启动\n", manager.len());

    // 6. 构建会话配置
    let session_config = SessionConfig::default()
        .with_timeout_minutes(timeout_minutes);

    // 7. 启动所有通道 —— 使用框架 SessionHandler 管理会话
    let llm_ref = llm_client.clone();
    let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
        let llm = llm_ref.clone();
        Arc::new(SessionHandler::new(
            session_config.clone(),
            move || -> Box<dyn MessageHandler> {
                Box::new(AgentHandler::new(llm.clone()))
            },
        ))
    };

    manager.start_all(handler_factory).await?;

    println!("  所有通道已启动，等待消息...");
    println!("  按 Ctrl+C 停止\n");

    // 8. 等待退出信号
    tokio::signal::ctrl_c().await.ok();

    println!("\n  正在关闭...");
    manager.stop_all().await?;
    println!("  所有通道已关闭。");

    Ok(())
}

// ── AgentHandler ─────────────────────────────────────────────────────────────

/// 单个 Agent 实例的消息处理器
///
/// SessionHandler 为每个用户创建一个 AgentHandler，
/// 内部持有一个 Agent 实例来维护多轮对话。
struct AgentHandler {
    agent: Arc<Mutex<Box<dyn Agent>>>,
}

impl AgentHandler {
    fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        use echo_agent::prelude::ReactAgentBuilder;

        let agent: Box<dyn Agent> = Box::new(
            ReactAgentBuilder::new()
                .model("deepseek-chat")
                .system_prompt("你是一个友好的助手，请用中文简洁回答。记住我们之前的对话内容。")
                .enable_tools()
                .llm_client(llm_client)
                .build()
                .expect("Failed to create agent"),
        );

        Self {
            agent: Arc::new(Mutex::new(agent)),
        }
    }
}

#[async_trait]
impl MessageHandler for AgentHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_agent::error::Result<OutboundMessage> {
        let mut agent = self.agent.lock().await;
        let reply = agent.chat(&msg.text).await?;

        Ok(OutboundMessage::new(
            &msg.channel_id,
            &msg.sender_id,
            msg.chat_type,
            &reply,
        ))
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_agent::error::Result<()> {
        Ok(())
    }
}

fn create_llm_client() -> echo_agent::error::Result<Arc<dyn LlmClient>> {
    let base_url = std::env::var("OPENAI_BASE_URL").ok();
    let api_key = std::env::var("OPENAI_API_KEY").ok();

    if let (Some(base_url), Some(api_key)) = (base_url, api_key) {
        let config = LlmConfig::new(base_url, api_key, "qwen3-max");
        let client = config.build_client().map_err(|e| {
            echo_agent::error::ReactError::Other(format!("Failed to create LLM client: {}", e))
        })?;
        return Ok(Arc::from(client));
    }

    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        let config = LlmConfig::anthropic(api_key, "claude-sonnet-4-6");
        let client = config.build_client().map_err(|e| {
            echo_agent::error::ReactError::Other(format!("Failed to create LLM client: {}", e))
        })?;
        return Ok(Arc::from(client));
    }

    Err(echo_agent::error::ReactError::Other(
        "未配置 OPENAI_API_KEY 或 ANTHROPIC_API_KEY".to_string(),
    ))
}
