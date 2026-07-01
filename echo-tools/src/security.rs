//! Security module - path validation and resource limits
//!
//! Provides unified file access security controls:
//! - Path sandbox: prevents path traversal attacks
//! - Resource limits: prevents DoS and OOM

use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use echo_core::error::{Result, ToolError};

// ─────────────────────────────────────────────────────────────────────────────
// Resource Limits Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Resource limits configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum file size in bytes, default 50MB
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Maximum preview rows, default 10000
    #[serde(default = "default_max_preview_rows")]
    pub max_preview_rows: usize,

    /// Maximum preview characters, default 100KB
    #[serde(default = "default_max_preview_chars")]
    pub max_preview_chars: usize,

    /// Maximum PDF preview pages, default 50
    #[serde(default = "default_max_preview_pages")]
    pub max_preview_pages: usize,

    /// Maximum image pixels, default 4096*4096 = 16M
    #[serde(default = "default_max_image_pixels")]
    pub max_image_pixels: usize,

    /// HTTP request timeout in seconds, default 30
    #[serde(default = "default_http_timeout_secs")]
    pub http_timeout_secs: u64,

    /// HTTP maximum response body size in bytes, default 10MB
    #[serde(default = "default_http_max_size")]
    pub http_max_size: u64,

    /// Regex timeout in seconds, default 5
    #[serde(default = "default_regex_timeout_secs")]
    pub regex_timeout_secs: u64,

    /// Regex maximum memory in bytes, default 10MB
    #[serde(default = "default_regex_max_size")]
    pub regex_max_size: usize,
}

fn default_max_file_size() -> u64 {
    50 * 1024 * 1024 // 50MB
}

fn default_max_preview_rows() -> usize {
    10000
}

fn default_max_preview_chars() -> usize {
    100 * 1024 // 100KB
}

fn default_max_preview_pages() -> usize {
    50
}

fn default_max_image_pixels() -> usize {
    4096 * 4096
}

fn default_http_timeout_secs() -> u64 {
    30
}

fn default_http_max_size() -> u64 {
    10 * 1024 * 1024 // 10MB
}

fn default_regex_timeout_secs() -> u64 {
    5
}

fn default_regex_max_size() -> usize {
    10 * 1024 * 1024 // 10MB
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_file_size: default_max_file_size(),
            max_preview_rows: default_max_preview_rows(),
            max_preview_chars: default_max_preview_chars(),
            max_preview_pages: default_max_preview_pages(),
            max_image_pixels: default_max_image_pixels(),
            http_timeout_secs: default_http_timeout_secs(),
            http_max_size: default_http_max_size(),
            regex_timeout_secs: default_regex_timeout_secs(),
            regex_max_size: default_regex_max_size(),
        }
    }
}

impl ResourceLimits {
    /// Create default limits configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Get HTTP timeout Duration
    pub fn http_timeout(&self) -> Duration {
        Duration::from_secs(self.http_timeout_secs)
    }

