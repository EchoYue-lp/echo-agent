//! 原 send_text POST → 原错误 → scoped EnvFilter/fmt writer 的回环集成控制。
//! 全部身份、凭据、响应都是合成夹具；不启动 App、不读用户数据、不访问真实微信。

use echo_integration::channels::weixin::{WeixinApi, WeixinCredentials, WeixinSendOutcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// 对应 EKO 现有默认 target filter；此处只验证真实 EnvFilter/fmt 接缝，不冒 App 文件层。
const DEFAULT_FILTER: &str =
    "echo_agent_cli=info,echo_agent=info,echo_agent_app_core=info,tower_http=info";
const ERROR_TARGET: &str = "echo_agent::channels::weixin::send_observation";
const CAPTURE_BYTES: usize = 32 * 1024;
const REQUEST_BYTES: usize = 64 * 1024;
const FIXTURE_DEADLINE: Duration = Duration::from_secs(5);

fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

#[derive(Default)]
struct CaptureState {
    bytes: Vec<u8>,
    overflow: bool,
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<CaptureState>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| failure("capture lock poisoned"))?;
        if state.bytes.len().saturating_add(bytes.len()) > CAPTURE_BYTES {
            state.overflow = true;
            return Err(failure("bounded log capture exceeded"));
        }
        state.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn text(&self) -> TestResult<String> {
        let state = self
            .0
            .lock()
            .map_err(|_| failure("capture lock poisoned"))?;
        if state.overflow {
            return Err(failure("bounded log capture exceeded").into());
        }
        String::from_utf8(state.bytes.clone())
            .map_err(|_| failure("fmt capture was not UTF-8").into())
    }
}

// 不实现 Debug，避免测试断言把合成敏感值的使用习惯扩散成生产日志 pattern。
#[derive(Clone, Copy)]
struct Material {
    token: &'static str,
    peer: &'static str,
    context: &'static str,
    client_id: &'static str,
    text: &'static str,
    route_tag: &'static str,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            token: "TOKEN_SECRET",
            peer: "RECIPIENT_SECRET",
            context: "CONTEXT_SECRET",
            client_id: "LOCAL_CLIENT_SECRET",
            text: "MESSAGE_SECRET",
            route_tag: "HEADER_SECRET",
        }
    }
}

struct Reply {
    status: &'static str,
    body: Vec<u8>,
    request_id: Option<&'static str>,
    declared_extra: usize,
}

impl Reply {
    fn json(value: Value) -> TestResult<Self> {
        Ok(Self {
            status: "200 OK",
            body: serde_json::to_vec(&value)?,
            request_id: None,
            declared_extra: 0,
        })
    }

    fn bytes(&self) -> TestResult<Vec<u8>> {
        // Content-Length 是真实 wire 字节数，不是用户可见字符串截断。
        let length = self
            .body
            .len()
            .checked_add(self.declared_extra)
            .ok_or_else(|| failure("fixture response length overflow"))?;
        let mut headers = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n",
            self.status
        );
        if let Some(value) = self.request_id {
            headers.push_str(&format!("x-request-id: {value}\r\n"));
        }
        headers.push_str("\r\n");
        let mut bytes = headers.into_bytes();
        bytes.extend_from_slice(&self.body);
        Ok(bytes)
    }
}

struct RequestFacts {
    original_post: bool,
    original_fields: bool,
}

