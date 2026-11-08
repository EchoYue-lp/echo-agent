//! Provider authentication values. Login-operation ownership belongs to the host.

use echo_core::error::{ChannelError, ReactError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;

pub const INITIAL_API_BASE_URL: &str = "https://ilinkai.weixin.qq.com";

/// The generation is created on credential issuance and persisted with the token.
/// Reconstructing a transport after a restart must preserve this generation.
#[derive(Clone, Serialize, Deserialize)]
pub struct WeixinCredentials {
    pub account_id: String,
    pub bot_token: String,
    pub base_url: String,
    pub user_id: String,
    pub generation: String,
}

impl fmt::Debug for WeixinCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeixinCredentials")
            .field("account_id", &"[redacted]")
            .field("bot_token", &"[redacted]")
            .field("base_url", &"[redacted]")
            .field("user_id", &"[redacted]")
            .field("generation", &self.generation)
            .finish()
    }
}

impl WeixinCredentials {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.account_id)?;
        validate_identifier(&self.user_id)?;
        validate_identifier(&self.generation)?;
        if self.bot_token.trim().is_empty()
            || self.bot_token.chars().any(char::is_control)
            || reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.bot_token))
                .is_err()
        {
            return Err(protocol_error(
                "Weixin credentials contain an invalid token",
            ));
        }
        validated_origin(&self.base_url)?;
        Ok(())
    }
}

/// QR content is intentionally exposed only through the dedicated host auth flow.
#[derive(Clone, Serialize, Deserialize)]
pub struct QrChallenge {
    pub qrcode: String,
    pub qrcode_img_content: String,
    pub polling_base_url: String,
}

impl fmt::Debug for QrChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QrChallenge")
            .field("qrcode", &"[redacted]")
            .field("qrcode_img_content", &"[redacted]")
            .field("polling_base_url", &"[redacted]")
            .finish()
    }
}

/// These are protocol observations, not claims that the host saved credentials or
/// that an authenticated transport is online. AlreadyBound contains no new token.
#[derive(Clone, Debug)]
pub enum QrStatus {
    Wait,
    Scanned,
    Confirmed(WeixinCredentials),
    Expired,
    NeedVerifyCode,
    VerifyCodeBlocked,
    Redirect { base_url: String },
    AlreadyBound,
}

pub(crate) fn protocol_error(message: &str) -> ReactError {
    ReactError::Channel(Box::new(ChannelError::Other(message.to_owned())))
}

pub(crate) fn validate_identifier(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(protocol_error(
            "Weixin protocol identity is missing or invalid",
        ));
    }
    Ok(())
}

/// A base/redirect is an origin, never an arbitrary URL or an HTTP redirect.
/// Locally this protects the user's token from accidental disclosure through
/// URL credentials, cleartext transport, or cross-origin redirect following.
pub(crate) fn validated_origin(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|_| protocol_error("Weixin API origin is not a valid HTTPS origin"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return Err(protocol_error(
            "Weixin API origin is not a valid HTTPS origin",
        ));
    }
    Ok(url)
}

#[derive(Deserialize)]
pub(crate) struct QrResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

#[derive(Deserialize)]
pub(crate) struct QrStatusResponse {
    status: String,
    bot_token: Option<String>,
    ilink_bot_id: Option<String>,
    baseurl: Option<String>,
    ilink_user_id: Option<String>,
    redirect_host: Option<String>,
}