    /// Get regex timeout Duration
    pub fn regex_timeout(&self) -> Duration {
        Duration::from_secs(self.regex_timeout_secs)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Path Validator
// ─────────────────────────────────────────────────────────────────────────────

/// Path validator
///
/// Ensures file access is within allowed directory ranges, preventing path traversal attacks.
#[derive(Debug, Clone)]
pub struct PathValidator {
    /// List of allowed root directories (empty = allow all)
    allowed_roots: Vec<PathBuf>,

    /// List of denied paths
    denied_paths: Vec<PathBuf>,

    /// Resource limits
    limits: ResourceLimits,

    /// Whether validation is enabled (for testing or trusted environments)
    enabled: bool,
}

impl Default for PathValidator {
    fn default() -> Self {
        Self {
            allowed_roots: Vec::new(),
            denied_paths: Vec::new(),
            limits: ResourceLimits::default(),
            enabled: true,
        }
    }
}

impl PathValidator {
    /// Create a new path validator
    pub fn new() -> Self {
        Self::default()
    }

    /// Set allowed root directories
    pub fn with_allowed_roots(mut self, roots: &[&str]) -> Self {
        self.allowed_roots = roots.iter().map(PathBuf::from).collect();
        self
    }

    /// Set denied paths
    pub fn with_denied_paths(mut self, paths: &[&str]) -> Self {
        self.denied_paths = paths.iter().map(PathBuf::from).collect();
        self
    }

    /// Set resource limits
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Enable/disable validation
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Validate file path
    ///
    /// Returns the canonicalized path, or an error
    pub fn validate_file(&self, path: &str) -> Result<PathBuf> {
        if !self.enabled {
            return Ok(PathBuf::from(path));
        }

        let path = Path::new(path);

        // 1. Check if path is absolute
        if !path.is_absolute() {
            return Err(ToolError::InvalidPath {
                path: path.display().to_string(),
                reason: "Path must be absolute".to_string(),
            }
            .into());
        }

        // 2. Normalize path (resolve .. and .)
        let canonical = path.canonicalize().map_err(|e| ToolError::InvalidPath {
            path: path.display().to_string(),
            reason: format!("Path does not exist or cannot be accessed: {}", e),
        })?;

        // 3. Check if in denied list
        for denied in &self.denied_paths {
            if canonical.starts_with(denied) {
                return Err(ToolError::AccessDenied {
                    path: path.display().to_string(),
                    reason: "Path is in the denied list".to_string(),
                }
                .into());
            }
        }

        // 4. Check if within allowed root directories (if configured)
        if !self.allowed_roots.is_empty() {
            let is_allowed = self.allowed_roots.iter().any(|root| {
                if let Ok(root_canonical) = root.canonicalize() {
                    canonical.starts_with(&root_canonical)
                } else {
                    false
                }
            });

            if !is_allowed {
                return Err(ToolError::AccessDenied {
                    path: path.display().to_string(),
                    reason: "Path is not within allowed directory scope".to_string(),
                }
                .into());
            }
        }

        // 5. Check file size
        let metadata = std::fs::metadata(&canonical).map_err(|e| ToolError::ExecutionFailed {
            tool: "path_validator".to_string(),
            message: format!("Unable to get file info: {}", e),
        })?;

        if metadata.len() > self.limits.max_file_size {
            return Err(ToolError::FileTooLarge {
                size: metadata.len(),
                max: self.limits.max_file_size,
            }
            .into());
        }

        Ok(canonical)
    }

    /// Validate that a path is within a base directory (may not exist yet).
    ///
    /// Canonicalizes the nearest existing ancestor, re-appends the suffix,
    /// verifies containment within the base, and checks the denied-paths list.
    /// This is the **canonical** validator for the common "restrict to a
    /// workspace root" pattern (Phase 6.1).  Other path validators in the
    /// codebase should converge here.
    pub fn validate_within_base(&self, path: &str, base: &Path) -> Result<PathBuf> {
        if !self.enabled {
            return Ok(PathBuf::from(path));
        }
        let requested = Path::new(path);
        if !requested.is_absolute() {
            return Err(ToolError::InvalidPath {
                path: path.to_string(),
                reason: "Path must be absolute".into(),
            }
            .into());
        }
        // Reject `..` lexically
        for comp in requested.components() {
            if matches!(comp, std::path::Component::ParentDir) {
                return Err(ToolError::InvalidPath {
                    path: path.to_string(),
                    reason: "Path traversal (..) is not allowed".into(),
                }
                .into());
            }
        }
        // Find nearest existing ancestor
        let mut check = if requested.exists() {
            requested
        } else {
            requested.parent().unwrap_or(requested)
        };
        while !check.exists() {
            if let Some(p) = check.parent() {
                check = p;
            } else {
                break;
            }
        }
        let canonical_ancestor = check.canonicalize().map_err(|e| ToolError::InvalidPath {
            path: check.display().to_string(),
            reason: format!("Cannot resolve path: {}", e),
        })?;
        let base_canonical = base.canonicalize().map_err(|e| ToolError::InvalidPath {
            path: base.display().to_string(),
            reason: format!("Cannot resolve base: {}", e),
        })?;
        if !canonical_ancestor.starts_with(&base_canonical) {
            return Err(ToolError::AccessDenied {
                path: path.to_string(),
                reason: "Path is outside the allowed base directory".into(),
            }
            .into());
        }
        // Reconstruct the full path
        let suffix = requested
            .strip_prefix(check)
            .map_err(|_| ToolError::InvalidPath {
                path: path.to_string(),
                reason: "Cannot compute relative suffix".into(),
            })?;
        let resolved = canonical_ancestor.join(suffix);
        // Re-check denied paths on the final path
        for denied in &self.denied_paths {
            if let Ok(d) = denied.canonicalize()
                && resolved.starts_with(&d)
            {
                return Err(ToolError::AccessDenied {
                    path: path.to_string(),
                    reason: "Path is in the denied list".into(),
                }
                .into());
            }
        }
        Ok(resolved)
    }

    /// Validate output file path.
    ///
    /// Unlike `validate_file()`, output paths allow the file to not yet exist,
    /// but still:
    /// - Require absolute paths
    /// - Normalize `.` / `..`
    /// - Check denied_paths / allowed_roots
    pub fn validate_output_file(&self, path: &str) -> Result<PathBuf> {
        if !self.enabled {
            return Ok(PathBuf::from(path));
        }

        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(ToolError::InvalidPath {
                path: path.display().to_string(),
                reason: "Path must be absolute".to_string(),
            }
            .into());
        }

        let normalized = normalize_absolute_path(path)?;

        for denied in &self.denied_paths {
            if normalize_for_policy(denied)
                .map(|denied_path| normalized.starts_with(&denied_path))
                .unwrap_or(false)
            {
                return Err(ToolError::AccessDenied {
                    path: path.display().to_string(),
                    reason: "Path is in the denied list".to_string(),
                }
                .into());
            }
        }

        if !self.allowed_roots.is_empty() {
            let is_allowed = self.allowed_roots.iter().any(|root| {
                normalize_for_policy(root)
                    .map(|allowed_root| normalized.starts_with(&allowed_root))
                    .unwrap_or(false)
            });

            if !is_allowed {
                return Err(ToolError::AccessDenied {
                    path: path.display().to_string(),
                    reason: "Path is not within allowed directory scope".to_string(),
                }
                .into());
            }
        }

        Ok(normalized)
    }

    /// Validate and get file content size
    pub fn get_file_size(path: &Path) -> Result<u64> {
        let metadata = std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed {
            tool: "path_validator".to_string(),
            message: format!("Unable to get file info: {}", e),
        })?;
        Ok(metadata.len())
    }

