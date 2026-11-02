//! 原 sender 的旁路观测，不参与业务接受、第一错误、重试或投递结算。

use super::super::wire::WeixinSendOutcome;
use reqwest::Request;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PUBLIC_ERROR_CHARS: usize = 160;
const OBSERVED_REQUEST_ID_CHARS: usize = 128;

// 不接受任意服务器正文作公开日志。未知 errmsg 只留类型、长度、摘要。
// 以下只声明可公开字面，不是错误码映射或本次 -2 的原因判断。
const PUBLIC_ERROR_LITERALS: &[&str] = &[
    "invalid parameter",
    "invalid params",
    "invalid argument",
    "invalid request",
    "invalid token",
    "invalid context token",
    "invalid context_token",
    "context token expired",
    "context_token expired",
    "context_token is expired",
    "context expired",
    "access token expired",
    "system error",
    "internal error",
    "参数错误",
    "上下文已过期",
    "上下文令牌已过期",
    "令牌已过期",
    "系统错误",
];

#[derive(Default)]
enum ObservedField {
    #[default]
    Missing,
    Present(Value),
}

impl<'de> Deserialize<'de> for ObservedField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Deserialize)]
struct ResponseFields {
    #[serde(default)]
    ret: ObservedField,
    #[serde(default)]
    errcode: ObservedField,
    #[serde(default)]
    errmsg: ObservedField,
    #[serde(default)]
    request_id: ObservedField,
}

/// 此类型不实现 Debug；只允许输出 public_view 的显式白名单字段。
pub(super) struct SendDiagnostic {
    view: Value,
    private_values: Vec<String>,
}

impl SendDiagnostic {
    pub(super) fn new(request: &Request, client_id: &str) -> Self {
        let mut private_values = vec![client_id.to_string()];
        // 所有原header值仅用于内存排除；provider字段若回显任一原值也不能明文日志。
        for value in request
            .headers()
            .values()
            .filter_map(|value| std::str::from_utf8(value.as_bytes()).ok())
        {
            private_values.push(value.to_string());
        }
        if let Some(value) = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
            && let Some(token) = value.strip_prefix("Bearer ")
        {
            private_values.push(token.to_string());
        }
        // 仅在进程内排除原目标和 context；不打印请求、headers 或 request Debug。
        if let Some(body) = request.body().and_then(reqwest::Body::as_bytes)
            && let Ok(value) = serde_json::from_slice::<Value>(body)
        {
            for path in [
                "/msg/to_user_id",
                "/msg/context_token",
                "/msg/item_list/0/text_item/text",
            ] {
                if let Some(value) = value.pointer(path).and_then(Value::as_str) {
                    private_values.push(value.to_string());
                }
            }
        }
        private_values.retain(|value| !value.is_empty());
        Self {
            view: json!({
                "endpoint_kind":"weixin_sendmessage",
                "local_client_id_sha256":format!("{:x}", Sha256::digest(client_id.as_bytes())),
                "stage":"transport_pending",
                "http_status":null,
                "body_captured":false,
                "observed_response_request_ids":[],
                "observation_only":true
            }),
            private_values,
        }
    }

    pub(super) fn stage(&mut self, stage: &'static str) {
        self.put("stage", json!(stage));
    }

    pub(super) fn response(&mut self, status: u16, response_request_id: Option<&str>) {
        self.put("http_status", json!(status));
        self.stage("http_response");
        if let Some(value) = response_request_id {
            self.request_id("response_header:x-request-id", value);
        }
    }