impl QrStatusResponse {
    pub(crate) fn into_status(self) -> Result<QrStatus> {
        match self.status.as_str() {
            "wait" => Ok(QrStatus::Wait),
            "scaned" => Ok(QrStatus::Scanned),
            "expired" => Ok(QrStatus::Expired),
            "need_verifycode" => Ok(QrStatus::NeedVerifyCode),
            "verify_code_blocked" => Ok(QrStatus::VerifyCodeBlocked),
            "binded_redirect" => Ok(QrStatus::AlreadyBound),
            "scaned_but_redirect" => {
                let host = self.redirect_host.ok_or_else(|| {
                    protocol_error("Weixin QR redirect response is missing its host")
                })?;
                // redirect_host is a host/optional port, not an alternate URL.
                if host.chars().any(|ch| matches!(ch, '/' | '?' | '#' | '@')) {
                    return Err(protocol_error("Weixin QR redirect host is invalid"));
                }
                let origin = validated_origin(&format!("https://{host}"))?;
                Ok(QrStatus::Redirect {
                    base_url: origin.origin().ascii_serialization(),
                })
            }
            "confirmed" => {
                let missing =
                    || protocol_error("Weixin confirmed response lacks required identity");
                let credentials = WeixinCredentials {
                    account_id: self.ilink_bot_id.ok_or_else(missing)?,
                    bot_token: self.bot_token.ok_or_else(missing)?,
                    base_url: self.baseurl.ok_or_else(missing)?,
                    user_id: self.ilink_user_id.ok_or_else(missing)?,
                    generation: uuid::Uuid::new_v4().to_string(),
                };
                credentials.validate()?;
                Ok(QrStatus::Confirmed(credentials))
            }
            _ => Err(protocol_error("Weixin QR response has an unknown status")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_requires_complete_identity_and_debug_does_not_expose_secrets() -> Result<()> {
        let incomplete: QrStatusResponse =
            serde_json::from_str(r#"{"status":"confirmed","ilink_bot_id":"account"}"#)
                .map_err(|_| protocol_error("fixture decode failed"))?;
        assert!(incomplete.into_status().is_err());
        let complete: QrStatusResponse = serde_json::from_str(
            r#"{"status":"confirmed","ilink_bot_id":"account","bot_token":"token-secret","ilink_user_id":"person-secret","baseurl":"https://ilinkai.weixin.qq.com"}"#,
        )
        .map_err(|_| protocol_error("fixture decode failed"))?;
        let status = complete.into_status()?;
        assert!(matches!(&status, QrStatus::Confirmed(_)));
        let debug = format!("{status:?}");
        assert!(!debug.contains("token-secret"));
        assert!(!debug.contains("person-secret"));
        Ok(())
    }

    #[test]
    fn qr_states_and_redirect_are_observations_not_fabricated_credentials() -> Result<()> {
        for (state, expected) in [
            ("wait", QrStatus::Wait),
            ("scaned", QrStatus::Scanned),
            ("expired", QrStatus::Expired),
            ("need_verifycode", QrStatus::NeedVerifyCode),
            ("verify_code_blocked", QrStatus::VerifyCodeBlocked),
            ("binded_redirect", QrStatus::AlreadyBound),
        ] {
            let response: QrStatusResponse =
                serde_json::from_value(serde_json::json!({"status":state}))
                    .map_err(|_| protocol_error("fixture decode failed"))?;
            assert_eq!(
                std::mem::discriminant(&response.into_status()?),
                std::mem::discriminant(&expected)
            );
        }
        for host in [
            "evil.invalid/path",
            "evil.invalid@other.invalid",
            "https://other.invalid",
            "other.invalid#fragment",
        ] {
            let response: QrStatusResponse = serde_json::from_value(
                serde_json::json!({"status":"scaned_but_redirect","redirect_host":host}),
            )
            .map_err(|_| protocol_error("fixture decode failed"))?;
            assert!(response.into_status().is_err());
        }
        let response: QrStatusResponse = serde_json::from_value(serde_json::json!({"status":"scaned_but_redirect","redirect_host":"ilinkai.weixin.qq.com"}))
            .map_err(|_| protocol_error("fixture decode failed"))?;
        assert!(matches!(response.into_status()?, QrStatus::Redirect { .. }));
        for origin in [
            "http://example.invalid",
            "https://token@example.invalid",
            "https://example.invalid/?token=secret",
            "https://example.invalid/#secret",
        ] {
            assert!(validated_origin(origin).is_err());
        }
        Ok(())
    }
}
