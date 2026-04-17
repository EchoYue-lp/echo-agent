//! 安全模块 - 路径验证和资源限制
//!
//! 提供统一的文件访问安全控制：
//! - 路径沙箱：防止路径遍历攻击
//! - 资源限制：防止 DoS 和 OOM

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Result, ToolError};

// ─────────────────────────────────────────────────────────────────────────────
// 资源限制配置
// ─────────────────────────────────────────────────────────────────────────────

/// 资源限制配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// 最大文件大小（字节），默认 50MB
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// 最大预览行数，默认 10000
    #[serde(default = "default_max_preview_rows")]
    pub max_preview_rows: usize,

    /// 最大预览字符数，默认 100KB
    #[serde(default = "default_max_preview_chars")]
    pub max_preview_chars: usize,

    /// PDF 最大预览页数，默认 50
    #[serde(default = "default_max_preview_pages")]
    pub max_preview_pages: usize,

    /// 最大图片像素数，默认 4096*4096 = 16M
    #[serde(default = "default_max_image_pixels")]
    pub max_image_pixels: usize,

    /// HTTP 请求超时（秒），默认 30
    #[serde(default = "default_http_timeout_secs")]
    pub http_timeout_secs: u64,

    /// HTTP 最大响应体大小（字节），默认 10MB
    #[serde(default = "default_http_max_size")]
    pub http_max_size: u64,

    /// 正则表达式超时（秒），默认 5
    #[serde(default = "default_regex_timeout_secs")]
    pub regex_timeout_secs: u64,

    /// 正则表达式最大内存（字节），默认 10MB
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
    /// 创建默认限制配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取 HTTP 超时 Duration
    pub fn http_timeout(&self) -> Duration {
        Duration::from_secs(self.http_timeout_secs)
    }

    /// 获取正则表达式超时 Duration
    pub fn regex_timeout(&self) -> Duration {
        Duration::from_secs(self.regex_timeout_secs)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 路径验证器
// ─────────────────────────────────────────────────────────────────────────────

/// 路径验证器
///
/// 确保文件访问在允许的目录范围内，防止路径遍历攻击。
#[derive(Debug, Clone)]
pub struct PathValidator {
    /// 允许的根目录列表（空 = 允许所有）
    allowed_roots: Vec<PathBuf>,

    /// 禁止的路径列表
    denied_paths: Vec<PathBuf>,

    /// 资源限制
    limits: ResourceLimits,

    /// 是否启用验证（用于测试或信任环境）
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
    /// 创建新的路径验证器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置允许的根目录
    pub fn with_allowed_roots(mut self, roots: &[&str]) -> Self {
        self.allowed_roots = roots.iter().map(PathBuf::from).collect();
        self
    }

    /// 设置禁止的路径
    pub fn with_denied_paths(mut self, paths: &[&str]) -> Self {
        self.denied_paths = paths.iter().map(PathBuf::from).collect();
        self
    }

    /// 设置资源限制
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// 启用/禁用验证
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// 验证文件路径
    ///
    /// 返回规范化后的路径，或错误
    pub fn validate_file(&self, path: &str) -> Result<PathBuf> {
        if !self.enabled {
            return Ok(PathBuf::from(path));
        }

        let path = Path::new(path);

        // 1. 检查是否为绝对路径
        if !path.is_absolute() {
            return Err(ToolError::InvalidPath {
                path: path.display().to_string(),
                reason: "路径必须是绝对路径".to_string(),
            }
            .into());
        }

        // 2. 规范化路径（解析 .. 和 .）
        let canonical = path.canonicalize().map_err(|e| ToolError::InvalidPath {
            path: path.display().to_string(),
            reason: format!("路径不存在或无法访问: {}", e),
        })?;

        // 3. 检查是否在禁止列表中
        for denied in &self.denied_paths {
            if canonical.starts_with(denied) {
                return Err(ToolError::AccessDenied {
                    path: path.display().to_string(),
                    reason: "路径在禁止列表中".to_string(),
                }
                .into());
            }
        }

        // 4. 检查是否在允许的根目录内（如果配置了）
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
                    reason: "路径不在允许的目录范围内".to_string(),
                }
                .into());
            }
        }

        // 5. 检查文件大小
        let metadata = std::fs::metadata(&canonical).map_err(|e| ToolError::ExecutionFailed {
            tool: "path_validator".to_string(),
            message: format!("无法获取文件信息: {}", e),
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

    /// 验证并获取文件内容大小
    pub fn get_file_size(path: &Path) -> Result<u64> {
        let metadata = std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed {
            tool: "path_validator".to_string(),
            message: format!("无法获取文件信息: {}", e),
        })?;
        Ok(metadata.len())
    }

    /// 获取资源限制
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 全局安全配置
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::OnceLock;

static GLOBAL_SECURITY: OnceLock<Arc<SecurityConfig>> = OnceLock::new();

/// 安全配置
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// 路径验证器，限制文件访问范围
    pub path_validator: PathValidator,
    /// 资源限制（内存、CPU、并发等）
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
    /// 获取全局安全配置
    pub fn global() -> Arc<Self> {
        GLOBAL_SECURITY
            .get_or_init(|| Arc::new(Self::default()))
            .clone()
    }

    /// 设置全局安全配置
    pub fn set_global(config: Self) {
        let _ = GLOBAL_SECURITY.set(Arc::new(config));
    }

    /// 验证文件路径
    pub fn validate_file(&self, path: &str) -> Result<PathBuf> {
        self.path_validator.validate_file(path)
    }

    /// 检查文件大小
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
// 安全 HTTP 客户端
// ─────────────────────────────────────────────────────────────────────────────

/// 创建安全配置的 HTTP 客户端
pub fn create_safe_http_client(limits: &ResourceLimits) -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(limits.http_timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "http_client".to_string(),
            message: format!("创建 HTTP 客户端失败: {}", e),
        })?;

    Ok(client)
}

// ─────────────────────────────────────────────────────────────────────────────
// 安全正则表达式
// ─────────────────────────────────────────────────────────────────────────────

use regex::RegexBuilder;

/// 创建安全配置的正则表达式
pub fn create_safe_regex(pattern: &str, limits: &ResourceLimits) -> Result<regex::Regex> {
    RegexBuilder::new(pattern)
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .map_err(|e| {
            ToolError::InvalidParameter {
                name: "pattern".to_string(),
                message: format!("无效的正则表达式: {}", e),
            }
            .into()
        })
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

        // 这个测试取决于实际文件系统
        // 在实际使用中应该创建测试文件
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_file_size, 50 * 1024 * 1024);
        assert_eq!(limits.max_preview_rows, 10000);
    }
}
