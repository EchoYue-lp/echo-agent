//! im_channels.rs —— 多 IM 平台接入演示
//!
//! 同时启动 QQ Bot 和飞书通道，将消息转交给 Agent 处理。
//!
//! 使用方法：
//! ```bash
//! export QQ_APP_ID="your-qq-app-id"
//! export QQ_CLIENT_SECRET="your-qq-client-secret"
//! export FEISHU_APP_ID="your-feishu-app-id"
//! export FEISHU_APP_SECRET="your-feishu-app-secret"
//! export FEISHU_WEBHOOK_BIND="0.0.0.0:8080"
//!
//! cargo run --example demo38_im_channels --features channels
//! ```

use echo_agent::agent::Agent;
use echo_agent::llm::LlmClient;
use echo_channels::prelude::*;
use echo_providers::LlmConfig;
use std::sync::Arc;

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

    // 1. 创建 LLM 客户端
    let llm_client = create_llm_client()?;

    // 2. 创建 ChannelManager
    let mut manager = ChannelManager::new();

    // 3. 注册 QQ Bot（如果配置了）
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

    // 4. 注册飞书（如果配置了）
    if let (Ok(app_id), Ok(secret), Ok(bind)) = (
        std::env::var("FEISHU_APP_ID"),
        std::env::var("FEISHU_APP_SECRET"),
        std::env::var("FEISHU_WEBHOOK_BIND"),
    ) {
        let feishu_config = FeishuConfig {
            app_id,
            app_secret: secret,
            webhook_bind: bind,
            webhook_path: "/webhook".to_string(),
            verification_token: std::env::var("FEISHU_VERIFICATION_TOKEN").ok(),
        };
        manager.register(Box::new(FeishuChannel::new(feishu_config)?));
        println!("  [+] 已注册飞书通道");
    } else {
        println!("  [-] 跳过飞书（未配置 FEISHU_APP_ID / FEISHU_APP_SECRET / FEISHU_WEBHOOK_BIND）");
    }

    if manager.is_empty() {
        println!("\n  没有可用的通道，请配置至少一个 IM 平台的环境变量。");
        return Ok(());
    }

    println!("\n  共 {} 个通道待启动\n", manager.len());

    // 5. 启动所有通道
    let llm_ref = llm_client.clone();
    let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
        Arc::new(AgentMessageHandler::new(llm_ref.clone()))
    };

    manager.start_all(handler_factory).await?;

    println!("  所有通道已启动，等待消息...");
    println!("  按 Ctrl+C 停止\n");

    // 6. 等待退出信号
    tokio::signal::ctrl_c().await.ok();

    println!("\n  正在关闭...");
    manager.stop_all().await?;
    println!("  所有通道已关闭。");

    Ok(())
}

/// 自定义消息处理器：将 IM 消息转给 Agent 处理
struct AgentMessageHandler {
    llm_client: Arc<dyn LlmClient>,
}

impl AgentMessageHandler {
    fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self { llm_client }
    }
}

#[async_trait::async_trait]
impl MessageHandler for AgentMessageHandler {
    async fn handle(
        &self,
        msg: InboundMessage,
    ) -> echo_agent::error::Result<OutboundMessage> {
        use echo_agent::prelude::ReactAgentBuilder;

        let mut agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .system_prompt("你是一个友好的助手，请用中文简洁回答。")
            .enable_tools()
            .llm_client(self.llm_client.clone())
            .build()?;

        let reply = agent.chat(&msg.text).await?;

        Ok(OutboundMessage::new(
            &msg.channel_id,
            &msg.sender_id,
            msg.chat_type,
            &reply,
        ))
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_agent::error::Result<()> {
        // 由 Channel 的 send_tx 自动处理，不需要手动发送
        Ok(())
    }
}

fn create_llm_client() -> echo_agent::error::Result<Arc<dyn LlmClient>> {
    // 优先使用 OpenAI 兼容接口（支持 DashScope、DeepSeek 等）
    let base_url = std::env::var("OPENAI_BASE_URL").ok();
    let api_key = std::env::var("OPENAI_API_KEY").ok();

    if let (Some(base_url), Some(api_key)) = (base_url, api_key) {
        let config = LlmConfig::new(base_url, api_key, "qwen3-max");
        let client = config.build_client().map_err(|e| {
            echo_agent::error::ReactError::Other(format!("Failed to create LLM client: {}", e))
        })?;
        return Ok(Arc::from(client));
    }

    // 尝试 Anthropic
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
