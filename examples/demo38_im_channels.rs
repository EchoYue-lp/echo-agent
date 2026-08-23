//! Programmatic IM channel composition.
//!
//! Product config-file discovery belongs to the embedding application. This
//! framework example reads explicit environment values and constructs channel
//! configs directly.

use std::sync::Arc;
use std::time::Duration;

use echo_agent::channels::{
    AgentChannelHandler, ChannelManager, FeishuChannel, FeishuConfig, MessageHandler, QqChannel,
    QqConfig, SessionConfig, SessionHandler,
};
use echo_agent::prelude::AgentConfig;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let model = required_env("ECHO_AGENT_MODEL")?;
    let mut manager = ChannelManager::new();

    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("QQ_APP_ID"),
        std::env::var("QQ_CLIENT_SECRET"),
    ) {
        manager.register(Box::new(QqChannel::new(QqConfig::new(app_id, secret))?))?;
    }

    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("FEISHU_APP_ID"),
        std::env::var("FEISHU_APP_SECRET"),
    ) {
        manager.register(Box::new(FeishuChannel::new(FeishuConfig::new_long_poll(
            app_id, secret,
        ))?))?;
    }

    if manager.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "configure either QQ or Feishu credentials before running demo38".to_string(),
        ));
    }

    let session_config = SessionConfig::default().with_timeout_minutes(60);
    let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
        let model = model.clone();
        Arc::new(SessionHandler::new(
            session_config.clone(),
            move || -> Box<dyn MessageHandler> {
                Box::new(AgentChannelHandler::from_config(AgentConfig::standard(
                    &model,
                    "im-assistant",
                    "Answer the user clearly.",
                )))
            },
        ))
    };

    for result in manager.start_all(handler_factory).await {
        result.result?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    manager.stop_all().await
}

fn required_env(name: &str) -> echo_agent::error::Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            echo_agent::error::ConfigError::MissingConfig("demo38".to_string(), name.to_string())
                .into()
        })
}
