//! Programmatic IM channel composition.
//!
//! Product config-file discovery belongs to the embedding application. This
//! framework example reads explicit environment values and constructs channel
//! configs directly.

mod support;

use std::sync::Arc;
use std::time::Duration;

use echo_agent::channels::{
    AgentChannelHandler, ChannelManager, ChannelSessionInstance, FeishuChannel, FeishuConfig,
    MessageHandler, QqChannel, QqConfig, SessionConfig, SessionHandler,
};
use echo_agent::prelude::{AgentConfig, LlmClient};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();
    let llm_config = support::llm_config(None)?;
    let model = llm_config.model.clone();
    let llm_client: Arc<dyn LlmClient> = Arc::from(llm_config.build_client()?);
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
        let llm_client = Arc::clone(&llm_client);
        Arc::new(SessionHandler::new(
            session_config.clone(),
            move |instance: &ChannelSessionInstance| -> Box<dyn MessageHandler> {
                let _runtime_incarnation = instance.incarnation_id();
                Box::new(AgentChannelHandler::from_config_with_client(
                    AgentConfig::standard(&model, "im-assistant", "Answer the user clearly."),
                    Arc::clone(&llm_client),
                ))
            },
        ))
    };

    for result in manager.start_all(handler_factory).await {
        result.result?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    manager.stop_all().await
}