    /// Get resource limits
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global Security Configuration
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::OnceLock;

static GLOBAL_SECURITY: OnceLock<Arc<SecurityConfig>> = OnceLock::new();

/// Security configuration
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Path validator, limits file access scope
    pub path_validator: PathValidator,
    /// Resource limits (memory, CPU, concurrency, etc.)
    pub limits: ResourceLimits,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            path_validator: PathValidator::new(),
            limits: ResourceLimits::default(),
        }
    }
}

impl SecurityConfig {
    /// Get global security configuration
    pub fn global() -> Arc<Self> {
        GLOBAL_SECURITY
            .get_or_init(|| Arc::new(Self::default()))
            .clone()
    }

    /// Set global security configuration
    pub fn set_global(config: Self) {
        let _ = GLOBAL_SECURITY.set(Arc::new(config));
    }

    /// Validate file path
    pub fn validate_file(&self, path: &str) -> Result<PathBuf> {
        self.path_validator.validate_file(path)
    }

    /// Validate output file path (allows target file to not yet exist).
    pub fn validate_output_file(&self, path: &str) -> Result<PathBuf> {
        self.path_validator.validate_output_file(path)
    }

    /// Check file size
    pub fn check_file_size(&self, size: u64) -> Result<()> {
        if size > self.limits.max_file_size {
            return Err(ToolError::FileTooLarge {
                size,
                max: self.limits.max_file_size,
            }
            .into());
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Safe HTTP Client
// ─────────────────────────────────────────────────────────────────────────────

/// Create a safely configured HTTP client
pub fn create_safe_http_client(limits: &ResourceLimits) -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(limits.http_timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .redirect(ssrf_safe_redirect_policy())
        .build()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "http_client".to_string(),
            message: format!("Failed to create HTTP client: {}", e),
        })?;

    Ok(client)
}

// ─────────────────────────────────────────────────────────────────────────────
// Safe Regex
// ─────────────────────────────────────────────────────────────────────────────

use regex::RegexBuilder;

/// Create a safely configured regex
pub fn create_safe_regex(pattern: &str, limits: &ResourceLimits) -> Result<regex::Regex> {
    RegexBuilder::new(pattern)
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .map_err(|e| {
            ToolError::InvalidParameter {
                name: "pattern".to_string(),
                message: format!("Invalid regex: {}", e),
            }
            .into()
        })
}

// ─────────────────────────────────────────────────────────────────────────────
// SSRF Protection
// ─────────────────────────────────────────────────────────────────────────────

/// Validate URL target address, rejecting requests to private/link-local IPs (SSRF protection).
///
/// Returns the validated hostname and the resolved public IP addresses. Callers that
/// connect using these addresses (via [`pin_request_to_addrs`]) close the DNS-rebinding
/// TOCTOU window that a standalone `validate_url` + independent client resolution leaves open.
///
/// Note: this function still resolves DNS once; to be fully rebinding-safe, use the
/// returned addresses to build a pinned connection (see [`ssrf_safe_get`]).
pub fn validate_url(url_str: &str) -> Result<()> {
    validate_url_with_addrs(url_str).map(|_| ())
}

/// Validate a URL and return the hostname plus all resolved, SSRF-checked IP addresses.
///
/// This is the rebinding-aware variant of [`validate_url`]: it resolves the hostname
/// once, rejects the request if *any* resolved address is private/link-local, and
/// returns the remaining public addresses. Pass these to a client configured with
/// [`reqwest::ClientBuilder::resolve_to_addrs`] so the connection reuses the exact
/// validated IPs instead of resolving a second time (which DNS-rebinding would
/// exploit to point at `127.0.0.1` / `169.254.169.254`).
pub fn validate_url_with_addrs(url_str: &str) -> Result<(String, Vec<std::net::IpAddr>)> {
    let host = extract_host(url_str)?.to_string();

    // Resolve hostname to IP address
    let addr_str = format!("{}:0", host);
    let addrs = addr_str
        .to_socket_addrs()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "security".to_string(),
            message: format!("SSRF protection: DNS resolution failed: {}", e),
        })?;

    let mut public: Vec<std::net::IpAddr> = Vec::new();
    for addr in addrs {
        let ip = addr.ip();
        if is_private_ip(&ip) {
            return Err(ToolError::AccessDenied {
                path: url_str.to_string(),
                reason: format!(
                    "SSRF protection: rejecting access to private IP address {}",
                    ip
                ),
            }
            .into());
        }
        public.push(ip);
    }

    Ok((host, public))
}