// 有界读取一条实际请求，只把布尔事实交回测试，不回显 header/body。
async fn read_request(stream: &mut TcpStream, material: Material) -> TestResult<RequestFacts> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut buffer = [0_u8; 2048];
    let mut boundary: Option<usize> = None;
    let mut body_length: Option<usize> = None;
    loop {
        if let (Some(end), Some(length)) = (boundary, body_length) {
            let total = end
                .checked_add(length)
                .ok_or_else(|| failure("fixture request length overflow"))?;
            if total > REQUEST_BYTES {
                return Err(failure("fixture request exceeded bound").into());
            }
            if bytes.len() >= total {
                break;
            }
        }
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(failure("fixture request ended before complete body").into());
        }
        if bytes.len().saturating_add(count) > REQUEST_BYTES {
            return Err(failure("fixture request exceeded bound").into());
        }
        bytes.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| failure("fixture read count invalid"))?,
        );
        if boundary.is_none()
            && let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
        {
            let end = position
                .checked_add(4)
                .ok_or_else(|| failure("fixture header length overflow"))?;
            let header = std::str::from_utf8(
                bytes
                    .get(..end)
                    .ok_or_else(|| failure("fixture header boundary invalid"))?,
            )?;
            let length = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .ok_or_else(|| failure("fixture request missing Content-Length"))?
                .1
                .trim()
                .parse::<usize>()
                .map_err(|_| failure("fixture request Content-Length invalid"))?;
            boundary = Some(end);
            body_length = Some(length);
        }
    }
    let end = boundary.ok_or_else(|| failure("fixture header not observed"))?;
    let total = end
        .checked_add(body_length.ok_or_else(|| failure("fixture body length not observed"))?)
        .ok_or_else(|| failure("fixture request length overflow"))?;
    let header = std::str::from_utf8(
        bytes
            .get(..end)
            .ok_or_else(|| failure("fixture header boundary invalid"))?,
    )?;
    let header_value = |name: &str| {
        header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    };
    let body: Value = serde_json::from_slice(
        bytes
            .get(end..total)
            .ok_or_else(|| failure("fixture body boundary invalid"))?,
    )?;
    let authorization = format!("Bearer {}", material.token);
    Ok(RequestFacts {
        original_post: header.lines().next() == Some("POST /ilink/bot/sendmessage HTTP/1.1"),
        original_fields: header_value("authorization") == Some(authorization.as_str())
            && header_value("skroutetag") == Some(material.route_tag)
            && body.pointer("/msg/to_user_id").and_then(Value::as_str) == Some(material.peer)
            && body.pointer("/msg/context_token").and_then(Value::as_str) == Some(material.context)
            && body.pointer("/msg/client_id").and_then(Value::as_str) == Some(material.client_id)
            && body
                .pointer("/msg/item_list/0/text_item/text")
                .and_then(Value::as_str)
                == Some(material.text),
    })
}

#[derive(Default)]
struct ServerReceipt {
    requests: usize,
    original_posts: usize,
    original_fields: usize,
}

async fn server(
    listener: TcpListener,
    response: Vec<u8>,
    material: Material,
    mut stop: oneshot::Receiver<()>,
) -> TestResult<ServerReceipt> {
    let mut receipt = ServerReceipt::default();
    loop {
        // 已ready的额外请求先计数，不能由client返回后的stop掩掉重复连接。
        let accepted = tokio::select! {
            biased;
            accepted = listener.accept() => Some(accepted),
            _ = &mut stop => None,
        };
        let Some(accepted) = accepted else {
            return Ok(receipt);
        };
        let (mut stream, _) = accepted?;
        receipt.requests = receipt.requests.saturating_add(1);
        if receipt.requests > 8 {
            return Err(failure("fixture observed excessive requests").into());
        }
        let facts = tokio::time::timeout(FIXTURE_DEADLINE, read_request(&mut stream, material))
            .await
            .map_err(|_| failure("fixture request read timed out"))??;
        if facts.original_post {
            receipt.original_posts = receipt.original_posts.saturating_add(1);
        }
        if facts.original_fields {
            receipt.original_fields = receipt.original_fields.saturating_add(1);
        }
        stream.write_all(&response).await?;
        stream.shutdown().await?;
    }
}

async fn join_server(
    mut handle: JoinHandle<TestResult<ServerReceipt>>,
) -> TestResult<ServerReceipt> {
    match tokio::time::timeout(FIXTURE_DEADLINE, &mut handle).await {
        Ok(joined) => joined.map_err(|_| failure("fixture server join failed"))?,
        Err(_) => {
            handle.abort();
            // 真正await原句柄；abort请求不等于已结算。任何出口都保留测试失败。
            match handle.await {
                Ok(_) => {}
                Err(error) if error.is_cancelled() => {}
                Err(_) => return Err(failure("fixture server cleanup join failed").into()),
            }
            Err(failure("fixture server join timed out").into())
        }
    }
}

struct Observation {
    outcome: WeixinSendOutcome,
    log: String,
    receipt: ServerReceipt,
}

