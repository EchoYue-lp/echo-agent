use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::error::{ReactError, Result};

/// HTTP Webhook 人工介入 Provider。
///
/// 向配置的 URL 发送 POST 请求，等待响应中返回的决策结果。
/// 适合与企业审批系统、Slack Bot、钉钉机器人等外部平台集成。
///
/// # 协议
///
/// **请求** POST body（`kind` 字段区分场景）：
/// ```json
/// {
///   "kind": "approval",
///   "prompt": "工具 [xxx] 需要人工审批...",
///   "tool_name": "xxx",
///   "args": { ... }
/// }
/// ```
/// 或：
/// ```json
/// {
///   "kind": "input",
///   "prompt": "请补充你的意图..."
/// }
/// ```
///
/// **响应**（统一格式）：
/// ```json
/// {
///   "decision": "approved" | "rejected" | "timeout",
///   "text": "用户输入的文本（input 场景）",
///   "reason": "可选说明"
/// }
/// ```
pub struct WebhookHumanLoopProvider {
    client: Arc<Client>,
    url: String,
    timeout: Duration,
}

impl WebhookHumanLoopProvider {
    /// 创建 Webhook Provider，使用默认超时（5 分钟）。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Client::new()),
            url: url.into(),
            timeout: Duration::from_secs(300),
        }
    }

    /// 自定义超时时长。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// 发往 Webhook 的请求体（`kind` 字段告知对端场景类型）。
#[derive(Serialize)]
struct WebhookPayload<'a> {
    kind: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<&'a serde_json::Value>,
}

/// Webhook 统一响应体。
#[derive(Deserialize)]
struct WebhookResponse {
    /// approval 场景：`"approved"` | `"rejected"` | `"timeout"`
    decision: Option<String>,
    /// input 场景：用户输入的文本
    text: Option<String>,
    reason: Option<String>,
}

#[async_trait]
impl HumanLoopProvider for WebhookHumanLoopProvider {
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse> {
        let kind_str = match req.kind {
            HumanLoopKind::Approval => "approval",
            HumanLoopKind::Input => "input",
        };

        let payload = WebhookPayload {
            kind: kind_str,
            prompt: &req.prompt,
            tool_name: req.tool_name.as_deref(),
            args: req.args.as_ref(),
        };

        let resp = self
            .client
            .post(&self.url)
            .timeout(self.timeout)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ReactError::Other(format!("Webhook 请求失败: {e}")))?;

        if !resp.status().is_success() {
            return Err(ReactError::Other(format!(
                "Webhook 返回非成功状态码: {}",
                resp.status()
            )));
        }

        let response: WebhookResponse = resp
            .json()
            .await
            .map_err(|e| ReactError::Other(format!("Webhook 响应解析失败: {e}")))?;

        match req.kind {
            HumanLoopKind::Approval => match response.decision.as_deref() {
                Some("approved") => Ok(HumanLoopResponse::Approved),
                Some("rejected") => Ok(HumanLoopResponse::Rejected {
                    reason: response.reason,
                }),
                Some("timeout") | None => Ok(HumanLoopResponse::Timeout),
                Some(other) => Err(ReactError::Other(format!("未知的审批决策值: {other}"))),
            },
            HumanLoopKind::Input => match response.text {
                Some(text) => Ok(HumanLoopResponse::Text(text)),
                None => Ok(HumanLoopResponse::Timeout),
            },
        }
    }
}