/// Extract hostname from URL string (without url crate dependency)
fn extract_host(url_str: &str) -> Result<&str> {
    let rest = url_str
        .strip_prefix("http://")
        .or_else(|| url_str.strip_prefix("https://"))
        .ok_or_else(|| ToolError::InvalidParameter {
            name: "url".to_string(),
            message: "URL must start with http:// or https://".to_string(),
        })?;

    // Extract authority (host:port)
    let authority = rest.split('/').next().unwrap_or(rest);
    // Remove ?query
    let authority = authority.split('?').next().unwrap_or(authority);
    // Remove userinfo@ -- take everything after the last @
    let authority = authority.rsplit('@').next().unwrap_or(authority);

    // Extract host, handling IPv6 bracket notation [::1]:port
    let host = if authority.starts_with('[') {
        // IPv6 literal: find the closing bracket
        if let Some(close) = authority.find(']') {
            &authority[1..close]
        } else {
            // Malformed — treat the whole thing as host (will likely fail DNS)
            authority
        }
    } else {
        // Plain host:port — strip port by splitting on first colon
        authority.split(':').next().unwrap_or(authority)
    };

    if host.is_empty() {
        return Err(ToolError::InvalidParameter {
            name: "url".to_string(),
            message: "URL missing hostname".to_string(),
        }
        .into());
    }

    Ok(host)
}

/// Check if an IP address is private/link-local
fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // Check for IPv4-mapped IPv6 addresses (::ffff:x.x.x.x)
            // and validate the mapped IPv4 address
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(&v4);
            }

            let octets = v6.octets();
            // ::1 (localhost)
            *v6 == std::net::Ipv6Addr::LOCALHOST
                // fc00::/7 (ULA, RFC 4193) — covers both fc00::/8 and fd00::/8
                || (octets[0] & 0xFE) == 0xFC
                // fe80::/10 (link-local)
                || (octets[0] == 0xfe && (octets[1] & 0xC0) == 0x80)
        }
    }
}

