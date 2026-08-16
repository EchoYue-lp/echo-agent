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
    verification: Option<JwtVerification>,
    /// Issuer validation (None means no validation)
    issuer: Option<String>,
    /// Audience validation (None means no validation)
    audience: Option<String>,
}

#[derive(Clone)]
enum JwtVerification {
    Hs256(DecodingKey),
    Rs256(DecodingKey),
}

impl JwtVerification {
    fn algorithm(&self) -> Algorithm {
        match self {
            Self::Hs256(_) => Algorithm::HS256,
            Self::Rs256(_) => Algorithm::RS256,
        }
    }

    fn decoding_key(&self) -> &DecodingKey {
        match self {
            Self::Hs256(key) | Self::Rs256(key) => key,
        }
    }
}

/// Invalid JWT verification configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtConfigError {
    message: String,
}

impl std::fmt::Display for JwtConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JwtConfigError {}

impl JwtConfig {
    /// Create configuration with HS256 symmetric key
    ///
    /// # Parameters
    ///
    /// * `secret` — Signing key (at least 32 characters, 64+ recommended)
    pub fn hs256(secret: impl Into<String>) -> Self {
        let secret = secret.into();
        Self {
            verification: Some(JwtVerification::Hs256(DecodingKey::from_secret(
                secret.as_bytes(),
            ))),
            issuer: None,
            audience: None,
        }
    }

    /// Create configuration with RS256 public key
    ///
    /// # Parameters
    ///
    /// * `public_key` - PEM-format RSA public key
    pub fn rs256(public_key: impl Into<String>) -> Result<Self, JwtConfigError> {
        let public_key = public_key.into();
        let decoding_key =
            DecodingKey::from_rsa_pem(public_key.as_bytes()).map_err(|error| JwtConfigError {
                message: format!("invalid RSA public key: {error}"),
            })?;
        Ok(Self {
            verification: Some(JwtVerification::Rs256(decoding_key)),
            issuer: None,
            audience: None,
        })
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

    /// Disable authentication (allow all requests through)
    pub fn disabled() -> Self {
        Self {
            verification: None,
            issuer: None,
            audience: None,
        }
    }

    /// Whether authentication is enabled
    pub fn is_enabled(&self) -> bool {
        self.verification.is_some()
    }
}

impl std::fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtConfig")
            .field("enabled", &self.is_enabled())
            .field(
                "algorithm",
                &self.verification.as_ref().map(JwtVerification::algorithm),
            )
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("verification_key", &"[redacted]")
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
    if !config.is_enabled() {
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
    let verification = config
        .verification
        .as_ref()
        .ok_or_else(|| "JWT authentication is disabled".to_string())?;
    let mut validation = Validation::new(verification.algorithm());

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

    let token_data = decode::<JwtClaims>(token, verification.decoding_key(), &validation)
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
    use axum::http::HeaderValue;
    use jsonwebtoken::{EncodingKey, Header, encode};

    const RSA_PRIVATE_KEY: &[u8] = include_bytes!("../../tests/fixtures/rsa-private.pem");
    const RSA_PUBLIC_KEY: &str = include_str!("../../tests/fixtures/rsa-public.pem");

    fn claims() -> JwtClaims {
        JwtClaims {
            iss: Some("echo-agent".to_string()),
            sub: Some("client-123".to_string()),
            aud: Some("a2a-clients".to_string()),
            exp: Some(4_102_444_800),
            nbf: None,
            iat: None,
            jti: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer my.jwt.token"),
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
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
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
        assert!(debug_str.contains("HS256"));
    }

    #[test]
    fn hs256_validates_only_hs256_tokens() {
        let config = JwtConfig::hs256("a-test-secret-that-is-long-enough")
            .with_issuer("echo-agent")
            .with_audience("a2a-clients");
        let token_result = encode(
            &Header::new(Algorithm::HS256),
            &claims(),
            &EncodingKey::from_secret(b"a-test-secret-that-is-long-enough"),
        );
        assert!(token_result.is_ok());
        let Some(token) = token_result.ok() else {
            return;
        };
        assert!(validate_token(&config, &token).is_ok());

        let wrong_secret_token = encode(
            &Header::new(Algorithm::HS256),
            &claims(),
            &EncodingKey::from_secret(b"a-different-test-secret-long-enough"),
        );
        assert!(wrong_secret_token.is_ok());
        let Some(wrong_secret_token) = wrong_secret_token.ok() else {
            return;
        };
        assert!(validate_token(&config, &wrong_secret_token).is_err());
    }

    #[test]
    fn rs256_validates_only_matching_rsa_tokens() {
        let config_result = JwtConfig::rs256(RSA_PUBLIC_KEY).map(|config| {
            config
                .with_issuer("echo-agent")
                .with_audience("a2a-clients")
        });
        assert!(config_result.is_ok());
        let Some(config) = config_result.ok() else {
            return;
        };
        let encoding_key_result = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY);
        assert!(encoding_key_result.is_ok());
        let Some(encoding_key) = encoding_key_result.ok() else {
            return;
        };
        let token_result = encode(&Header::new(Algorithm::RS256), &claims(), &encoding_key);
        assert!(token_result.is_ok());
        let Some(token) = token_result.ok() else {
            return;
        };
        assert!(validate_token(&config, &token).is_ok());

        let hmac_token = encode(
            &Header::new(Algorithm::HS256),
            &claims(),
            &EncodingKey::from_secret(RSA_PUBLIC_KEY.as_bytes()),
        );
        assert!(hmac_token.is_ok());
        let Some(hmac_token) = hmac_token.ok() else {
            return;
        };
        assert!(validate_token(&config, &hmac_token).is_err());
    }

    #[test]
    fn rs256_rejects_invalid_pem_at_configuration_time() {
        assert!(JwtConfig::rs256("not a PEM key").is_err());
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
