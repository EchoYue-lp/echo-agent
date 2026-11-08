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

// 只表示当前腾讯字段的原始presence；Missing与显式null不是同一业务事实。
#[derive(Default)]
enum PollField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for PollField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}
#[derive(Deserialize)]
struct UpdatesFields {
    #[serde(default)]
    ret: PollField<i64>,
    #[serde(default)]
    errcode: PollField<i64>,
    #[serde(default)]
    msgs: PollField<Vec<RawMessage>>,
    #[serde(default)]
    get_updates_buf: PollField<String>,
    longpolling_timeout_ms: Option<u64>,
}

pub(crate) struct UpdatesResponse {
    ret: Option<i64>,
    errcode: Option<i64>,
    msgs: Vec<RawMessage>,
    get_updates_buf: Option<String>,
    longpolling_timeout_ms: Option<u64>,
    null_code: bool,
    typed_payload_present: bool,
}
impl<'de> Deserialize<'de> for UpdatesResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectOnly;
        impl<'de> serde::de::Visitor<'de> for ObjectOnly {
            type Value = UpdatesResponse;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("current Weixin getupdates JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                // MapAccess复用typed字段解码和duplicate-field拒绝，不将array/空序列当object。
                let fields =
                    UpdatesFields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                let null_code = matches!(&fields.ret, PollField::Null)
                    || matches!(&fields.errcode, PollField::Null);
                let ret = match fields.ret {
                    PollField::Value(value) => Some(value),
                    _ => None,
                };
                let errcode = match fields.errcode {
                    PollField::Value(value) => Some(value),
                    _ => None,
                };
                let has_msgs = matches!(&fields.msgs, PollField::Value(_));
                let msgs = match fields.msgs {
                    PollField::Value(value) => value,
                    PollField::Missing => Vec::new(),
                    PollField::Null => {
                        return Err(serde::de::Error::custom("Weixin msgs must be an array"));
                    }
                };
                let has_cursor = matches!(&fields.get_updates_buf, PollField::Value(_));
                let get_updates_buf = match fields.get_updates_buf {
                    PollField::Value(value) => Some(value),
                    _ => None,
                };
                Ok(UpdatesResponse {
                    ret,
                    errcode,
                    msgs,
                    get_updates_buf,
                    longpolling_timeout_ms: fields.longpolling_timeout_ms,
                    null_code,
                    typed_payload_present: has_msgs || has_cursor,
                })
            }
        }
        deserializer.deserialize_map(ObjectOnly)
    }
}