/// Check if an IPv4 address is private/link-local/reserved
fn is_private_v4(v4: &std::net::Ipv4Addr) -> bool {
    let octets = v4.octets();
    // 127.0.0.0/8 (loopback)
    octets[0] == 127
        // 10.0.0.0/8 (RFC 1918)
        || octets[0] == 10
        // 172.16.0.0/12 (RFC 1918)
        || (octets[0] == 172 && (octets[1] & 0xF0) == 16)
        // 192.168.0.0/16 (RFC 1918)
        || (octets[0] == 192 && octets[1] == 168)
        // 169.254.0.0/16 (link-local)
        || (octets[0] == 169 && octets[1] == 254)
        // 0.0.0.0/8 (current network)
        || octets[0] == 0
        // 100.64.0.0/10 (CGNAT / Shared Address Space)
        || (octets[0] == 100 && (octets[1] & 0xC0) == 64)
        // 192.0.0.0/24 (IETF Protocol Assignments)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        // 198.18.0.0/15 (Network benchmark testing)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        // 192.0.2.0/24 (TEST-NET-1)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        // 198.51.100.0/24 (TEST-NET-2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        // 203.0.113.0/24 (TEST-NET-3)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
}

/// Create an SSRF-safe redirect policy
pub fn ssrf_safe_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 5 {
            return attempt.error("SSRF protection: too many redirects");
        }
        match validate_url(attempt.url().as_str()) {
            Ok(()) => attempt.follow(),
            Err(e) => attempt.error(format!("SSRF protection: redirect target blocked: {}", e)),
        }
    })
}

/// Build a one-shot [`reqwest::Client`] pinned to the validated IP addresses for `host`,
/// closing the DNS-rebinding TOCTOU window.
///
/// The returned client sends the TLS SNI and `Host` header as `host` (so virtual-hosting
/// and TLS certificate validation still work) but connects *only* to the addresses in
/// `addrs` — the exact IPs that [`validate_url_with_addrs`] checked. DNS is never
/// consulted again for this request.
fn pinned_client(
    host: &str,
    addrs: &[std::net::IpAddr],
    timeout: Duration,
) -> Result<reqwest::Client> {
    // `resolve_to_addrs` needs SocketAddrs; the port is irrelevant for IP pinning
    // (reqwest fills in the real port from the URL), so use a placeholder port.
    let socket_addrs: Vec<std::net::SocketAddr> = addrs
        .iter()
        .map(|ip| std::net::SocketAddr::new(*ip, 0))
        .collect();
    reqwest::Client::builder()
        .resolve_to_addrs(host, &socket_addrs)
        .redirect(reqwest::redirect::Policy::none()) // redirects are re-validated by callers
        .timeout(timeout)
        .build()
        .map_err(|e| {
            ToolError::ExecutionFailed {
                tool: "security".to_string(),
                message: format!("SSRF protection: failed to build pinned client: {}", e),
            }
            .into()
        })
}

/// Perform an SSRF-safe HTTP GET that pins the resolved IP to defeat DNS rebinding.
///
/// Workflow: resolve+validate once → build a client pinned to the validated IPs →
/// send. Each redirect hop is independently re-resolved and re-validated (so a
/// `302 → 169.254.169.254` cannot slip through). This is the single entry point
/// all web-fetching tools should use instead of `validate_url(url)?; client.get(url)`.
pub async fn ssrf_safe_get(
    url: &str,
    timeout: Duration,
    max_redirects: usize,
) -> Result<reqwest::Response> {
    ssrf_safe_request(url, timeout, max_redirects, reqwest::Method::GET).await
}

/// SSRF-safe request (any method) with IP pinning and per-hop redirect validation.
pub async fn ssrf_safe_request(
    url: &str,
    timeout: Duration,
    max_redirects: usize,
    method: reqwest::Method,
) -> Result<reqwest::Response> {
    ssrf_safe_request_full(url, timeout, max_redirects, method, None, None).await
}

/// SSRF-safe request with an optional JSON body and extra headers.
///
/// Used by HTTP hooks that POST a payload to an operator-configured URL.
/// Reuses the same resolve-once → pin-IP → per-redirect-revalidate pipeline
/// as [`ssrf_safe_request`]; the body and headers are attached to each hop's
/// request. The body is consumed on the first hop (a JSON body cannot be
/// re-sent across redirects in reqwest without re-serializing), so redirects
/// drop the body — which is the conservative choice for an SSRF-sensitive
/// outbound POST (we never want to replay secrets to a redirect target).
pub async fn ssrf_safe_request_with_body(
    url: &str,
    timeout: Duration,
    max_redirects: usize,
    method: reqwest::Method,
    body: &serde_json::Value,
    headers: Option<&HashMap<String, String>>,
) -> Result<reqwest::Response> {
    ssrf_safe_request_full(url, timeout, max_redirects, method, Some(body), headers).await
}

