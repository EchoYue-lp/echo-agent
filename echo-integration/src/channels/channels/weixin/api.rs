//! Native JSON/HTTPS adapter for Tencent/openclaw-weixin 874637a0 (2.4.10).
//! No retries are hidden here: an uncertain send remains owned by its caller.

use super::auth::{
    INITIAL_API_BASE_URL, QrChallenge, QrResponse, QrStatus, QrStatusResponse, WeixinCredentials,
    protocol_error, validate_identifier, validated_origin,
};
use super::wire::{SendResponse, UpdatesResponse, WeixinSendOutcome, WeixinUpdates};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use echo_core::error::Result;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;

const PROTOCOL_VERSION: &str = "2.4.10";
// The first-party application version encoding is 0x00MMNNPP.
const APP_CLIENT_VERSION: &str = "132106";
const API_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_TIMEOUT: Duration = Duration::from_secs(35);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct WeixinApi {
    client: reqwest::Client,
    bot_agent: String,
    route_tag: Option<HeaderValue>,
    #[cfg(test)]
    fixture_origin: Option<url::Url>,
}

impl WeixinApi {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            // Redirects are protocol observations handled by the auth owner.
            // Automatic HTTP redirects must never move a local bot credential.
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .timeout(POLL_TIMEOUT)
            .build()
            .map_err(|_| protocol_error("Weixin HTTPS client could not be initialized"))?;
        Ok(Self {
            client,
            bot_agent: format!("echo-agent/{}", env!("CARGO_PKG_VERSION")),
            route_tag: None,
            #[cfg(test)]
            fixture_origin: None,
        })
    }

    /// Local TCP fixture only. Production credentials and production HTTP clients
    /// still require validated HTTPS origins; this endpoint override is not compiled.
    #[cfg(test)]
    pub(crate) fn for_local_poll_fixture(address: std::net::SocketAddr) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err(protocol_error("Weixin fixture must use a loopback address"));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| protocol_error("Weixin fixture client could not be initialized"))?;
        Ok(Self {
            client,
            bot_agent: "test/poll-loop".into(),
            route_tag: None,
            fixture_origin: Some(
                url::Url::parse(&format!("http://{address}/"))
                    .map_err(|_| protocol_error("Weixin fixture origin is invalid"))?,
            ),
        })
    }

    /// The application may declare its own honest observability identifier.
    /// Protocol compatibility version and host identity are separate values.
    pub fn with_bot_agent(mut self, bot_agent: &str) -> Result<Self> {
        if bot_agent.is_empty()
            || bot_agent.chars().count() > 256
            || !bot_agent
                .chars()
                .all(|ch| ch.is_ascii() && !ch.is_ascii_control())
        {
            return Err(protocol_error(
                "Weixin bot agent is not a valid ASCII identity",
            ));
        }
        self.bot_agent = bot_agent.to_owned();
        Ok(self)
    }

    /// Optional routing metadata, never an authorization credential.
    pub fn with_route_tag(mut self, route_tag: &str) -> Result<Self> {
        if route_tag.is_empty() {
            self.route_tag = None;
        } else {
            self.route_tag = Some(
                HeaderValue::from_str(route_tag)
                    .map_err(|_| protocol_error("Weixin route tag is invalid"))?,
            );
        }
        Ok(self)
    }

    pub async fn get_qrcode(&self, local_token_list: &[String]) -> Result<QrChallenge> {
        let tokens = local_token_list.iter().take(10).collect::<Vec<_>>();
        if tokens
            .iter()
            .any(|token| token.trim().is_empty() || token.chars().any(char::is_control))
        {
            return Err(protocol_error("Weixin local credential list is invalid"));
        }
        let mut url = validated_origin(INITIAL_API_BASE_URL)?
            .join("ilink/bot/get_bot_qrcode")
            .map_err(|_| protocol_error("Weixin QR endpoint could not be constructed"))?;
        url.query_pairs_mut().append_pair("bot_type", "3");
        // QR POST uses normal POST headers, with no Bearer and no base_info.
        let request = self
            .client
            .post(url)
            .headers(self.post_headers(None)?)
            .timeout(API_TIMEOUT)
            .json(&json!({"local_token_list": tokens}))
            .build()
            .map_err(|_| protocol_error("Weixin QR request could not be constructed"))?;
        let response: QrResponse = self.execute_json(request).await?;
        validate_identifier(&response.qrcode)?;
        if response.qrcode_img_content.trim().is_empty() {
            return Err(protocol_error("Weixin QR response lacks display content"));
        }
        Ok(QrChallenge {
            qrcode: response.qrcode,
            qrcode_img_content: response.qrcode_img_content,
            polling_base_url: INITIAL_API_BASE_URL.to_owned(),
        })
    }

    pub async fn get_qrcode_status(
        &self,
        challenge: &QrChallenge,
        verify_code: Option<&str>,
    ) -> Result<QrStatus> {
        validate_identifier(&challenge.qrcode)?;
        if let Some(code) = verify_code
            && (code.is_empty() || !code.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Err(protocol_error(
                "Weixin verification code must contain digits",
            ));
        }
        let mut url = validated_origin(&challenge.polling_base_url)?
            .join("ilink/bot/get_qrcode_status")
            .map_err(|_| protocol_error("Weixin QR status endpoint could not be constructed"))?;
        url.query_pairs_mut()
            .append_pair("qrcode", &challenge.qrcode);
        if let Some(code) = verify_code {
            url.query_pairs_mut().append_pair("verify_code", code);
        }
        // GET deliberately has no AuthorizationType, Bearer or X-WECHAT-UIN.
        let request = self
            .client
            .get(url)
            .headers(self.common_headers())
            .timeout(POLL_TIMEOUT)
            .build()
            .map_err(|_| protocol_error("Weixin QR status request could not be constructed"))?;
        let response: QrStatusResponse = self.execute_json(request).await?;
        response.into_status()
    }

    pub async fn poll(
        &self,
        credentials: &WeixinCredentials,
        cursor: &str,
    ) -> Result<WeixinUpdates> {
        credentials.validate()?;
        let request = self.bot_post(
            credentials,
            "ilink/bot/getupdates",
            json!({
                "get_updates_buf": cursor,
                "base_info": self.base_info(),
            }),
            POLL_TIMEOUT,
        )?;
        let response: UpdatesResponse = self.execute_json(request).await?;
        response.into_updates()
    }

    /// Only construction/identity failures return Err. Once execute starts, every
    /// transport/HTTP/decode failure is Unknown, because receipt loss does not
    /// establish that the platform failed to perform the send effect.
    pub async fn send_text(
        &self,
        credentials: &WeixinCredentials,
        peer: &str,
        context: Option<&str>,
        client_id: &str,
        text: &str,
    ) -> Result<WeixinSendOutcome> {
        credentials.validate()?;
        validate_identifier(peer)?;
        validate_identifier(client_id)?;
        if text.trim().is_empty() {
            return Err(protocol_error("Weixin outgoing text is empty"));
        }
        let request = self.bot_post(
            credentials,
            "ilink/bot/sendmessage",
            text_payload(peer, context, client_id, text, self.base_info()),
            API_TIMEOUT,
        )?;
        Ok(self.execute_send(request, client_id).await)
    }

    async fn execute_send(&self, request: reqwest::Request, client_id: &str) -> WeixinSendOutcome {
        match self.execute_json::<SendResponse>(request).await {
            Ok(response) => response.into_outcome(client_id),
            Err(_) => WeixinSendOutcome::Unknown {
                client_id: client_id.to_owned(),
                // Never return reqwest errors here: they can include URLs,
                // query credentials, transport details, or response bodies.
                reason: "Weixin send has no definitive platform receipt".to_owned(),
            },
        }
    }

    fn bot_post(
        &self,
        credentials: &WeixinCredentials,
        endpoint: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<reqwest::Request> {
        let url = validated_origin(&credentials.base_url)?
            .join(endpoint)
            .map_err(|_| protocol_error("Weixin bot endpoint could not be constructed"))?;
        #[cfg(test)]
        let url = match self.fixture_origin.as_ref() {
            Some(origin) => origin
                .join(endpoint)
                .map_err(|_| protocol_error("Weixin fixture endpoint is invalid"))?,
            None => url,
        };
        self.client
            .post(url)
            .headers(self.post_headers(Some(&credentials.bot_token))?)
            .timeout(timeout)
            .json(&payload)
            .build()
            .map_err(|_| protocol_error("Weixin bot request could not be constructed"))
    }

    fn common_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("ilink-app-id", HeaderValue::from_static("bot"));
        headers.insert(
            "ilink-app-clientversion",
            HeaderValue::from_static(APP_CLIENT_VERSION),
        );
        if let Some(tag) = &self.route_tag {
            headers.insert("skroutetag", tag.clone());
        }
        headers
    }

    fn post_headers(&self, token: Option<&str>) -> Result<HeaderMap> {
        let mut headers = self.common_headers();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "authorizationtype",
            HeaderValue::from_static("ilink_bot_token"),
        );
        let uin = STANDARD.encode(rand::random::<u32>().to_string());
        headers.insert(
            "x-wechat-uin",
            HeaderValue::from_str(&uin)
                .map_err(|_| protocol_error("Weixin request metadata could not be constructed"))?,
        );
        if let Some(token) = token {
            let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| protocol_error("Weixin authorization value is invalid"))?;
            authorization.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, authorization);
        }
        Ok(headers)
    }

    fn base_info(&self) -> Value {
        json!({"channel_version":PROTOCOL_VERSION,"bot_agent":self.bot_agent})
    }

    async fn execute_json<T: DeserializeOwned>(&self, request: reqwest::Request) -> Result<T> {
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(|_| protocol_error("Weixin HTTPS request failed or timed out"))?;
        if !response.status().is_success() {
            return Err(protocol_error(
                "Weixin HTTPS response has a non-success status",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| protocol_error("Weixin HTTPS response was interrupted"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(protocol_error(
                    "Weixin HTTPS response exceeds the local buffer limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| protocol_error("Weixin HTTPS response has an invalid protocol payload"))
    }
}

fn text_payload(
    peer: &str,
    context: Option<&str>,
    client_id: &str,
    text: &str,
    base_info: Value,
) -> Value {
    let mut message = json!({
        "from_user_id":"",
        "to_user_id":peer,
        "client_id":client_id,
        "message_type":2,
        "message_state":2,
        "item_list":[{"type":1,"text_item":{"text":text}}],
    });
    if let Some(token) = context.filter(|token| !token.is_empty())
        && let Some(object) = message.as_object_mut()
    {
        object.insert("context_token".to_owned(), Value::String(token.to_owned()));
    }
    json!({"msg":message,"base_info":base_info})
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct HttpFixture {
        api: WeixinApi,
        request: reqwest::Request,
        server: tokio::task::JoinHandle<Result<()>>,
    }

    /// Only the test client accepts local plaintext HTTP. Production origins
    /// and the production client retain their HTTPS validation unchanged.
    async fn local_response(status: &str, body: &str) -> Result<HttpFixture> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| protocol_error("fixture listener failed"))?;
        let address = listener
            .local_addr()
            .map_err(|_| protocol_error("fixture address unavailable"))?;
        // Content-Length describes wire bytes, not a user-visible character cap.
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .map_err(|_| protocol_error("fixture accept timed out"))?
                .map_err(|_| protocol_error("fixture accept failed"))?;
            let mut request_bytes = [0_u8; 4096];
            let _ = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut request_bytes))
                .await
                .map_err(|_| protocol_error("fixture request timed out"))?
                .map_err(|_| protocol_error("fixture request failed"))?;
            stream
                .write_all(response.as_bytes())
                .await
                .map_err(|_| protocol_error("fixture response failed"))?;
            Ok(())
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| protocol_error("fixture HTTP client failed"))?;
        let request = client
            .get(format!("http://{address}/?qrcode=query-secret"))
            .build()
            .map_err(|_| protocol_error("fixture request failed"))?;
        Ok(HttpFixture {
            api: WeixinApi {
                client,
                bot_agent: "test/1".to_owned(),
                route_tag: None,
                fixture_origin: None,
            },
            request,
            server,
        })
    }

    #[tokio::test]
    async fn actual_http_success_does_not_replace_business_acceptance() -> Result<()> {
        for body in [
            r#"{}"#,
            r#"{"ret":-14}"#,
            r#"{"ret":0}"#,
            "payload-secret-not-json",
        ] {
            let HttpFixture {
                api,
                request,
                server,
            } = local_response("200 OK", body).await?;
            let outcome = api.execute_send(request, "stable-id").await;
            server
                .await
                .map_err(|_| protocol_error("fixture server join failed"))??;
            match body {
                r#"{"ret":0}"# => assert!(matches!(outcome, WeixinSendOutcome::Accepted { .. })),
                r#"{"ret":-14}"# => assert_eq!(outcome, WeixinSendOutcome::Rejected { code: -14 }),
                _ => assert!(matches!(outcome, WeixinSendOutcome::Unknown { .. })),
            }
            let debug = format!("{outcome:?}");
            assert!(!debug.contains("payload-secret"));
            assert!(!debug.contains("query-secret"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn http_failure_and_truncated_receipt_remain_unknown() -> Result<()> {
        for (status, body) in [("503 Unavailable", r#"{"ret":0}"#), ("200 OK", "{\"ret\":")] {
            let HttpFixture {
                api,
                request,
                server,
            } = local_response(status, body).await?;
            assert!(matches!(
                api.execute_send(request, "stable-id").await,
                WeixinSendOutcome::Unknown { .. }
            ));
            server
                .await
                .map_err(|_| protocol_error("fixture server join failed"))??;
        }
        Ok(())
    }

    #[test]
    fn qr_get_and_authenticated_post_have_distinct_credentials() -> Result<()> {
        let api = WeixinApi::new()?.with_bot_agent("EKO/0.2.0")?;
        let common = api.common_headers();
        assert!(!common.contains_key("authorization"));
        assert!(!common.contains_key("authorizationtype"));
        assert!(!common.contains_key("x-wechat-uin"));
        let qr_post = api.post_headers(None)?;
        assert!(qr_post.contains_key("authorizationtype"));
        assert!(qr_post.contains_key("x-wechat-uin"));
        assert!(!qr_post.contains_key("authorization"));
        let authenticated = api.post_headers(Some("secret"))?;
        let authorization = authenticated
            .get("authorization")
            .ok_or_else(|| protocol_error("fixture header missing"))?;
        assert!(authorization.is_sensitive());
        let encoded_uin = authenticated
            .get("x-wechat-uin")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| protocol_error("fixture UIN missing"))?;
        let decoded = STANDARD
            .decode(encoded_uin)
            .map_err(|_| protocol_error("fixture UIN decode failed"))?;
        let decimal =
            String::from_utf8(decoded).map_err(|_| protocol_error("fixture UIN UTF-8 invalid"))?;
        assert!(decimal.parse::<u32>().is_ok());
        Ok(())
    }

    #[test]
    fn text_payload_keeps_original_context_and_unicode_without_claiming_a_window() {
        let payload = text_payload(
            "peer",
            Some("request-context"),
            "stable-id",
            "你好😀",
            json!({"bot_agent":"EKO/0.2.0"}),
        );
        assert_eq!(
            payload
                .pointer("/msg/context_token")
                .and_then(Value::as_str),
            Some("request-context")
        );
        assert_eq!(
            payload
                .pointer("/msg/item_list/0/text_item/text")
                .and_then(Value::as_str),
            Some("你好😀")
        );
        assert_eq!(
            payload.pointer("/msg/client_id").and_then(Value::as_str),
            Some("stable-id")
        );
        let no_context = text_payload("peer", None, "stable-id", "hello", json!({}));
        assert!(no_context.pointer("/msg/context_token").is_none());
    }
}