impl UpdatesResponse {
    pub(crate) fn into_updates(self) -> Result<WeixinUpdates> {
        let status = if self.ret == Some(-14) || self.errcode == Some(-14) {
            WeixinPollStatus::SessionExpired
        } else if let Some(code) = nonzero_code(self.ret, self.errcode) {
            WeixinPollStatus::Rejected { code }
        } else if !self.null_code
            && (self.ret == Some(0) || self.errcode == Some(0) || self.typed_payload_present)
        {
            // 当前官方ret可选；只认可明确业务0或当前typed msgs/cursor，绝非HTTP200/空object。
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
struct SendFields {
    #[serde(default)]
    ret: PollField<i64>,
    #[serde(default)]
    errcode: PollField<i64>,
    #[serde(default, deserialize_with = "deserialize_message_id")]
    message_id: Option<String>,
}

pub(crate) struct SendResponse(SendFields);

impl<'de> Deserialize<'de> for SendResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectOnly;
        impl<'de> serde::de::Visitor<'de> for ObjectOnly {
            type Value = SendResponse;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("current Weixin sendmessage JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                // Reuse typed/presence decoding and duplicate-key rejection, as in poll.
                SendFields::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(SendResponse)
            }
        }
        deserializer.deserialize_map(ObjectOnly)
    }
}

impl SendResponse {
    pub(crate) fn into_outcome(self, client_id: &str) -> WeixinSendOutcome {
        let fields = self.0;
        let code = |field: &PollField<i64>| match field {
            PollField::Value(value) => Some(*value),
            _ => None,
        };
        if let Some(code) = nonzero_code(code(&fields.ret), code(&fields.errcode)) {
            WeixinSendOutcome::Rejected { code }
        } else if matches!(&fields.ret, PollField::Value(0))
            || (matches!(&fields.ret, PollField::Missing)
                && !matches!(&fields.errcode, PollField::Null)
                && fields
                    .message_id
                    .as_ref()
                    .is_some_and(|id| !id.trim().is_empty()))
        {
            // Tencent ret is optional. Only a typed nonempty server ID adds an ACK;
            // HTTP200/missing fields alone never do. Existing explicit ret=0 stays intact.
            // Keep the original lossless string/u64 decoder; do not reinterpret string IDs.
            WeixinSendOutcome::Accepted {
                client_id: client_id.to_owned(),
                message_id: fields.message_id,
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

#[cfg(test)]
mod tencent_current_poll_contract {
    use super::*;

    // Tencent 2.4.10 GetUpdatesResp的ret可选；monitor只拒存在的非零ret/errcode。
    // 这是当前provider合同，不是本系统旧schema兼容；不把HTTP200本身当成功。
    #[test]
    fn current_provider_poll_without_ret_preserves_typed_payload() -> Result<()> {
        for (body, expected_messages, expected_cursor) in [
            (
                r#"{"get_updates_buf":"opaque-next"}"#,
                0,
                Some("opaque-next"),
            ),
            (
                r#"{"errcode":0,"msgs":[],"get_updates_buf":"opaque-next"}"#,
                0,
                Some("opaque-next"),
            ),
            (
                r#"{"msgs":[{"message_id":"current-1","message_type":1,"from_user_id":"test-peer","item_list":[{"type":1,"text_item":{"text":"受控测试正文"}}]}],"get_updates_buf":"opaque-next"}"#,
                1,
                Some("opaque-next"),
            ),
        ] {
            let response: UpdatesResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("current provider fixture could not decode"))?;
            let updates = response.into_updates()?;
            assert_eq!(updates.status, WeixinPollStatus::Accepted);
            assert_eq!(updates.messages.len(), expected_messages);
            assert_eq!(updates.cursor.as_deref(), expected_cursor);
            if expected_messages == 1 {
                let message = updates
                    .messages
                    .first()
                    .ok_or_else(|| protocol_error("current provider message missing"))?;
                assert_eq!(message.message_id, "current-1");
                assert_eq!(message.peer, "test-peer");
                assert_eq!(message.text, "受控测试正文");
                assert!(!message.unsupported_media);
                assert!(message.group_id.is_none());
            }
        }
        Ok(())
    }

    #[test]
    fn current_provider_explicit_error_codes_still_block_messages_and_cursor() -> Result<()> {
        for (codes, expected) in [
            (
                serde_json::json!({"errcode":-14}),
                WeixinPollStatus::SessionExpired,
            ),
            (
                serde_json::json!({"ret":0,"errcode":-14}),
                WeixinPollStatus::SessionExpired,
            ),
            (
                serde_json::json!({"errcode":9}),
                WeixinPollStatus::Rejected { code: 9 },
            ),
            (
                serde_json::json!({"ret":7,"errcode":0}),
                WeixinPollStatus::Rejected { code: 7 },
            ),
        ] {
            let mut value = codes;
            let map = value
                .as_object_mut()
                .ok_or_else(|| protocol_error("fixture code map missing"))?;
            map.insert("msgs".into(), serde_json::json!([{"message_id":"must-not-admit","message_type":1,"from_user_id":"test-peer","item_list":[{"type":1,"text_item":{"text":"must-not-execute"}}]}]));
            map.insert(
                "get_updates_buf".into(),
                serde_json::json!("must-not-commit"),
            );
            let response: UpdatesResponse = serde_json::from_value(value)
                .map_err(|_| protocol_error("current provider fixture could not decode"))?;
            let updates = response.into_updates()?;
            assert_eq!(updates.status, expected);
            assert!(updates.messages.is_empty());
            assert!(updates.cursor.is_none());
        }
        Ok(())
    }

    #[test]
    fn empty_or_unrelated_object_never_becomes_authenticated_poll_ready() -> Result<()> {
        for body in [
            "{}",
            r#"{"unrelated":true}"#,
            r#"{"longpolling_timeout_ms":35000}"#,
        ] {
            let response: UpdatesResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("unknown-shape fixture could not decode"))?;
            let updates = response.into_updates()?;
            assert_eq!(updates.status, WeixinPollStatus::Unknown);
            assert!(updates.messages.is_empty());
            assert!(updates.cursor.is_none());
        }
        Ok(())
    }

    #[test]
    fn null_codes_and_non_object_envelopes_never_establish_poll_ready() -> Result<()> {
        for body in [
            r#"{"ret":null,"msgs":[],"get_updates_buf":"must-not-accept"}"#,
            r#"{"errcode":null,"msgs":[],"get_updates_buf":"must-not-accept"}"#,
            r#"[0,null,[],"must-not-accept",35000]"#,
        ] {
            if let Ok(response) = serde_json::from_str::<UpdatesResponse>(body) {
                let updates = response.into_updates()?;
                assert_ne!(
                    updates.status,
                    WeixinPollStatus::Accepted,
                    "字段缺失与null、object与数组不能被归一化后一起当当前协议成功"
                );
                assert!(updates.messages.is_empty());
                assert!(updates.cursor.is_none());
            }
        }
        Ok(())
    }

    #[test]
    fn malformed_current_poll_values_are_not_business_acceptance() {
        for body in [
            r#"{"ret":"0"}"#,
            r#"{"errcode":[]}"#,
            r#"{"msgs":{}}"#,
            r#"{"get_updates_buf":[]}"#,
            "null",
            "not JSON",
        ] {
            assert!(serde_json::from_str::<UpdatesResponse>(body).is_err());
        }
    }
}

#[cfg(test)]
mod tencent_current_send_contract {
    use super::*;

    #[test]
    fn missing_ret_typed_server_message_id_is_a_lossless_ack() -> Result<()> {
        for (body, expected_id) in [
            (
                r#"{"message_id":"18446744073709551615"}"#,
                "18446744073709551615",
            ),
            (
                r#"{"message_id":18446744073709551615}"#,
                "18446744073709551615",
            ),
            (r#"{"message_id":"9007199254740993"}"#, "9007199254740993"),
            (
                r#"{"errcode":0,"message_id":"18446744073709551615"}"#,
                "18446744073709551615",
            ),
        ] {
            let response: SendResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("send contract fixture decode failed"))?;
            assert_eq!(
                response.into_outcome("original-client"),
                WeixinSendOutcome::Accepted {
                    client_id: "original-client".to_owned(),
                    message_id: Some(expected_id.to_owned()),
                }
            );
        }
        Ok(())
    }

    #[test]
    fn missing_ret_without_an_unambiguous_typed_id_is_unknown() -> Result<()> {
        for body in [
            r#"{}"#,
            r#"{"unrelated":true}"#,
            r#"{"errmsg":"not a receipt"}"#,
            r#"{"errcode":0}"#,
            r#"{"message_id":null}"#,
            r#"{"message_id":""}"#,
            r#"{"message_id":"  "}"#,
            r#"{"ret":null,"message_id":"18446744073709551615"}"#,
            r#"{"errcode":null,"message_id":"18446744073709551615"}"#,
        ] {
            let response: SendResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("send negative fixture decode failed"))?;
            assert!(matches!(response.into_outcome("original-client"),
                WeixinSendOutcome::Unknown { client_id, .. } if client_id == "original-client"));
        }
        Ok(())
    }

    #[test]
    fn nonzero_send_code_precedes_valid_id_and_null_other_code() -> Result<()> {
        for (body, expected_code) in [
            (r#"{"ret":17,"message_id":"18446744073709551615"}"#, 17),
            (r#"{"errcode":17,"message_id":"18446744073709551615"}"#, 17),
            (
                r#"{"ret":0,"errcode":17,"message_id":"18446744073709551615"}"#,
                17,
            ),
            (
                r#"{"ret":null,"errcode":17,"message_id":"18446744073709551615"}"#,
                17,
            ),
            (
                r#"{"ret":17,"errcode":null,"message_id":"18446744073709551615"}"#,
                17,
            ),
            (r#"{"ret":-14,"message_id":"18446744073709551615"}"#, -14),
        ] {
            let response: SendResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("send rejection fixture decode failed"))?;
            assert_eq!(
                response.into_outcome("original-client"),
                WeixinSendOutcome::Rejected {
                    code: expected_code
                }
            );
        }
        Ok(())
    }

    #[test]
    fn explicit_zero_keeps_the_existing_send_receipt_contract() -> Result<()> {
        for (body, expected_id) in [
            (r#"{"ret":0}"#, None),
            (r#"{"ret":0,"message_id":null}"#, None),
            (r#"{"ret":0,"errcode":null}"#, None),
            (
                r#"{"ret":0,"message_id":"18446744073709551615"}"#,
                Some("18446744073709551615"),
            ),
        ] {
            let response: SendResponse = serde_json::from_str(body)
                .map_err(|_| protocol_error("existing send fixture decode failed"))?;
            assert_eq!(
                response.into_outcome("original-client"),
                WeixinSendOutcome::Accepted {
                    client_id: "original-client".to_owned(),
                    message_id: expected_id.map(str::to_owned),
                }
            );
        }
        Ok(())
    }

    #[test]
    fn non_object_or_bad_types_never_establish_send_acceptance() {
        for body in [
            "null",
            "[]",
            r#"[0,null,"18446744073709551615"]"#,
            r#"{"message_id":true}"#,
            r#"{"message_id":1.5}"#,
            r#"{"message_id":-1}"#,
            r#"{"message_id":[]}"#,
            r#"{"ret":"0","message_id":"18446744073709551615"}"#,
            r#"{"errcode":false,"message_id":"18446744073709551615"}"#,
            r#"{"ret":0,"ret":17,"message_id":"18446744073709551615"}"#,
        ] {
            if let Ok(response) = serde_json::from_str::<SendResponse>(body) {
                assert!(matches!(
                    response.into_outcome("original-client"),
                    WeixinSendOutcome::Unknown { .. }
                ));
            }
        }
    }
}