/// Core SSRF-safe pipeline shared by the body and bodyless variants.
///
/// `body` and `headers` are only applied on the first hop. Redirects are
/// followed bodyless to avoid replaying a (possibly secret-laden) payload to
/// a redirect destination the validator may not have seen.
async fn ssrf_safe_request_full(
    url: &str,
    timeout: Duration,
    max_redirects: usize,
    method: reqwest::Method,
    body: Option<&serde_json::Value>,
    headers: Option<&HashMap<String, String>>,
) -> Result<reqwest::Response> {
    let mut current = url.to_string();
    let mut first_hop = true;
    for _ in 0..=max_redirects {
        let (host, addrs) = validate_url_with_addrs(&current)?;
        let client = pinned_client(&host, &addrs, timeout)?;
        let mut req = client.request(method.clone(), &current);
        // Attach body + headers only on the first hop; see fn doc.
        if first_hop {
            if let Some(b) = body {
                req = req.json(b);
            }
            if let Some(h) = headers {
                for (k, v) in h {
                    req = req.header(k, v);
                }
            }
            first_hop = false;
        }
        let response = req.send().await.map_err(|e| ToolError::ExecutionFailed {
            tool: "security".to_string(),
            message: format!("SSRF-safe request failed: {}", e),
        })?;

        if response.status().is_redirection() {
            // Re-resolve and re-validate the redirect target independently.
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| ToolError::ExecutionFailed {
                    tool: "security".to_string(),
                    message: "redirect without Location header".to_string(),
                })?;
            current = if location.starts_with("http://") || location.starts_with("https://") {
                location.to_string()
            } else {
                // Relative redirect: resolve against the current URL's origin.
                let origin_end = current
                    .find("://")
                    .and_then(|i| current[i + 3..].find('/').map(|j| i + 3 + j))
                    .unwrap_or(current.len());
                // origin_end indexes into a `&str` derived from a URL that is
                // ASCII up to the first path separator; byte index is safe.
                format!("{}{}", &current[..origin_end], location)
            };
            continue;
        }
        return Ok(response);
    }
    Err(ToolError::ExecutionFailed {
        tool: "security".to_string(),
        message: format!("SSRF protection: too many redirects (>{max_redirects})"),
    }
    .into())
}

