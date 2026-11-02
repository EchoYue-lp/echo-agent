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

mod send_diagnostic;
use send_diagnostic::SendDiagnostic;

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
    // 同原client/clone至多一份脱敏轮廓，不增加poller或改变接受判定。
    poll_observation_recorded: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(any(test, feature = "channel-test-support"))]
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
            poll_observation_recorded: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            #[cfg(any(test, feature = "channel-test-support"))]
            fixture_origin: None,
        })
    }

    /// Explicit local TCP fixture only. Production construction still requires
    /// HTTPS; downstream tests enable the same override through channel-test-support.
    #[cfg(any(test, feature = "channel-test-support"))]
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
            poll_observation_recorded: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            fixture_origin: Some(
                url::Url::parse(&format!("http://{address}/"))
                    .map_err(|_| protocol_error("Weixin fixture origin is invalid"))?,
            ),
        })
    }

    /// 下游认证回归复用原客户端与 QR 解码；仅显式测试 feature 允许回环 HTTP。
    #[cfg(any(test, feature = "channel-test-support"))]
    pub fn for_local_auth_fixture(address: std::net::SocketAddr) -> Result<Self> {
        Self::for_local_poll_fixture(address)
    }

    fn qr_origin(&self, origin: &str) -> Result<url::Url> {
        let origin = validated_origin(origin)?;
        #[cfg(any(test, feature = "channel-test-support"))]
        if let Some(fixture) = self.fixture_origin.as_ref() {
            return Ok(fixture.clone());
        }
        Ok(origin)
    }

    fn qr_status_timeout(&self) -> Duration {
        #[cfg(any(test, feature = "channel-test-support"))]
        if self.fixture_origin.is_some() {
            // 自有回环夹具缩短请求 deadline；生产仍是原 35 秒。
            return Duration::from_millis(50);
        }
        POLL_TIMEOUT
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
        let mut url = self
            .qr_origin(INITIAL_API_BASE_URL)?
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
        let mut url = self
            .qr_origin(&challenge.polling_base_url)?
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
            .timeout(self.qr_status_timeout())
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
        let mut diagnostic = SendDiagnostic::new(&request, client_id);
        let outcome = match self
            .execute_json_observed::<SendResponse>(request, Some(&mut diagnostic))
            .await
        {
            Ok(response) => response.into_outcome(client_id),
            Err(_) => WeixinSendOutcome::Unknown {
                client_id: client_id.to_owned(),
                // Never return reqwest errors here: they can include URLs,
                // query credentials, transport details, or response bodies.
                reason: "Weixin send has no definitive platform receipt".to_owned(),
            },
        };
        if let Some(view) = diagnostic.error_view(&outcome) {
            tracing::warn!(target: "echo_agent::channels::weixin::send_observation", diagnostic = %view,
                "Weixin original send error observation");
        }
        outcome
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
        #[cfg(any(test, feature = "channel-test-support"))]
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
        self.execute_json_observed(request, None).await
    }

    // 同一个 HTTP producer；诊断只旁读原响应，不改第一错误、接受谓词或重试。
    async fn execute_json_observed<T: DeserializeOwned>(
        &self,
        request: reqwest::Request,
        mut diagnostic: Option<&mut SendDiagnostic>,
    ) -> Result<T> {
        let observe_poll = request.method() == reqwest::Method::POST
            && request.url().path() == "/ilink/bot/getupdates";
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(|error| request_error(error, "Weixin HTTPS request failed"))?;
        let http_status = response.status().as_u16();
        if let Some(observation) = diagnostic.as_mut() {
            observation.response(
                http_status,
                response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
            );
        }
        if !response.status().is_success() {
            if let Some(observation) = diagnostic.as_mut() {
                observation.stage("http_non_success");
            }
            if observe_poll {
                self.record_poll_observation(http_status, None);
            }
            return Err(protocol_error(
                "Weixin HTTPS response has a non-success status",
            ));
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    if let Some(observation) = diagnostic.as_mut() {
                        observation.stage("body_interrupted");
                    }
                    return Err(request_error(
                        error,
                        "Weixin HTTPS response was interrupted",
                    ));
                }
            };
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                if let Some(observation) = diagnostic.as_mut() {
                    observation.stage("body_limit_exceeded");
                }
                return Err(protocol_error(
                    "Weixin HTTPS response exceeds the local buffer limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if observe_poll {
            self.record_poll_observation(http_status, Some(&bytes));
        }
        if let Some(observation) = diagnostic.as_mut() {
            observation.body(&bytes);
        }
        match serde_json::from_slice(&bytes) {
            Ok(value) => {
                if let Some(observation) = diagnostic.as_mut() {
                    observation.stage("typed_response_decoded");
                }
                Ok(value)
            }
            Err(_) => {
                if let Some(observation) = diagnostic.as_mut() {
                    observation.stage("invalid_protocol_payload");
                }
                Err(protocol_error(
                    "Weixin HTTPS response has an invalid protocol payload",
                ))
            }
        }
    }
    fn record_poll_observation(&self, http_status: u16, bytes: Option<&[u8]>) {
        if !self
            .poll_observation_recorded
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            tracing::info!(target:"echo::weixin_poll_observation",diagnostic=%poll_observation(http_status,bytes),"Weixin poll protocol shape");
        }
    }
}

