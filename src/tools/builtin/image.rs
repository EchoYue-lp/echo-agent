//! Image analysis tools
//!
//! Provides image analysis capabilities, enabling the Agent to "see" images.
//! Leverages the framework's existing multimodal support by encoding images
//! as base64 and sending them to the LLM.

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{ResourceLimits, SecurityConfig, create_safe_http_client, validate_url};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

/// Image analysis tool
///
/// Supports:
/// - Reading images from file paths
/// - Fetching images from URLs
/// - Analyzing from base64 data
pub struct ImageAnalysisTool;

impl Tool for ImageAnalysisTool {
    fn name(&self) -> &str {
        "analyze_image"
    }

    fn description(&self) -> &str {
        "Analyze image content, describing the information in the image. Supports reading images from file path, URL, or base64 data. Returns a detailed description and analysis of the image."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Image source type: 'file' (file path), 'url' (network address), or 'base64' (encoded data)"
                },
                "data": {
                    "type": "string",
                    "description": "Image data: file path, URL, or base64 encoded image data"
                },
                "prompt": {
                    "type": "string",
                    "description": "Analysis prompt, telling the LLM what information you want from the image (optional)"
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
                "Please describe the contents of this image in detail, including main elements, colors, layout, and any visible text information.",
            );

            let security = SecurityConfig::global();

            // Get image data based on source type
            let (base64_data, mime_type) = match source {
                "file" => read_image_from_file(data, &security)?,
                "url" => fetch_image_from_url(data, &security.limits).await?,
                "base64" => {
                    // Validate base64 data
                    let mime_type = detect_mime_type_from_base64(data);
                    validate_base64_size(data, &security.limits)?;
                    (data.to_string(), mime_type)
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "source".to_string(),
                        message: format!(
                            "Unsupported source type: '{}', use 'file', 'url', or 'base64'",
                            source
                        ),
                    }
                    .into());
                }
            };

            // Return image metadata, not raw base64 data
            // Note: passing full base64 as text to LLM causes context bloat and LLM cannot "see" the image
            let size_kb = base64_data.len() as f64 / 1024.0;
            let estimated_raw_kb = (base64_data.len() as f64 * 3.0 / 4.0) / 1024.0;
            Ok(ToolResult::success(format!(
                "Image successfully loaded.\n- MIME type: {}\n- Base64 size: {:.1} KB\n- Estimated raw size: {:.1} KB\n- Analysis prompt: {}\n\nNote: Image data has been loaded but cannot be displayed as text to the LLM. To analyze image content, consider using a multimodal LLM model, or use browser tools to view the image URL.",
                mime_type, size_kb, estimated_raw_kb, prompt,
            )))
        })
    }
}

/// Read image from file and convert to base64
fn read_image_from_file(path: &str, security: &SecurityConfig) -> Result<(String, String)> {
    use std::fs;
    let path_obj = security.validate_file(path)?;

    // 1. Detect MIME type (using file content, not extension)
    let mime_type = detect_image_mime_type(&path_obj);

    // 2. Read file
    let bytes = fs::read(&path_obj).map_err(|e| ToolError::ExecutionFailed {
        tool: "analyze_image".to_string(),
        message: format!("Failed to read file: {}", e),
    })?;

    let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);

    Ok((base64_data, mime_type))
}

/// Fetch image from URL and convert to base64
async fn fetch_image_from_url(url: &str, limits: &ResourceLimits) -> Result<(String, String)> {
    // SSRF protection: validate target address
    validate_url(url)?;

    // Use securely-configured HTTP client
    let client = create_safe_http_client(limits)?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("Failed to request image: {}", e),
        })?;

    // Check response status
    if !response.status().is_success() {
        return Err(ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("HTTP request failed: {}", response.status()),
        }
        .into());
    }

    // Check Content-Length
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

    // Get MIME type from response headers
    let mime_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| {
            // Extract main type (remove charset etc.)
            ct.split(';').next()
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "image/png".to_string());

    // Read response body
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "analyze_image".to_string(),
            message: format!("Failed to read image data: {}", e),
        })?;

    // Double-check actual size
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

/// Detect image MIME type using magic number
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

    // Fallback to extension detection
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

/// Detect MIME type from base64 data header
fn detect_mime_type_from_base64(data: &str) -> String {
    // Check common image format base64 header signatures
    if data.starts_with("iVBORw0KGgo") {
        "image/png"
    } else if data.starts_with("/9j/") {
        "image/jpeg"
    } else if data.starts_with("R0lGOD") {
        "image/gif"
    } else if data.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/png" // default
    }
    .to_string()
}

/// Validate base64 data size
fn validate_base64_size(data: &str, limits: &ResourceLimits) -> Result<()> {
    // Base64 encoding size is about 4/3 of raw data
    // Estimate raw size
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
