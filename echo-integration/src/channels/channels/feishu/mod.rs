//! Feishu/Lark Channel Module
//!
//! Supports two receive modes:
//! - WebSocket long connection (Long Poll): no public IP required, pure Rust implementation
//! - Webhook: requires public IP, HTTP event push

use super::super::types::require_sender_id;
use echo_core::error::ChannelError;

pub mod api;
pub mod channel;
pub mod long_poll;
pub mod proto;
pub mod webhook;

pub use channel::{FeishuChannel, FeishuConfig, FeishuMode};

fn sender_scope_for_kind(kind: &str, value: &str) -> Result<String, ChannelError> {
    require_sender_id(Some(value)).map(|value| format!("{kind}:{value}"))
}

pub(super) fn sender_scope(
    open_id: Option<&str>,
    user_id: Option<&str>,
) -> Result<String, ChannelError> {
    match (open_id, user_id) {
        (Some(open_id), _) if !open_id.is_empty() => sender_scope_for_kind("open_id", open_id),
        (_, Some(user_id)) if !user_id.is_empty() => sender_scope_for_kind("user_id", user_id),
        _ => Err(ChannelError::Other(
            "missing channel message sender_id".to_string(),
        )),
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::channels::{
        ChatType, InboundMessage, MessageHandler, OutboundMessage, SessionHandler,
    };
    use async_trait::async_trait;

    struct EchoHandler;

    #[async_trait]
    impl MessageHandler for EchoHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                msg.sender_id.clone(),
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn identity_kinds_with_same_raw_value_create_distinct_sessions() -> Result<(), String> {
        let open_id = sender_scope(Some("same"), None).map_err(|error| error.to_string())?;
        let user_id = sender_scope(None, Some("same")).map_err(|error| error.to_string())?;
        if open_id != "open_id:same" || user_id != "user_id:same" {
            return Err("Feishu sender identity was not namespaced".to_string());
        }

        let sessions = SessionHandler::with_defaults(
            |_instance: &crate::channels::ChannelSessionInstance| -> Box<dyn MessageHandler> {
                Box::new(EchoHandler)
            },
        );
        for (sender_id, message_id) in [(open_id, "m1"), (user_id, "m2")] {
            sessions
                .handle(InboundMessage::new(
                    "feishu",
                    sender_id,
                    "group-1",
                    ChatType::Group,
                    "hello",
                    message_id,
                ))
                .await
                .map_err(|error| error.to_string())?;
        }
        if sessions.active_sessions() != 2 {
            return Err("Feishu identity namespaces shared one session".to_string());
        }
        Ok(())
    }

    #[test]
    fn both_missing_identity_kinds_fail_closed() -> Result<(), String> {
        let result = sender_scope(None, None);
        if !matches!(result, Err(ChannelError::Other(ref message)) if message.contains("sender_id"))
        {
            return Err("missing Feishu sender identity was accepted".to_string());
        }
        Ok(())
    }
}