async fn run_case(filter: &str, material: Material, reply: Reply) -> TestResult<Observation> {
    // fallible准备先于spawn，避免测试夹具在准备错误时留下detached server。
    let response = reply.bytes()?;
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_new(filter)?)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_target(true)
                .with_writer(move || writer.clone()),
        );
    let dispatch = tracing::Dispatch::new(subscriber);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let api = WeixinApi::for_local_auth_fixture(listener.local_addr()?)?
        .with_route_tag(material.route_tag)?;
    let credentials = WeixinCredentials {
        account_id: "fixture-account".into(),
        generation: "fixture-generation".into(),
        base_url: "https://example.invalid".into(),
        user_id: material.peer.into(),
        bot_token: material.token.into(),
    };
    let (stop, stopped) = oneshot::channel();
    let handle = tokio::spawn(server(listener, response, material, stopped));
    let result = tokio::time::timeout(
        FIXTURE_DEADLINE,
        api.send_text(
            &credentials,
            material.peer,
            Some(material.context),
            material.client_id,
            material.text,
        )
        .with_subscriber(dispatch),
    )
    .await;
    let _ = stop.send(());
    let receipt = join_server(handle).await?;
    let outcome = result.map_err(|_| failure("original sender fixture timed out"))??;
    assert_eq!(receipt.requests, 1, "actual HTTP request count");
    assert_eq!(
        receipt.original_posts, 1,
        "original send_text POST must reach server"
    );
    assert_eq!(
        receipt.original_fields, 1,
        "original private fields must reach server intact"
    );
    Ok(Observation {
        outcome,
        log: capture.text()?,
        receipt,
    })
}

fn diagnostic(log: &str) -> TestResult<Value> {
    let mut lines = log
        .lines()
        .filter(|line| line.contains("Weixin original send error observation"));
    let line = lines
        .next()
        .ok_or_else(|| failure("original producer trace not captured"))?;
    if lines.next().is_some() {
        return Err(failure("original producer emitted duplicate error traces").into());
    }
    assert!(line.contains(ERROR_TARGET));
    let (_, encoded) = line
        .split_once("diagnostic=")
        .ok_or_else(|| failure("fmt diagnostic field unavailable"))?;
    serde_json::Deserializer::from_str(encoded)
        .into_iter::<Value>()
        .next()
        .ok_or_else(|| failure("fmt diagnostic JSON unavailable"))?
        .map_err(Into::into)
}

