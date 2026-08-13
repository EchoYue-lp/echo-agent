//! im_channels.rs —— 多 IM 平台接入演示（使用框架集成）
//!
//! 通过 `AgentChannelHandler` 将 ReactAgent 与 IM 通道桥接，
//! 自动继承框架的所有能力（工具、记忆、MCP、Skills 等）。
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

use echo_agent::channels::{
    AgentChannelHandler, ChannelManager, FeishuChannel, FeishuConfig, MessageHandler, QqChannel,
    QqConfig, SessionConfig, SessionHandler,
};
use echo_agent::config::{apply_env_overrides, load_config};
use echo_agent::prelude::AgentConfig;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,echo_channels=info".into()),
        )
        .init();

    println!("{}", "═".repeat(62));
    println!("      Echo Agent × IM Channels (Framework Integration)");
    println!("{}", "═".repeat(62));
    println!();

    let mut app_config = load_config(None);
    apply_env_overrides(&mut app_config);

    // 1. 读取会话超时配置
    let timeout_minutes = app_config.channels.session.timeout_minutes;

    println!("  会话超时: {} 分钟", timeout_minutes);

    // 2. Agent 配置（自动继承工具、记忆等所有能力）
    let model = app_config.model.name.clone();
    println!("  模型: {}", model);

    // 3. 创建 ChannelManager
    let mut manager = ChannelManager::new();
    let mut registered = Vec::new();

    // 4. 注册 QQ Bot（优先读取 echo-agent.yaml，环境变量已在 apply_env_overrides 中覆盖）
    if app_config.channels.qq.enabled
        && !app_config.channels.qq.app_id.is_empty()
        && !app_config.channels.qq.client_secret.is_empty()
    {
        let qq_config = QqConfig::new(
            app_config.channels.qq.app_id.clone(),
            app_config.channels.qq.client_secret.clone(),
        );
        manager.register(Box::new(QqChannel::new(qq_config)?))?;
        registered.push("qq");
        println!("  [+] 已注册 QQ Bot 通道");
    }

    // 5. 注册飞书（优先读取 echo-agent.yaml，环境变量已在 apply_env_overrides 中覆盖）
    if app_config.channels.feishu.enabled
        && !app_config.channels.feishu.app_id.is_empty()
        && !app_config.channels.feishu.app_secret.is_empty()
    {
        let feishu_config = match app_config.channels.feishu.mode.as_str() {
            "webhook" => FeishuConfig::new_webhook(
                app_config.channels.feishu.app_id.clone(),
                app_config.channels.feishu.app_secret.clone(),
                "0.0.0.0:3001".to_string(),
                "/feishu/webhook".to_string(),
                None,
            ),
            _ => FeishuConfig::new_long_poll(
                app_config.channels.feishu.app_id.clone(),
                app_config.channels.feishu.app_secret.clone(),
            ),
        };
        manager.register(Box::new(FeishuChannel::new(feishu_config)?))?;
        registered.push("feishu");
        println!(
            "  [+] 已注册飞书通道（{} 模式）",
            app_config.channels.feishu.mode
        );
    }

    if manager.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo38 验收失败：没有可用的 IM 通道配置，请至少启用并完整配置一个通道".to_string(),
        ));
    }

    println!("\n  共 {} 个通道待启动\n", manager.len());
    if manager.len() != registered.len() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo38 验收失败：注册通道数不一致，manager={}, recorded={}",
            manager.len(),
            registered.len()
        )));
    }

    // 6. 构建会话配置
    let session_config = SessionConfig::default()
        .with_timeout_minutes(timeout_minutes)
        .with_reset_keywords(app_config.channels.session.reset_keywords.clone())
        .with_reset_commands(app_config.channels.session.reset_commands.clone());
    if timeout_minutes == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "demo38 验收失败：session timeout_minutes 不能为 0".to_string(),
        ));
    }

    // 7. 使用 AgentChannelHandler 桥接 —— 自动继承全部框架能力
    let model_ref = model.clone();
    let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
        let model = model_ref.clone();
        let session_config = session_config.clone();
        Arc::new(SessionHandler::new(
            session_config,
            move || -> Box<dyn MessageHandler> {
                Box::new(AgentChannelHandler::from_config(
                    AgentConfig::standard(
                        &model,
                        "im-assistant",
                        "你是一个友好的助手，请用中文简洁回答。记住我们之前的对话内容。",
                    )
                    .enable_tool(true)
                    .enable_memory(true),
                ))
            },
        ))
    };

    let start_results = manager.start_all(handler_factory).await;
    if start_results.len() != manager.len() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo38 验收失败：启动结果数与通道数不一致，results={}, manager={}",
            start_results.len(),
            manager.len()
        )));
    }
    for result in start_results {
        result.result?;
    }

    println!("  所有通道已启动: {:?}", manager.channel_ids());
    println!("  Agent 已自动启用：工具、记忆、MCP 等能力");
    println!("  进入短暂运行窗口后自动关闭\n");

    tokio::time::sleep(Duration::from_millis(300)).await;

    println!("\n  正在关闭...");
    manager.stop_all().await?;
    println!("  所有通道已关闭。");

    Ok(())
}
