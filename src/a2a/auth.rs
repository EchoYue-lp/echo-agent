//! A2A JWT 鉴权中间件
//!
//! 为 A2A HTTP 端点提供 JWT Bearer Token 验证。
//!
//! # 使用方式
//!
//! ```rust,no_run
//! use echo_agent::a2a::{A2AServer, AgentCard, serve_with_auth, JwtConfig};
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let card = AgentCard::builder("my-agent", "http://localhost:3000").build();
//! let agent = ReactAgentBuilder::simple("qwen3-max", "test")?;
//! let server = A2AServer::new(card, agent);
//!
//! let jwt_config = JwtConfig::hs256("my-secret-key")
//!     .with_issuer("echo-agent")
//!     .with_audience("a2a-clients");
//!
//! serve_with_auth(server, "0.0.0.0:3000", jwt_config).await?;
//! # Ok(())
//! # }
//! ```

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── JWT 配置 ───────────────────────────────────────────────────────────────────

/// JWT 鉴权配置
#[derive(Clone)]
pub struct JwtConfig {
    /// JWT 签名密钥（用于 HMAC 对称算法）
    secret: Arc<str>,
    /// 允许的签名算法（默认 HS256）
    algorithms: Vec<Algorithm>,
    /// 发行人验证（None 表示不验证）
    issuer: Option<String>,
    /// 受众验证（None 表示不验证）
    audience: Option<String>,
    /// 是否启用鉴权（false 时跳过所有验证）
    enabled: bool,
}

impl JwtConfig {
    /// 使用 HS256 对称密钥创建配置
    ///
    /// # 参数
    ///
    /// * `secret` — 签名密钥（至少 32 字符，推荐 64 字符以上）
    pub fn hs256(secret: impl Into<String>) -> Self {
        Self {
            secret: Arc::from(secret.into().as_str()),
            algorithms: vec![Algorithm::HS256],
            issuer: None,
            audience: None,
            enabled: true,
        }
    }

    /// 使用 RS256 公钥创建配置
    ///
    /// # 参数
    ///
    /// * `public_key` — PEM 格式的 RSA/EC 公钥
    pub fn rs256(public_key: impl Into<String>) -> Self {
        Self {
            secret: Arc::from(public_key.into().as_str()),
            algorithms: vec![Algorithm::RS256],
            issuer: None,
            audience: None,
            enabled: true,
        }
    }

    /// 设置发行人验证
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// 设置受众验证
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// 设置允许的签名算法
    pub fn with_algorithms(mut self, algorithms: Vec<Algorithm>) -> Self {
        self.algorithms = algorithms;
        self
    }

    /// 关闭鉴权（所有请求直接放行）
    pub fn disabled() -> Self {
        Self {
            secret: Arc::from(""),
            algorithms: vec![],
            issuer: None,
            audience: None,
            enabled: false,
        }
    }

    /// 是否已启用鉴权
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl std::fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtConfig")
            .field("enabled", &self.enabled)
            .field("algorithms", &self.algorithms)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("secret", &"[redacted]")
            .finish()
    }
}

// ── JWT Claims ─────────────────────────────────────────────────────────────────

/// JWT Claims（标准字段 + 自定义扩展）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// 签发者
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// 主题（通常为用户/客户端 ID）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// 受众
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// 过期时间（Unix 时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<usize>,
    /// 生效时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<usize>,
    /// 签发时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<usize>,
    /// 令牌唯一 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// 自定义字段（其他所有 claim）
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl JwtClaims {
    /// 获取主题（sub）的引用
    pub fn subject(&self) -> Option<&str> {
        self.sub.as_deref()
    }
}

// ── 中间件 ─────────────────────────────────────────────────────────────────────

/// JWT 鉴权中间件
///
/// 从 `Authorization: Bearer <token>` 请求头提取并验证 JWT。
/// 验证通过后将 claims 注入到请求扩展中，后续处理器可通过 [`JwtClaims`] 提取。
///
/// # 错误响应
///
/// - 401 `{"error": "missing Authorization header"}` — 缺少 Authorization 头
/// - 401 `{"error": "invalid token"}` — Token 无效或已过期
pub async fn jwt_middleware(
    State(config): State<Arc<JwtConfig>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // 未启用鉴权时直接放行
    if !config.enabled {
        return next.run(req).await;
    }

    // 提取 Bearer token
    let token = match extract_bearer_token(req.headers()) {
        Some(t) => t,
        None => {
            return unauthorized_response("missing Authorization header");
        }
    };

    // 验证 JWT
    match validate_token(&config, token) {
        Ok(claims) => {
            // 注入 claims 供下游处理器使用
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "JWT validation failed");
            unauthorized_response("invalid token")
        }
    }
}

/// 从请求头提取 Bearer token
fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let header_value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    header_value.strip_prefix("Bearer ")
}

/// 验证 JWT 并返回 Claims
fn validate_token(config: &JwtConfig, token: &str) -> Result<JwtClaims, String> {
    let mut validation = Validation::new(
        config
            .algorithms
            .first()
            .copied()
            .unwrap_or(Algorithm::HS256),
    );

    // 允许常用算法
    if config.algorithms.len() > 1 {
        validation.algorithms = config.algorithms.clone();
    }

    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(&[issuer.as_str()]);
    }
    if let Some(ref audience) = config.audience {
        validation.set_audience(&[audience.as_str()]);
    }

    // 设置一些合理的验证选项
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = 30; // 30 秒时钟偏差容忍

    let decoding_key = DecodingKey::from_secret(config.secret.as_bytes());
    let token_data = decode::<JwtClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT validation error: {e}"))?;

    Ok(token_data.claims)
}

/// 401 未授权响应
fn unauthorized_response(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}

// ── 辅助函数 ───────────────────────────────────────────────────────────────────

/// 从请求扩展中提取 JWT Claims（验证通过后注入）
///
/// # 使用示例
///
/// ```rust,ignore
/// async fn my_handler(Extension(claims): Extension<JwtClaims>) -> impl IntoResponse {
///     format!("Hello, {}", claims.subject().unwrap_or("unknown"))
/// }
/// ```
pub fn get_claims<B>(req: &Request<B>) -> Option<&JwtClaims>
where
    B: std::fmt::Debug + Send + Sync + 'static,
{
    req.extensions().get::<JwtClaims>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bearer_token_valid() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer my.jwt.token".parse().unwrap(),
        );
        assert_eq!(extract_bearer_token(&headers), Some("my.jwt.token"));
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let headers = axum::http::HeaderMap::new();
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn test_extract_bearer_token_not_bearer() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn test_jwt_config_disabled() {
        let config = JwtConfig::disabled();
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_jwt_config_debug_redacts_secret() {
        let config = JwtConfig::hs256("super-secret-key-1234567890");
        let debug_str = format!("{:?}", config);
        assert!(!debug_str.contains("super-secret"));
        assert!(debug_str.contains("[redacted]"));
    }

    #[test]
    fn test_jwt_claims_subject() {
        let claims = JwtClaims {
            iss: None,
            sub: Some("client-123".to_string()),
            aud: None,
            exp: None,
            nbf: None,
            iat: None,
            jti: None,
            extra: serde_json::Map::new(),
        };
        assert_eq!(claims.subject(), Some("client-123"));
    }

    #[test]
    fn test_jwt_claims_no_subject() {
        let claims = JwtClaims {
            iss: None,
            sub: None,
            aud: None,
            exp: None,
            nbf: None,
            iat: None,
            jti: None,
            extra: serde_json::Map::new(),
        };
        assert_eq!(claims.subject(), None);
    }
}
