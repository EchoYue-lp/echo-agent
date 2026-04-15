//! 图片分析工具
//!
//! 提供图片分析能力，让 Agent 能主动"看图"。
//! 利用框架已有的多模态支持，通过 base64 编码图片发送给 LLM。

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{ResourceLimits, SecurityConfig, create_safe_http_client};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

/// 图片分析工具
///
/// 支持：
/// - 从文件路径读取图片
/// - 从 URL 获取图片
/// - 从 base64 数据分析
pub struct ImageAnalysisTool;

impl Tool for ImageAnalysisTool {
    fn name(&self) -> &str {
        "analyze_image"
    }

    fn description(&self) -> &str {
        "分析图片内容，描述图片中的信息。支持从文件路径、URL 或 base64 数据读取图片。返回图片的详细描述和分析结果。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "图片来源类型：'file'（文件路径）、'url'（网络地址）或 'base64'（编码数据）"
                },
                "data": {
                    "type": "string",
                    "description": "图片数据：文件路径、URL 或 base64 编码的图片数据"
                },
                "prompt": {
                    "type": "string",
                    "description": "分析提示词，告诉 LLM 你希望从图片中获取什么信息（可选）"
                }
            },
            "required": ["source", "data"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let source = parameters
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("source".to_string()))?;

            let data = parameters
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("data".to_string()))?;

            let prompt = parameters.get("prompt").and_then(|v| v.as_str()).unwrap_or(
                "请详细描述这张图片的内容，包括主要元素、颜色、布局和任何可见的文字信息。",
            );

            let security = SecurityConfig::global();

            // 根据来源类型获取图片数据
            let (base64_data, mime_type) = match source {
                "file" => read_image_from_file(data, &security.limits)?,
                "url" => fetch_image_from_url(data, &security.limits).await?,
                "base64" => {
                    // 验证 base64 数据
                    let mime_type = detect_mime_type_from_base64(data);
                    validate_base64_size(data, &security.limits)?;
                    (data.to_string(), mime_type)
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "source".to_string(),
                        message: format!(
                            "不支持的来源类型: '{}', 请使用 'file', 'url' 或 'base64'",
                            source
                        ),
                    }
                    .into());
                }
            };

            // 返回图片元信息，不返回原始 base64 数据
            // 注意：将完整 base64 作为文本传给 LLM 会导致上下文膨胀且 LLM 无法"看到"图片
            let size_kb = base64_data.len() as f64 / 1024.0;
            let estimated_raw_kb = (base64_data.len() as f64 * 3.0 / 4.0) / 1024.0;
            Ok(ToolResult::success(format!(
                "图片已成功加载。\n- MIME 类型: {}\n- Base64 长度: {:.1} KB\n- 估算原始大小: {:.1} KB\n- 分析提示: {}\n\n提示：图片数据已加载但无法直接以文本形式展示给 LLM。如需分析图片内容，请考虑使用支持多模态的 LLM 模型，或使用浏览器工具查看图片 URL。",
                mime_type, size_kb, estimated_raw_kb, prompt,
            )))
        })
    }
}

/// 从文件读取图片并转换为 base64
fn read_image_from_file(path: &str, limits: &ResourceLimits) -> Result<(String, String)> {
    use std::fs;
    use std::path::Path;

    let path_obj = Path::new(path);

    // 1. 检查是否为绝对路径
    if !path_obj.is_absolute() {
        return Err(ToolError::InvalidPath {
            path: path.to_string(),
            reason: "路径必须是绝对路径".to_string(),
        }
        .into());
    }

    // 2. 检查文件是否存在
    if !path_obj.exists() {
        return Err(ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: "文件不存在".to_string(),
        }
        .into());
    }

    // 3. 检查文件大小
    let metadata = fs::metadata(path_obj).map_err(|e| ToolError::ExecutionFailed {
        tool: "analyze_image".to_string(),
        message: format!("获取文件信息失败: {}", e),
    })?;

    if metadata.len() > limits.max_file_size {
        return Err(ToolError::FileTooLarge {
            size: metadata.len(),
            max: limits.max_file_size,
        }
        .into());
    }

    // 4. 检测 MIME 类型（使用文件内容而非扩展名）
    let mime_type = detect_image_mime_type(path_obj);

    // 5. 读取文件
    let bytes = fs::read(path_obj).map_err(|e| ToolError::ExecutionFailed {
        tool: "analyze_image".to_string(),
        message: format!("读取文件失败: {}", e),
    })?;

    let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);

    Ok((base64_data, mime_type))
}

