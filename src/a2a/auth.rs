//! A2A JWT authentication middleware
//!
//! Provides JWT Bearer Token verification for A2A HTTP endpoints.
//!
//! # Usage
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

// ── JWT config ────────────────────────────────────────────────────────────────

/// JWT authentication configuration
#[derive(Clone)]
pub struct JwtConfig {
    /// JWT signing secret (for HMAC symmetric algorithms)
    secret: Arc<str>,
    /// Allowed signing algorithms (default HS256)
    algorithms: Vec<Algorithm>,
    /// Issuer validation (None means no validation)
    issuer: Option<String>,
    /// Audience validation (None means no validation)
    audience: Option<String>,
    /// Whether auth is enabled (when false, all validation is skipped)
    enabled: bool,
}

impl JwtConfig {
    /// Create configuration with HS256 symmetric key
    ///
    /// # Parameters
    ///
    /// * `secret` — Signing key (at least 32 characters, 64+ recommended)
    pub fn hs256(secret: impl Into<String>) -> Self {
        Self {
            secret: Arc::from(secret.into().as_str()),
            algorithms: vec![Algorithm::HS256],
            issuer: None,
            audience: None,
            enabled: true,
        }
    }

    /// Create configuration with RS256 public key
    ///
    /// # Parameters
    ///
    /// * `public_key` — PEM-format RSA/EC public key
    pub fn rs256(public_key: impl Into<String>) -> Self {
        Self {
            secret: Arc::from(public_key.into().as_str()),
            algorithms: vec![Algorithm::RS256],
            issuer: None,
            audience: None,
            enabled: true,
        }
    }

    /// Set issuer validation
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Set audience validation
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Set allowed signing algorithms
    pub fn with_algorithms(mut self, algorithms: Vec<Algorithm>) -> Self {
        self.algorithms = algorithms;
        self
    }

    /// Disable authentication (allow all requests through)
    pub fn disabled() -> Self {
        Self {
            secret: Arc::from(""),
            algorithms: vec![],
            issuer: None,
            audience: None,
            enabled: false,
        }
    }

    /// Whether authentication is enabled
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

/// JWT Claims (standard fields + custom extensions)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Issuer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// Subject (typically user/client ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// Audience
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// Expiration time (Unix timestamp)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<usize>,
    /// Not before time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<usize>,
    /// Issued at time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<usize>,
    /// Token unique ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Custom fields (all other claims)
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl JwtClaims {
    /// Get a reference to the subject (sub)
    pub fn subject(&self) -> Option<&str> {
        self.sub.as_deref()
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

/// JWT authentication middleware
///
/// Extracts and verifies JWT from the `Authorization: Bearer <token>` header.
/// On successful verification, claims are injected into the request extensions,
/// and downstream handlers can extract them via [`JwtClaims`].
///
/// # Error responses
///
/// - 401 `{"error": "missing Authorization header"}` — Missing Authorization header
/// - 401 `{"error": "invalid token"}` — Token invalid or expired
pub async fn jwt_middleware(
    State(config): State<Arc<JwtConfig>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // When auth is disabled, pass through directly
    if !config.enabled {
        return next.run(req).await;
    }

    // Extract Bearer token
    let token = match extract_bearer_token(req.headers()) {
        Some(t) => t,
        None => {
            return unauthorized_response("missing Authorization header");
        }
    };

    // Verify JWT
    match validate_token(&config, token) {
        Ok(claims) => {
            // Inject claims for use by downstream handlers
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "JWT validation failed");
            unauthorized_response("invalid token")
        }
    }
}

/// Extract Bearer token from request headers
fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let header_value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    header_value.strip_prefix("Bearer ")
}

/// Validate JWT and return Claims
fn validate_token(config: &JwtConfig, token: &str) -> Result<JwtClaims, String> {
    let mut validation = Validation::new(
        config
            .algorithms
            .first()
            .copied()
            .unwrap_or(Algorithm::HS256),
    );

    // Allow common algorithms
    if config.algorithms.len() > 1 {
        validation.algorithms = config.algorithms.clone();
    }

    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(&[issuer.as_str()]);
    }
    if let Some(ref audience) = config.audience {
        validation.set_audience(&[audience.as_str()]);
    }

    // Set reasonable validation options
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = 30; // 30 second clock skew tolerance

    let decoding_key = DecodingKey::from_secret(config.secret.as_bytes());
    let token_data = decode::<JwtClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT validation error: {e}"))?;

    Ok(token_data.claims)
}

/// 401 Unauthorized response
fn unauthorized_response(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Extract JWT Claims from request extensions (injected after successful validation)
///
/// # Usage example
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
