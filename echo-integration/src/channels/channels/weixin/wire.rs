//! The text-only adapter normalizes first-party wire messages without inventing
//! identities, dropping mixed attachments, or treating HTTP success as delivery.

use super::auth::protocol_error;
use echo_core::error::Result;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

#[derive(Clone, Serialize, Deserialize)]
pub struct WeixinMessage {
    pub message_id: String,
    pub peer: String,
    pub context_token: Option<String>,
    pub text: String,
    pub unsupported_media: bool,
    pub group_id: Option<String>,
    pub timestamp_ms: Option<i64>,
}

impl fmt::Debug for WeixinMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeixinMessage")
            .field("message_id", &self.message_id)
            .field("peer", &"[redacted]")
            .field(
                "context_token",
                &self.context_token.as_ref().map(|_| "[redacted]"),
            )
            .field("text", &"[redacted]")
            .field("unsupported_media", &self.unsupported_media)
            .field("group_id", &self.group_id.as_ref().map(|_| "[redacted]"))
            .field("timestamp_ms", &self.timestamp_ms)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeixinPollStatus {
    Accepted,
    SessionExpired,
    Rejected { code: i64 },
    Unknown,
}

#[derive(Clone)]
pub struct WeixinUpdates {
    pub status: WeixinPollStatus,
    pub messages: Vec<WeixinMessage>,
    pub cursor: Option<String>,
    pub longpolling_timeout_ms: Option<u64>,
}

impl fmt::Debug for WeixinUpdates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeixinUpdates")
            .field("status", &self.status)
            .field("message_count", &self.messages.len())
            .field("cursor", &self.cursor.as_ref().map(|_| "[redacted]"))
            .field("longpolling_timeout_ms", &self.longpolling_timeout_ms)
            .finish()
    }
}

/// Unknown is deliberately not a retry authorization. The producer must retain
/// the original client ID and settle its original delivery receipt exactly once.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WeixinSendOutcome {
    Accepted {
        client_id: String,
        message_id: Option<String>,
    },
    Rejected {
        code: i64,
    },
    Unknown {
        client_id: String,
        reason: String,
    },
}

#[derive(Deserialize)]
pub(crate) struct UpdatesResponse {
    ret: Option<i64>,
    errcode: Option<i64>,
    #[serde(default)]
    msgs: Vec<RawMessage>,
    get_updates_buf: Option<String>,
    longpolling_timeout_ms: Option<u64>,
}

impl UpdatesResponse {
    pub(crate) fn into_updates(self) -> Result<WeixinUpdates> {
        let status = if self.ret == Some(-14) || self.errcode == Some(-14) {
            WeixinPollStatus::SessionExpired
        } else if let Some(code) = nonzero_code(self.ret, self.errcode) {
            WeixinPollStatus::Rejected { code }
        } else if self.ret == Some(0) {
            WeixinPollStatus::Accepted
        } else {
            WeixinPollStatus::Unknown
        };
        if status != WeixinPollStatus::Accepted {
            // Neither messages nor cursor from an unaccepted response may be
            // committed by the transport as authenticated delivery evidence.
            return Ok(WeixinUpdates {
                status,
                messages: Vec::new(),
                cursor: None,
                longpolling_timeout_ms: None,
            });
        }
        let messages = self
            .msgs
            .into_iter()
            .filter_map(|message| match message.message_type {
                // The platform can return the bot's own messages; never loop
                // them back into the host as new user requests.
                Some(2) => None,
                _ => Some(message.into_message()),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(WeixinUpdates {
            status,
            messages,
            cursor: self.get_updates_buf.filter(|cursor| !cursor.is_empty()),
            longpolling_timeout_ms: self.longpolling_timeout_ms,
        })
    }
}

fn nonzero_code(ret: Option<i64>, errcode: Option<i64>) -> Option<i64> {
    ret.filter(|code| *code != 0)
        .or_else(|| errcode.filter(|code| *code != 0))
}

#[derive(Deserialize)]
pub(crate) struct SendResponse {
    ret: Option<i64>,
    errcode: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_message_id")]
    message_id: Option<String>,
}

impl SendResponse {
    pub(crate) fn into_outcome(self, client_id: &str) -> WeixinSendOutcome {
        if let Some(code) = nonzero_code(self.ret, self.errcode) {
            WeixinSendOutcome::Rejected { code }
        } else if self.ret == Some(0) {
            WeixinSendOutcome::Accepted {
                client_id: client_id.to_owned(),
                message_id: self.message_id,
            }
        } else {
            WeixinSendOutcome::Unknown {
                client_id: client_id.to_owned(),
                reason: "Weixin send response lacks an explicit success code".to_owned(),
            }
        }
    }
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default, deserialize_with = "deserialize_message_id")]
    message_id: Option<String>,
    from_user_id: Option<String>,
    context_token: Option<String>,
    message_type: Option<i64>,
    group_id: Option<String>,
    create_time_ms: Option<i64>,
    #[serde(default)]
    item_list: Vec<MessageItem>,
}