fn assert_private_absent(log: &str, material: Material) {
    for secret in [
        material.token,
        material.peer,
        material.context,
        material.client_id,
        material.text,
        material.route_tag,
    ] {
        assert!(
            !log.contains(secret),
            "original private value escaped into actual fmt writer"
        );
    }
    assert!(
        !log.contains("Bearer "),
        "Authorization escaped into actual fmt writer"
    );
    assert!(
        !log.contains("example.invalid"),
        "request origin escaped into actual fmt writer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn original_post_error_reaches_default_env_filter_and_fmt_writer() -> TestResult {
    let material = Material::default();
    let mut reply = Reply::json(json!({
        "ret":-2,"errcode":17,"errmsg":"invalid parameter",
        "request_id":"fixture-response-body-id","unknown_body":"UNRECOGNIZED_BODY_SECRET"
    }))?;
    reply.request_id = Some("fixture-response-header-id");
    let observed = run_case(DEFAULT_FILTER, material, reply).await?;
    assert!(matches!(
        observed.outcome,
        WeixinSendOutcome::Rejected { code: -2 }
    ));
    let view = diagnostic(&observed.log)?;
    assert_eq!(view.get("http_status"), Some(&json!(200)));
    assert_eq!(view.pointer("/ret/code"), Some(&json!(-2)));
    assert_eq!(view.pointer("/errcode/code"), Some(&json!(17)));
    assert_eq!(
        view.pointer("/errmsg/public_text"),
        Some(&json!("invalid parameter"))
    );
    assert_eq!(
        view.get("local_client_id_sha256"),
        Some(&json!(format!(
            "{:x}",
            Sha256::digest(material.client_id.as_bytes())
        )))
    );
    let ids = view
        .get("observed_response_request_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| failure("observed response IDs unavailable"))?;
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().any(
        |id| id.get("source") == Some(&json!("response_header:x-request-id"))
            && id.get("value") == Some(&json!("fixture-response-header-id"))
    ));
    assert!(ids.iter().any(
        |id| id.get("source") == Some(&json!("response_body:request_id"))
            && id.get("value") == Some(&json!("fixture-response-body-id"))
    ));
    assert!(!observed.log.contains("UNRECOGNIZED_BODY_SECRET"));
    assert_private_absent(&observed.log, material);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn accepted_original_post_emits_no_error_trace() -> TestResult {
    let observed = run_case(
        DEFAULT_FILTER,
        Material::default(),
        Reply::json(json!({"ret":0,"message_id":"fixture-accepted-message"}))?,
    )
    .await?;
    assert!(matches!(
        observed.outcome,
        WeixinSendOutcome::Accepted { .. }
    ));
    assert_eq!(observed.receipt.requests, 1);
    assert!(
        observed.log.is_empty(),
        "accepted send must not emit error diagnostics"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn strict_filter_hides_warn_but_original_http_still_occurs() -> TestResult {
    let observed = run_case(
        "echo_agent=error",
        Material::default(),
        Reply::json(json!({"ret":-2,"errmsg":"invalid parameter"}))?,
    )
    .await?;
    assert!(matches!(
        observed.outcome,
        WeixinSendOutcome::Rejected { code: -2 }
    ));
    assert_eq!(
        observed.receipt.requests, 1,
        "empty capture is not evidence of no HTTP"
    );
    assert!(observed.log.is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn original_non2xx_truncated_and_unknown_receipts_keep_their_first_error() -> TestResult {
    let material = Material::default();
    let mut non2xx = Reply::json(json!({"ret":0,"errmsg":"NON2XX_BODY_SECRET"}))?;
    non2xx.status = "503 Unavailable";
    non2xx.request_id = Some("fixture-http-error-id");
    let observed = run_case(DEFAULT_FILTER, material, non2xx).await?;
    assert!(matches!(
        observed.outcome,
        WeixinSendOutcome::Unknown { .. }
    ));
    let view = diagnostic(&observed.log)?;
    assert_eq!(view.get("stage"), Some(&json!("http_non_success")));
    assert_eq!(view.get("http_status"), Some(&json!(503)));
    assert_eq!(view.get("body_captured"), Some(&json!(false)));
    assert!(view.get("ret").is_none());
    assert!(!observed.log.contains("NON2XX_BODY_SECRET"));

    let truncated = Reply {
        status: "200 OK",
        body: br#"{"ret":-2"#.to_vec(),
        request_id: None,
        declared_extra: 17,
    };
    let observed = run_case(DEFAULT_FILTER, material, truncated).await?;
    assert!(matches!(
        observed.outcome,
        WeixinSendOutcome::Unknown { .. }
    ));
    let view = diagnostic(&observed.log)?;
    assert_eq!(view.get("stage"), Some(&json!("body_interrupted")));
    assert_eq!(view.get("body_captured"), Some(&json!(false)));

    for body in [b"{}".to_vec(), b"UNPARSED_RESPONSE_SECRET".to_vec()] {
        let observed = run_case(
            DEFAULT_FILTER,
            material,
            Reply {
                status: "200 OK",
                body,
                request_id: None,
                declared_extra: 0,
            },
        )
        .await?;
        assert!(matches!(
            observed.outcome,
            WeixinSendOutcome::Unknown { .. }
        ));
        let view = diagnostic(&observed.log)?;
        assert_eq!(view.get("http_status"), Some(&json!(200)));
        assert_eq!(view.get("observed_response_request_ids"), Some(&json!([])));
        assert!(!observed.log.contains("UNPARSED_RESPONSE_SECRET"));
        assert_private_absent(&observed.log, material);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_writer_rejects_private_body_auth_headers_recipient_context_and_client_echoes()
-> TestResult {
    let material = Material::default();
    for echoed in [
        material.token,
        material.peer,
        material.context,
        material.client_id,
        material.text,
        material.route_tag,
    ] {
        let mut reply = Reply::json(json!({"ret":-2,"errmsg":echoed,"request_id":echoed}))?;
        reply.request_id = Some(echoed);
        let observed = run_case(DEFAULT_FILTER, material, reply).await?;
        assert!(matches!(
            observed.outcome,
            WeixinSendOutcome::Rejected { code: -2 }
        ));
        let view = diagnostic(&observed.log)?;
        assert!(view.pointer("/errmsg/public_text").is_none());
        assert!(
            view.get("observed_response_request_ids")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty),
            "private response id escaped into actual writer"
        );
        assert_private_absent(&observed.log, material);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_writer_suppresses_utf8_header_auth_and_body_public_literal_echoes() -> TestResult {
    let original = Material::default();
    for material in [
        Material {
            route_tag: "参数错误",
            ..original
        },
        Material {
            token: "参数错误",
            ..original
        },
        Material {
            text: "参数错误",
            ..original
        },
    ] {
        let observed = run_case(
            DEFAULT_FILTER,
            material,
            Reply::json(json!({"ret":-2,"errmsg":"参数错误","request_id":"参数错误"}))?,
        )
        .await?;
        assert!(matches!(
            observed.outcome,
            WeixinSendOutcome::Rejected { code: -2 }
        ));
        let view = diagnostic(&observed.log)?;
        assert!(view.pointer("/errmsg/public_text").is_none());
        assert!(
            view.get("observed_response_request_ids")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty),
            "private response id escaped into actual writer"
        );
        assert_private_absent(&observed.log, material);
    }
    Ok(())
}