fn normalize_for_policy(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .or_else(|| normalize_absolute_path(path).ok())
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(ToolError::InvalidPath {
            path: path.display().to_string(),
            reason: "Path must be absolute".to_string(),
        }
        .into());
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_validator_absolute_required() {
        let validator = PathValidator::new().with_enabled(true);
        let result = validator.validate_file("relative/path.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_path_validator_allowed_root() {
        let _validator = PathValidator::new()
            .with_allowed_roots(&["/tmp"])
            .with_enabled(true);
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_file_size, 50 * 1024 * 1024);
        assert_eq!(limits.max_preview_rows, 10000);
    }

    #[test]
    fn test_validate_output_file_absolute_required() {
        let validator = PathValidator::new().with_enabled(true);
        let result = validator.validate_output_file("relative/output.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_output_file_normalizes_parent_segments() {
        let validator = PathValidator::new().with_enabled(true);
        let path = validator
            .validate_output_file("/tmp/demo/../result.txt")
            .unwrap();
        assert_eq!(path, PathBuf::from("/tmp/result.txt"));
    }

    // ── SSRF golden tests ─────────────────────────────────────────────────

    #[test]
    fn test_is_private_ip_v4_loopback() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"127.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v4_rfc1918() {
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.255.255.255".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"172.31.255.255".parse().unwrap()));
        assert!(is_private_ip(&"192.168.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v4_link_local() {
        assert!(is_private_ip(&"169.254.0.1".parse().unwrap()));
        assert!(is_private_ip(&"169.254.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v4_cgnat() {
        assert!(is_private_ip(&"100.64.0.1".parse().unwrap()));
        assert!(is_private_ip(&"100.127.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v4_special_ranges() {
        // 0.0.0.0/8
        assert!(is_private_ip(&"0.0.0.0".parse().unwrap()));
        assert!(is_private_ip(&"0.255.255.255".parse().unwrap()));
        // TEST-NET-1
        assert!(is_private_ip(&"192.0.2.1".parse().unwrap()));
        // TEST-NET-2
        assert!(is_private_ip(&"198.51.100.1".parse().unwrap()));
        // TEST-NET-3
        assert!(is_private_ip(&"203.0.113.1".parse().unwrap()));
        // IETF assignments
        assert!(is_private_ip(&"192.0.0.1".parse().unwrap()));
        // Benchmark
        assert!(is_private_ip(&"198.18.0.1".parse().unwrap()));
        assert!(is_private_ip(&"198.19.255.255".parse().unwrap()));
    }

    #[test]
    fn test_public_ip_v4_allowed() {
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip(&"93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_loopback() {
        assert!(is_private_ip(&"::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_link_local() {
        assert!(is_private_ip(&"fe80::1".parse().unwrap()));
        assert!(is_private_ip(&"feb0::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_ula_full_range() {
        // fc00::/7 covers both fc00::/8 and fd00::/8 (RFC 4193)
        assert!(is_private_ip(&"fc00::1".parse().unwrap()));
        assert!(is_private_ip(&"fcff::1".parse().unwrap()));
        assert!(is_private_ip(&"fd00::1".parse().unwrap()));
        assert!(is_private_ip(&"fdff::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_mapped_v4() {
        // ::ffff:127.0.0.1 should be treated as private
        assert!(is_private_ip(&"::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"::ffff:10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"::ffff:192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_public_ip_v6_allowed() {
        assert!(!is_private_ip(&"2001:4860:4860::8888".parse().unwrap()));
        assert!(!is_private_ip(&"2606:4700:4700::1111".parse().unwrap()));
    }

    // ── extract_host golden tests ─────────────────────────────────────────

    #[test]
    fn test_extract_host_plain() {
        assert_eq!(
            extract_host("http://example.com/path").unwrap(),
            "example.com"
        );
        assert_eq!(extract_host("https://example.com").unwrap(), "example.com");
    }

    #[test]
    fn test_extract_host_with_port() {
        assert_eq!(
            extract_host("http://example.com:8080/path").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn test_extract_host_with_userinfo() {
        assert_eq!(
            extract_host("http://user:pass@example.com/path").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn test_extract_host_ipv6() {
        assert_eq!(extract_host("http://[::1]/path").unwrap(), "::1");
        assert_eq!(
            extract_host("https://[2001:db8::1]/api").unwrap(),
            "2001:db8::1"
        );
    }

    #[test]
    fn test_extract_host_ipv6_with_port() {
        assert_eq!(extract_host("http://[::1]:8080/path").unwrap(), "::1");
        assert_eq!(
            extract_host("http://[2001:db8::1]:443/data").unwrap(),
            "2001:db8::1"
        );
    }

    #[test]
    fn test_extract_host_with_query() {
        assert_eq!(
            extract_host("http://example.com/path?key=value").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn test_extract_host_rejects_non_http() {
        assert!(extract_host("ftp://example.com").is_err());
        assert!(extract_host("file:///etc/passwd").is_err());
    }

    // ── validate_url_with_addrs golden tests ─────────────────────────────

    #[test]
    fn test_validate_url_rejects_loopback() {
        assert!(validate_url("http://127.0.0.1:8080/data").is_err());
        assert!(validate_url("http://[::1]:8080/data").is_err());
    }

    #[test]
    fn test_validate_url_allows_public() {
        // This test requires DNS resolution of a known-public host.
        // example.com resolves to public IPs; if DNS is unavailable, skip.
        let result = validate_url("https://example.com/");
        // In CI without network, DNS may fail. We check that it's not a
        // private-IP rejection (which would always be an error).
        if let Err(e) = &result {
            let msg = format!("{}", e);
            assert!(
                !msg.contains("private IP"),
                "should not reject public host: {msg}"
            );
        }
    }

    #[test]
    fn test_validate_url_with_addrs_returns_ips() {
        let result = validate_url_with_addrs("https://example.com/");
        if let Ok((host, addrs)) = result {
            assert_eq!(host, "example.com");
            assert!(!addrs.is_empty(), "should return at least one IP");
            for ip in &addrs {
                assert!(!is_private_ip(ip), "returned IP {ip} should be public");
            }
        }
    }

    // ── Regression tests for fixed vulnerabilities ────────────────────────

    #[test]
    fn test_ipv6_ula_fc00_blocked() {
        // Regression: fc00::/8 was NOT blocked before fix (only fd00::/8 was)
        let ip: std::net::IpAddr = "fc00::1".parse().unwrap();
        assert!(
            is_private_ip(&ip),
            "fc00::/8 (ULA lower half) must be blocked"
        );
    }

    #[test]
    fn test_extract_host_ipv6_port_regression() {
        // Regression: extract_host used split(':').next() which broke on IPv6+port
        assert_eq!(
            extract_host("http://[::1]:8080/path").unwrap(),
            "::1",
            "IPv6 with port must work"
        );
        assert_eq!(
            extract_host("http://[fe80::1]:3000/").unwrap(),
            "fe80::1",
            "IPv6 link-local with port must work"
        );
    }

    // ── Fuzz / boundary tests ─────────────────────────────────────────────

    #[test]
    fn test_truncate_utf8_safe() {
        // Verify that truncation at byte boundaries is safe for multi-byte chars.
        // "é" = 0xC3 0xA9 (2 bytes), "好" = 0xE5 0xA5 0xBD (3 bytes)
        let s = "hello 世界!";
        // floor_char_boundary returns the safe byte index
        let safe = s.floor_char_boundary(8); // "hello " = 6 bytes, next is 3-byte "世"
        assert_eq!(&s[..safe], "hello ");
    }

    #[test]
    fn test_unicode_case_fold_security() {
        let path = "/home/user/.ssh/authorized_keys";
        let path_buf = std::path::PathBuf::from(path);
        let parent = path_buf.parent().unwrap();
        assert!(
            parent.ends_with(".ssh"),
            ".ssh directory should be detectable from path"
        );
    }

    // ── Non-ASCII fuzz tests (Phase 3.7) ──────────────────────────────────

    #[test]
    fn test_truncate_emoji_boundary() {
        // Emoji = 4 bytes (e.g., 😀 = 0xF0 0x9F 0x98 0x80)
        let s = "abc😀def";
        let safe = s.floor_char_boundary(4); // "abc" = 3, then 4-byte emoji
        assert_eq!(&s[..safe], "abc");
    }

    #[test]
    fn test_truncate_all_multibyte() {
        // String with only multi-byte chars
        let s = "世界你好"; // 4 chars × 3 bytes each = 12 bytes
        let safe = s.floor_char_boundary(4); // should stop at 3 bytes (1 char)
        assert_eq!(&s[..safe], "世");
        let safe2 = s.floor_char_boundary(7); // should stop at 6 bytes (2 chars)
        assert_eq!(&s[..safe2], "世界");
    }

    #[test]
    fn test_truncate_zero() {
        let s = "hello";
        assert_eq!(s.floor_char_boundary(0), 0);
    }

    #[test]
    fn test_truncate_past_end() {
        let s = "hi";
        let safe = s.floor_char_boundary(100);
        assert_eq!(safe, 2); // should return length, not panic
    }

    #[test]
    fn test_validate_url_rejects_unicode_homoglyph() {
        // Unicode FULLWIDTH SOLIDUS (U+FF0F) ／ should not bypass URL parsing
        // since it must start with ASCII "http://"
        let result = validate_url("http：//evil.com/");
        assert!(
            result.is_err(),
            "Non-ASCII colon should fail URL validation"
        );
    }

    #[test]
    fn test_extract_host_unicode_idn() {
        // IDN (Internationalized Domain Names) — extract_host strips the
        // hostname, including non-ASCII chars if present.
        let host = extract_host("http://münchen.de/path").unwrap();
        assert_eq!(host, "münchen.de");
    }

    #[test]
    fn test_byte_slice_safety_on_boundary_edge() {
        // Simulate the pattern from web_fetch_enhanced.rs: truncate at an
        // arbitrary byte position and verify floor_char_boundary is safe.
        for boundary in 0..=20 {
            let s = "a😀b世c界déefgh";
            let safe = s.floor_char_boundary(boundary);
            // Must be a valid char boundary — slicing should never panic
            let _ = &s[..safe];
        }
    }
}
