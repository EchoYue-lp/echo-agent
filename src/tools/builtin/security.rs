//! Security module - path validation and resource limits
//!
//! Provides unified file access security controls:
//! - Path sandbox: prevents path traversal attacks
//! - Resource limits: prevents DoS and OOM

use std::net::ToSocketAddrs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Result, ToolError};

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

/// Validate URL target address, rejecting requests to private/link-local IPs (SSRF protection)
pub fn validate_url(url_str: &str) -> Result<()> {
    let host = extract_host(url_str)?;

    // Resolve hostname to IP address
    let addr_str = format!("{}:0", host);
    let addrs = addr_str
        .to_socket_addrs()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "security".to_string(),
            message: format!("SSRF protection: DNS resolution failed: {}", e),
        })?;

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
    }

    Ok(())
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
    // Remove :port
    let authority = authority.split(':').next().unwrap_or(authority);
    // Remove userinfo@ -- take everything after the last @
    let host = authority.rsplit('@').next().unwrap_or(authority);

    // Handle IPv6: [::1] → ::1
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.split(']').next())
        .unwrap_or(host);

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
        std::net::IpAddr::V4(v4) => {
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
        }
        std::net::IpAddr::V6(v6) => {
            let octets = v6.octets();
            // ::1 (localhost)
            *v6 == std::net::Ipv6Addr::LOCALHOST
                // fd00::/8 (ULA)
                || octets[0] == 0xfd
                // fe80::/10 (link-local)
                || (octets[0] == 0xfe && (octets[1] & 0xC0) == 0x80)
        }
    }
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

        // This test depends on the actual filesystem
        // In practice, a test file should be created
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
}