/// 从 URL 获取图片并转换为 base64
async fn fetch_image_from_url(url: &str, limits: &ResourceLimits) -> Result<(String, String)> {
    // 使用安全配置的 HTTP 客户端
    let client = create_safe_http_client(limits)?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("请求图片失败: {}", e),
        })?;

    // 检查响应状态
    if !response.status().is_success() {
        return Err(ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("HTTP 请求失败: {}", response.status()),
        }
        .into());
    }

    // 检查 Content-Length
    if let Some(content_length) = response.headers().get("content-length")
        && let Ok(len_str) = content_length.to_str()
        && let Ok(len) = len_str.parse::<u64>()
        && len > limits.http_max_size
    {
        return Err(ToolError::FileTooLarge {
            size: len,
            max: limits.http_max_size,
        }
        .into());
    }

    // 从响应头获取 MIME 类型
    let mime_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| {
            // 提取主类型（去除 charset 等参数）
            ct.split(';').next()
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "image/png".to_string());

    // 读取响应体
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("读取图片数据失败: {}", e),
        })?;

    // 再次检查实际大小
    if bytes.len() as u64 > limits.http_max_size {
        return Err(ToolError::FileTooLarge {
            size: bytes.len() as u64,
            max: limits.http_max_size,
        }
        .into());
    }

    let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);

    Ok((base64_data, mime_type))
}

/// 使用 magic number 检测图片 MIME 类型
fn detect_image_mime_type(path: &std::path::Path) -> String {
    use std::fs::File;
    use std::io::Read;

    if let Ok(mut file) = File::open(path) {
        let mut buf = [0u8; 16];
        if let Ok(n) = file.read(&mut buf) {
            let header = &buf[..n];

            // PNG: 89 50 4E 47 0D 0A 1A 0A
            if header.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
                return "image/png".to_string();
            }
            // JPEG: FF D8 FF
            if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
                return "image/jpeg".to_string();
            }
            // GIF: 47 49 46 38
            if header.starts_with(b"GIF8") {
                return "image/gif".to_string();
            }
            // WebP: 52 49 46 46 ... 57 45 42 50
            if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
                return "image/webp".to_string();
            }
            // BMP: 42 4D
            if header.starts_with(b"BM") {
                return "image/bmp".to_string();
            }
        }
    }

    // 回退到扩展名检测
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    }
    .to_string()
}

/// 从 base64 数据头部检测 MIME 类型
fn detect_mime_type_from_base64(data: &str) -> String {
    // 检查常见图片格式的 base64 头部特征
    if data.starts_with("iVBORw0KGgo") {
        "image/png"
    } else if data.starts_with("/9j/") {
        "image/jpeg"
    } else if data.starts_with("R0lGOD") {
        "image/gif"
    } else if data.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/png" // 默认
    }
    .to_string()
}

/// 验证 base64 数据大小
fn validate_base64_size(data: &str, limits: &ResourceLimits) -> Result<()> {
    // Base64 编码后大小约为原始数据的 4/3
    // 估算原始大小
    let estimated_size = (data.len() as u64 * 3) / 4;

    if estimated_size > limits.max_file_size {
        return Err(ToolError::FileTooLarge {
            size: estimated_size,
            max: limits.max_file_size,
        }
        .into());
    }

    Ok(())
}