// 超时来自 reqwest 的结构化事实；固定文案不保留含 QR/验证码的 URL。
// 复用原 Io/TimedOut，避免为单个协议扩张 Core 公共错误枚举。
fn request_error(error: reqwest::Error, message: &'static str) -> echo_core::error::ReactError {
    if error.is_timeout() {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "微信 HTTPS 请求超时").into()
    } else {
        protocol_error(message)
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
                poll_observation_recorded: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                    false,
                )),
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
    async fn current_send_ack_http_keeps_business_receipt_and_transport_unknown_distinct()
    -> Result<()> {
        for (status, body, expected) in [
            (
                "200 OK",
                r#"{"message_id":"18446744073709551615"}"#,
                "accepted-id",
            ),
            (
                "200 OK",
                r#"{"message_id":18446744073709551615}"#,
                "accepted-id",
            ),
            ("200 OK", r#"{"ret":0}"#, "accepted-empty-id"),
            ("200 OK", r#"{}"#, "unknown"),
            ("200 OK", r#"{"unrelated":true}"#, "unknown"),
            (
                "200 OK",
                r#"{"ret":null,"message_id":"18446744073709551615"}"#,
                "unknown",
            ),
            (
                "200 OK",
                r#"{"errcode":null,"message_id":"18446744073709551615"}"#,
                "unknown",
            ),
            (
                "200 OK",
                r#"{"ret":17,"message_id":"18446744073709551615"}"#,
                "rejected",
            ),
            (
                "200 OK",
                r#"{"ret":0,"errcode":17,"message_id":"18446744073709551615"}"#,
                "rejected",
            ),
            (
                "503 Unavailable",
                r#"{"ret":0,"message_id":"18446744073709551615"}"#,
                "unknown",
            ),
            ("200 OK", r#"[0,null,"18446744073709551615"]"#, "unknown"),
            ("200 OK", r#"{"message_id":true}"#, "unknown"),
            ("200 OK", r#"{"message_id":"#, "unknown"),
        ] {
            let HttpFixture {
                api,
                request,
                server,
            } = local_response(status, body).await?;
            let outcome = api.execute_send(request, "original-client").await;
            server
                .await
                .map_err(|_| protocol_error("send HTTP fixture join failed"))??;
            match expected {
                "accepted-id" => assert_eq!(
                    outcome,
                    WeixinSendOutcome::Accepted {
                        client_id: "original-client".into(),
                        message_id: Some("18446744073709551615".into()),
                    }
                ),
                "accepted-empty-id" => assert_eq!(
                    outcome,
                    WeixinSendOutcome::Accepted {
                        client_id: "original-client".into(),
                        message_id: None,
                    }
                ),
                "rejected" => assert_eq!(outcome, WeixinSendOutcome::Rejected { code: 17 }),
                _ => assert!(
                    matches!(outcome, WeixinSendOutcome::Unknown { client_id, .. } if client_id == "original-client")
                ),
            }
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

    async fn assert_typed_request_timeout(after_headers: bool) -> Result<()> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| protocol_error("夹具监听失败"))?;
        let address = listener
            .local_addr()
            .map_err(|_| protocol_error("夹具地址不可用"))?;
        let (release, released) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|_| protocol_error("夹具接收失败"))?;
            let mut bytes = [0_u8; 4096];
            stream
                .read(&mut bytes)
                .await
                .map_err(|_| protocol_error("夹具读取失败"))?;
            if after_headers {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .map_err(|_| protocol_error("夹具响应头失败"))?;
            }
            let _ = released.await;
            Ok::<(), echo_core::error::ReactError>(())
        });
        let api = WeixinApi::for_local_auth_fixture(address)?;
        let request = api
            .client
            .get(format!("http://{address}/?qrcode=fixture-query-private"))
            .timeout(api.qr_status_timeout())
            .build()
            .map_err(|_| protocol_error("夹具请求构造失败"))?;
        let result = api.execute_json::<Value>(request).await;
        let _ = release.send(());
        server
            .await
            .map_err(|_| protocol_error("夹具服务结算失败"))??;
        let error = result
            .err()
            .ok_or_else(|| protocol_error("超时被误当成功"))?;
        assert!(
            matches!(&error, echo_core::error::ReactError::Io(inner) if inner.kind() == std::io::ErrorKind::TimedOut)
        );
        assert!(!format!("{error:?}").contains("fixture-query-private"));
        assert!(!error.to_string().contains(&address.to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn qr_timeout_before_response_headers_keeps_typed_timeout_and_redacts_url() -> Result<()>
    {
        assert_typed_request_timeout(false).await
    }

    #[tokio::test]
    async fn qr_timeout_during_response_body_keeps_typed_timeout_and_redacts_url() -> Result<()> {
        assert_typed_request_timeout(true).await
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

// 原client的一次响应轮廓：不检查Ready、不遍历msgs、不输出未知键名或任何值正文。
fn poll_observation(http_status: u16, bytes: Option<&[u8]>) -> Value {
    use sha2::{Digest, Sha256};
    let mut observation = json!({"endpoint_kind":"weixin_getupdates","path":"/ilink/bot/getupdates","http_status":http_status,"body_captured":bytes.is_some(),"no_business_acceptance_decision":true});
    let Some(bytes) = bytes else {
        return observation;
    };
    observation["response_bytes"] = json!(bytes.len());
    observation["response_sha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        observation["root_type"] = json!("invalid_json");
        return observation;
    };
    observation["root_type"] = json!(json_kind(&value));
    let Some(object) = value.as_object() else {
        return observation;
    };
    let allowed = [
        "ret",
        "errcode",
        "errmsg",
        "msgs",
        "get_updates_buf",
        "longpolling_timeout_ms",
    ];
    let mut fields = serde_json::Map::new();
    for name in allowed {
        let field = match object.get(name) {
            None => json!({"presence":"absent"}),
            Some(value) => {
                let mut field = json!({"presence":"present","type":json_kind(value)});
                if matches!(name, "ret" | "errcode") {
                    if let Some(code) = value.as_i64() {
                        field["code"] = json!(code);
                    }
                } else if name == "msgs"
                    && let Some(messages) = value.as_array()
                {
                    field["count"] = json!(messages.len());
                }
                field
            }
        };
        fields.insert(name.to_string(), field);
    }
    observation["fields"] = Value::Object(fields);
    observation["unrecognized_top_level_key_count"] = json!(
        object
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .count()
    );
    observation
}
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod bounded_poll_diagnostic_tests {
    use super::*;
    #[test]
    fn diagnostic_has_only_allowlisted_shape_and_no_secrets() {
        let body=br#"{"ret":null,"errcode":0,"errmsg":"secret-message","get_updates_buf":"secret-cursor","msgs":[{"token":"secret-token","text":"secret-body"}],"secret-dynamic-key":"secret-unknown-value"}"#;
        let observed = poll_observation(200, Some(body));
        let text = observed.to_string();
        for secret in [
            "secret-message",
            "secret-cursor",
            "secret-token",
            "secret-body",
            "secret-dynamic-key",
            "secret-unknown-value",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(observed.pointer("/fields/ret/type"), Some(&json!("null")));
        assert_eq!(observed.pointer("/fields/errcode/code"), Some(&json!(0)));
        assert_eq!(observed.pointer("/fields/msgs/count"), Some(&json!(1)));
        assert_eq!(
            observed.get("unrecognized_top_level_key_count"),
            Some(&json!(1))
        );
    }
    #[test]
    fn diagnostic_does_not_normalize_absent_null_or_wrong_code_types() {
        for (body, expected) in [
            (b"{}".as_slice(), "absent"),
            (br#"{"ret":null}"#.as_slice(), "null"),
            (br#"{"ret":"secret"}"#.as_slice(), "string"),
            (br#"{"ret":true}"#.as_slice(), "boolean"),
        ] {
            let observed = poll_observation(200, Some(body));
            let actual = observed
                .pointer("/fields/ret/type")
                .or_else(|| observed.pointer("/fields/ret/presence"));
            assert_eq!(actual, Some(&json!(expected)));
            assert!(observed.pointer("/fields/ret/code").is_none());
        }
    }
    #[test]
    fn non_success_without_captured_body_never_fakes_an_empty_payload() {
        let observed = poll_observation(503, None);
        assert_eq!(observed.get("http_status"), Some(&json!(503)));
        assert_eq!(observed.get("body_captured"), Some(&json!(false)));
        assert!(observed.get("response_sha256").is_none());
    }
    #[test]
    fn first_poll_diagnostic_budget_is_shared_by_client_clones() -> Result<()> {
        let api = WeixinApi::new()?;
        let cloned = api.clone();
        assert!(
            !api.poll_observation_recorded
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        );
        assert!(
            cloned
                .poll_observation_recorded
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        );
        Ok(())
    }
}