impl RawMessage {
    fn into_message(self) -> Result<WeixinMessage> {
        if self.message_type != Some(1) {
            return Err(protocol_error(
                "Weixin input has an unsupported message type",
            ));
        }
        let message_id = self
            .message_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| protocol_error("Weixin input lacks a stable message identity"))?;
        let peer = self
            .from_user_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| protocol_error("Weixin input lacks its sender identity"))?;
        super::auth::validate_identifier(&message_id)?;
        super::auth::validate_identifier(&peer)?;
        let unsupported_media = self.item_list.iter().any(|item| {
            item.kind != Some(1)
                || item.image_item.is_some()
                || item.voice_item.is_some()
                || item.file_item.is_some()
                || item.video_item.is_some()
        });
        let text = self
            .item_list
            .into_iter()
            .filter_map(|item| item.text_item.and_then(|text| text.text))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(WeixinMessage {
            message_id,
            peer,
            context_token: self.context_token.filter(|token| !token.is_empty()),
            text,
            unsupported_media,
            group_id: self.group_id.filter(|group| !group.is_empty()),
            timestamp_ms: self.create_time_ms,
        })
    }
}

#[derive(Deserialize)]
struct MessageItem {
    #[serde(rename = "type")]
    kind: Option<i64>,
    text_item: Option<TextItem>,
    image_item: Option<serde_json::Value>,
    voice_item: Option<serde_json::Value>,
    file_item: Option<serde_json::Value>,
    video_item: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TextItem {
    text: Option<String>,
}

/// uint64 identifiers must never pass through floating point or host word size.
fn deserialize_message_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| serde::de::Error::custom("invalid Weixin uint64 identity")),
        Some(_) => Err(serde::de::Error::custom("invalid Weixin identity type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_codes_cannot_publish_unaccepted_messages_or_cursor() -> Result<()> {
        for (json, expected) in [
            (
                r#"{"ret":0,"errcode":-14,"get_updates_buf":"secret"}"#,
                WeixinPollStatus::SessionExpired,
            ),
            (
                r#"{"ret":7,"get_updates_buf":"secret"}"#,
                WeixinPollStatus::Rejected { code: 7 },
            ),
            (r#"{"get_updates_buf":"secret"}"#, WeixinPollStatus::Unknown),
        ] {
            let response: UpdatesResponse =
                serde_json::from_str(json).map_err(|_| protocol_error("fixture decode failed"))?;
            let updates = response.into_updates()?;
            assert_eq!(updates.status, expected);
            assert!(updates.messages.is_empty());
            assert!(updates.cursor.is_none());
        }
        Ok(())
    }

    #[test]
    fn uint64_mixed_media_and_unicode_remain_lossless() -> Result<()> {
        let response: UpdatesResponse = serde_json::from_str(
            r#"{"ret":0,"msgs":[{"message_id":18446744073709551615,"message_type":1,"from_user_id":"peer","context_token":"context-secret","item_list":[{"type":1,"text_item":{"text":"你好😀"}},{"type":2,"image_item":{"aeskey":"secret"}}]}],"get_updates_buf":"next"}"#,
        ).map_err(|_| protocol_error("fixture decode failed"))?;
        let updates = response.into_updates()?;
        let message = updates
            .messages
            .first()
            .ok_or_else(|| protocol_error("fixture message missing"))?;
        assert_eq!(message.message_id, "18446744073709551615");
        assert_eq!(message.text, "你好😀");
        assert!(message.unsupported_media);
        assert!(!format!("{message:?}").contains("context-secret"));
        Ok(())
    }

    #[test]
    fn send_needs_explicit_business_acceptance() -> Result<()> {
        for (body, expected) in [
            (
                r#"{"ret":0,"message_id":"18446744073709551615"}"#,
                WeixinSendOutcome::Accepted {
                    client_id: "stable".to_owned(),
                    message_id: Some("18446744073709551615".to_owned()),
                },
            ),
            (r#"{"ret":-14}"#, WeixinSendOutcome::Rejected { code: -14 }),
        ] {
            let response: SendResponse =
                serde_json::from_str(body).map_err(|_| protocol_error("fixture decode failed"))?;
            assert_eq!(response.into_outcome("stable"), expected);
        }
        let missing: SendResponse =
            serde_json::from_str(r#"{}"#).map_err(|_| protocol_error("fixture decode failed"))?;
        assert!(matches!(
            missing.into_outcome("stable"),
            WeixinSendOutcome::Unknown { .. }
        ));
        Ok(())
    }

    #[test]
    fn missing_message_identity_does_not_advance_cursor() -> Result<()> {
        let response: UpdatesResponse = serde_json::from_str(r#"{"ret":0,"msgs":[{"message_type":1,"from_user_id":"peer"}],"get_updates_buf":"next"}"#)
            .map_err(|_| protocol_error("fixture decode failed"))?;
        assert!(response.into_updates().is_err());
        Ok(())
    }
}