    pub(super) fn body(&mut self, bytes: &[u8]) {
        self.put("body_captured", json!(true));
        self.put("response_bytes", json!(bytes.len()));
        self.put(
            "response_sha256",
            json!(format!("{:x}", Sha256::digest(bytes))),
        );
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            self.put("fields_available", json!(false));
            return;
        };
        if !value.is_object() {
            self.put("fields_available", json!(false));
            return;
        }
        // known 字段重复时不选择最后一个值；诊断解码失败不覆盖原 producer 错误。
        let Ok(fields) = serde_json::from_slice::<ResponseFields>(bytes) else {
            self.put("fields_available", json!(false));
            return;
        };
        self.put("fields_available", json!(true));
        self.put("ret", Self::field(&fields.ret, true));
        self.put("errcode", Self::field(&fields.errcode, true));
        let mut errmsg = Self::field(&fields.errmsg, false);
        if let ObservedField::Present(Value::String(text)) = &fields.errmsg {
            if let Some(object) = errmsg.as_object_mut() {
                object.insert("characters".into(), json!(text.chars().count()));
                object.insert(
                    "sha256".into(),
                    json!(format!("{:x}", Sha256::digest(text.as_bytes()))),
                );
            }
            let canonical = text.trim().to_ascii_lowercase();
            if PUBLIC_ERROR_LITERALS.contains(&canonical.as_str())
                && !text.chars().any(char::is_control)
                && !self.contains_private_value(text)
                && let Some(object) = errmsg.as_object_mut()
            {
                object.insert(
                    "public_text".into(),
                    json!(
                        text.trim()
                            .chars()
                            .take(PUBLIC_ERROR_CHARS)
                            .collect::<String>()
                    ),
                );
            }
        }
        self.put("errmsg", errmsg);
        if let ObservedField::Present(Value::String(value)) = &fields.request_id {
            self.request_id("response_body:request_id", value);
        }
    }

    fn put(&mut self, key: &str, value: Value) {
        if let Some(object) = self.view.as_object_mut() {
            object.insert(key.to_string(), value);
        }
    }

    fn contains_private_value(&self, value: &str) -> bool {
        self.private_values
            .iter()
            .any(|private| value.contains(private))
    }

    fn request_id(&mut self, source: &'static str, value: &str) {
        // 原值过长或可能含私密内容时整项省略，绝不截成另一个身份，也不生成替代ID。
        if value.is_empty()
            || value.chars().count() > OBSERVED_REQUEST_ID_CHARS
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_.:".contains(ch))
            || self.contains_private_value(value)
        {
            return;
        }
        if let Some(ids) = self
            .view
            .get_mut("observed_response_request_ids")
            .and_then(Value::as_array_mut)
        {
            ids.push(json!({"source":source,"value":value}));
        }
    }

    fn field(field: &ObservedField, numeric: bool) -> Value {
        let ObservedField::Present(value) = field else {
            return json!({"presence":"absent"});
        };
        let mut view = json!({"presence":"present","type":super::json_kind(value)});
        if numeric
            && let Some(code) = value.as_i64()
            && let Some(object) = view.as_object_mut()
        {
            object.insert("code".into(), json!(code));
        }
        view
    }

    pub(super) fn error_view(&self, outcome: &WeixinSendOutcome) -> Option<Value> {
        let kind = match outcome {
            WeixinSendOutcome::Accepted { .. } => return None,
            WeixinSendOutcome::Rejected { .. } => "rejected",
            WeixinSendOutcome::Unknown { .. } => "unknown",
        };
        let mut view = self.view.clone();
        if let Some(object) = view.as_object_mut() {
            object.insert("original_outcome".into(), json!(kind));
        }
        Some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<SendDiagnostic, Box<dyn std::error::Error>> {
        let url = url::Url::parse("https://example.invalid/ilink/bot/sendmessage")?;
        let mut request = Request::new(reqwest::Method::POST, url);
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_static("Bearer TOKEN_SECRET"),
        );
        request.headers_mut().insert(
            "x-private-header",
            reqwest::header::HeaderValue::from_static("HEADER_SECRET"),
        );
        *request.body_mut() = Some(reqwest::Body::from(
            json!({"msg":{"to_user_id":"PEER_SECRET","context_token":"CONTEXT_SECRET",
                "item_list":[{"type":1,"text_item":{"text":"MESSAGE_SECRET"}}]}})
            .to_string(),
        ));
        Ok(SendDiagnostic::new(&request, "LOCAL_CLIENT_SECRET"))
    }

    fn rejected() -> WeixinSendOutcome {
        WeixinSendOutcome::Rejected { code: -2 }
    }

    #[test]
    fn observed_error_keeps_distinct_codes_and_only_real_response_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut observation = fixture()?;
        observation.response(200, Some("server-header-id"));
        let bytes = br#"{"ret":-2,"errcode":17,"errmsg":"invalid parameter","request_id":"server-body-id"}"#;
        let original = serde_json::from_slice::<super::super::super::wire::SendResponse>(bytes)?
            .into_outcome("LOCAL_CLIENT_SECRET");
        observation.body(bytes);
        let view = observation
            .error_view(&original)
            .ok_or("missing error view")?;
        assert_eq!(view.pointer("/ret/code"), Some(&json!(-2)));
        assert_eq!(view.pointer("/errcode/code"), Some(&json!(17)));
        assert_eq!(
            view.pointer("/errmsg/public_text"),
            Some(&json!("invalid parameter"))
        );
        assert_eq!(view.get("http_status"), Some(&json!(200)));
        assert_eq!(
            view.get("observed_response_request_ids")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(original, WeixinSendOutcome::Rejected { code: -2 });
        Ok(())
    }

    #[test]
    fn unknown_error_text_and_private_values_never_enter_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut observation = fixture()?;
        observation.response(200, Some("PEER_SECRET"));
        observation.body(br#"{"ret":-2,"errmsg":"TOKEN_SECRET CONTEXT_SECRET PEER_SECRET LOCAL_CLIENT_SECRET https://secret.invalid/","request_id":"CONTEXT_SECRET","secret-key":"secret-body"}"#);
        let view = observation
            .error_view(&rejected())
            .ok_or("missing error view")?;
        let text = view.to_string();
        for secret in [
            "TOKEN_SECRET",
            "CONTEXT_SECRET",
            "PEER_SECRET",
            "LOCAL_CLIENT_SECRET",
            "secret-key",
            "secret-body",
            "secret.invalid",
        ] {
            assert!(!text.contains(secret));
        }
        assert!(view.pointer("/errmsg/public_text").is_none());
        assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
        // 原header/body字面即使被服务器放入request_id，也只能留空而不能当公开ID。
        for private in ["MESSAGE_SECRET", "HEADER_SECRET"] {
            let mut echoed = fixture()?;
            echoed.response(200, Some(private));
            echoed.body(
                json!({"ret":-2,"request_id":private,"errmsg":private})
                    .to_string()
                    .as_bytes(),
            );
            let view = echoed
                .error_view(&rejected())
                .ok_or("missing echoed error view")?;
            assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
            assert!(view.pointer("/errmsg/public_text").is_none());
            assert!(!view.to_string().contains(private));
        }
        Ok(())
    }

    #[test]
    fn utf8_header_and_authorization_echoes_cannot_become_public_errmsg()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, value) in [
            ("x-private-header", "参数错误"),
            ("authorization", "Bearer 参数错误"),
        ] {
            let url = url::Url::parse("https://example.invalid/ilink/bot/sendmessage")?;
            let mut request = Request::new(reqwest::Method::POST, url);
            request.headers_mut().insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                reqwest::header::HeaderValue::from_str(value)?,
            );
            let mut observation = SendDiagnostic::new(&request, "LOCAL_CLIENT_SECRET");
            observation.body("{\"ret\":-2,\"errmsg\":\"参数错误\"}".as_bytes());
            let view = observation
                .error_view(&rejected())
                .ok_or("missing UTF8 error view")?;
            assert!(view.pointer("/errmsg/public_text").is_none());
            assert!(!view.to_string().contains("参数错误"));
        }
        Ok(())
    }

    #[test]
    fn missing_null_wrong_types_and_duplicate_fields_are_not_normalized()
    -> Result<(), Box<dyn std::error::Error>> {
        for (body, ret_type) in [
            (r#"{"errcode":-2}"#, None),
            (r#"{"ret":null,"errcode":-2}"#, Some("null")),
            (r#"{"ret":"-2","errcode":-2}"#, Some("string")),
        ] {
            let mut observation = fixture()?;
            observation.body(body.as_bytes());
            let view = observation
                .error_view(&rejected())
                .ok_or("missing error view")?;
            assert_eq!(view.pointer("/ret/type").and_then(Value::as_str), ret_type);
            assert!(view.pointer("/ret/code").is_none());
            assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
        }
        let mut duplicate = fixture()?;
        duplicate.body(br#"{"ret":-2,"ret":0,"errmsg":"invalid parameter"}"#);
        let view = duplicate
            .error_view(&rejected())
            .ok_or("missing error view")?;
        assert_eq!(view.get("fields_available"), Some(&json!(false)));
        assert!(view.get("ret").is_none());
        Ok(())
    }

    #[test]
    fn unicode_and_oversized_or_absent_request_ids_remain_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut observation = fixture()?;
        observation.response(200, Some(&"x".repeat(129)));
        observation.body("{\"ret\":-2,\"errmsg\":\"参数错误\"}".as_bytes());
        let view = observation
            .error_view(&rejected())
            .ok_or("missing error view")?;
        assert_eq!(
            view.pointer("/errmsg/public_text"),
            Some(&json!("参数错误"))
        );
        assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
        assert!(view.to_string().chars().count() < 2048);
        Ok(())
    }

    #[test]
    fn unavailable_http_and_success_keep_original_outcome_without_synthetic_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let observation = fixture()?;
        let unknown = WeixinSendOutcome::Unknown {
            client_id: "LOCAL_CLIENT_SECRET".into(),
            reason: "original fixed reason".into(),
        };
        let view = observation
            .error_view(&unknown)
            .ok_or("missing unknown view")?;
        assert_eq!(view.get("http_status"), Some(&Value::Null));
        assert_eq!(view.get("body_captured"), Some(&json!(false)));
        assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
        assert!(
            observation
                .error_view(&WeixinSendOutcome::Accepted {
                    client_id: "LOCAL_CLIENT_SECRET".into(),
                    message_id: None
                })
                .is_none()
        );
        Ok(())
    }
}
