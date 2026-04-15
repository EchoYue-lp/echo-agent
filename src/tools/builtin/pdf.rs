//! PDF 文档处理工具
//!
//! 提供 PDF 文本提取能力，支持：
//! - 提取全部文本内容
//! - 提取指定页面范围
//! - 获取文档元数据（标题、作者、页数等）

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{ResourceLimits, SecurityConfig};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "pdf_tools";

/// 验证文件路径并返回规范化的路径
fn validate_file_path(path_str: &str, limits: &ResourceLimits) -> Result<std::path::PathBuf> {
    let path = std::path::Path::new(path_str);

    // 1. 检查是否为绝对路径
    if !path.is_absolute() {
        return Err(ToolError::InvalidPath {
            path: path_str.to_string(),
            reason: "路径必须是绝对路径".to_string(),
        }
        .into());
    }

    // 2. 检查文件是否存在
    if !path.exists() {
        return Err(ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: "文件不存在".to_string(),
        }
        .into());
    }

    // 3. 检查文件大小
    let metadata = std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed {
        tool: TOOL_NAME.to_string(),
        message: format!("获取文件信息失败: {}", e),
    })?;

    if metadata.len() > limits.max_file_size {
        return Err(ToolError::FileTooLarge {
            size: metadata.len(),
            max: limits.max_file_size,
        }
        .into());
    }

    Ok(path.to_path_buf())
}

/// PDF 文本提取工具
pub struct PdfExtractTool;

impl Tool for PdfExtractTool {
    fn name(&self) -> &str {
        "extract_pdf"
    }

    fn description(&self) -> &str {
        "从 PDF 文档中提取文本内容。支持提取全部文本、指定页面范围或获取文档元数据。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "PDF 文件的绝对路径"
                },
                "pages": {
                    "type": "string",
                    "description": "要提取的页面范围（可选），如 '1-5'、'1,3,7' 或 'all'（默认）"
                },
                "extract_metadata": {
                    "type": "boolean",
                    "description": "是否同时提取文档元数据（默认 false）"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let pages = parameters
                .get("pages")
                .and_then(|v| v.as_str())
                .unwrap_or("all");

            let extract_metadata = parameters
                .get("extract_metadata")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let security = SecurityConfig::global();
            let path = validate_file_path(file_path, &security.limits)?;

            // 使用 lopdf 打开文档
            let pdf = lopdf::Document::load(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("打开 PDF 失败: {}", e),
            })?;

            // 获取页数
            let total_pages = pdf.get_pages().len();

            // 提取元数据
            let metadata_str = if extract_metadata {
                extract_pdf_metadata(&pdf)?
            } else {
                String::new()
            };

            // 解析页面范围
            let page_numbers = parse_page_range(pages, total_pages, &security.limits)?;

            // 提取指定页面的文本
            let text_content = extract_pages_text(&pdf, &page_numbers, &security.limits)?;

            // 构建结果
            let result = if extract_metadata {
                format!(
                    "=== PDF 元数据 ===\n{}\n\n=== 文本内容 (第 {} 页，共 {} 页) ===\n{}",
                    metadata_str, pages, total_pages, text_content
                )
            } else {
                format!(
                    "=== 文本内容 (第 {} 页，共 {} 页) ===\n{}",
                    pages, total_pages, text_content
                )
            };

            Ok(ToolResult::success(result))
        })
    }
}

/// PDF 信息工具（获取文档概览）
pub struct PdfInfoTool;

impl Tool for PdfInfoTool {
    fn name(&self) -> &str {
        "pdf_info"
    }

    fn description(&self) -> &str {
        "获取 PDF 文档的基本信息：页数、标题、作者、创建时间等元数据，不提取文本内容。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "PDF 文件的绝对路径"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let security = SecurityConfig::global();
            let path = validate_file_path(file_path, &security.limits)?;

            let pdf = lopdf::Document::load(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("打开 PDF 失败: {}", e),
            })?;

            let metadata = extract_pdf_metadata(&pdf)?;

            Ok(ToolResult::success(metadata))
        })
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────

/// 提取 PDF 元数据
fn extract_pdf_metadata(pdf: &lopdf::Document) -> Result<String> {
    use lopdf::Object;

    let mut info = Vec::new();

    info.push(format!("页数: {}", pdf.get_pages().len()));

    // 尝试从 trailer 获取元数据
    if let Ok(trailer) = pdf.trailer.get(b"Info")
        && let Object::Dictionary(dict) = trailer
    {
        for (key, value) in dict.iter() {
            let key_str = match key.as_slice() {
                b"Title" => "标题",
                b"Author" => "作者",
                b"Subject" => "主题",
                b"Creator" => "创建工具",
                b"Producer" => "PDF 生成器",
                b"CreationDate" => "创建时间",
                b"ModDate" => "修改时间",
                other => std::str::from_utf8(other).unwrap_or("未知"),
            };

            let value_str = match value {
                Object::String(s, _) => {
                    // PDF 日期格式转换
                    if key.as_slice() == b"CreationDate" || key.as_slice() == b"ModDate" {
                        parse_pdf_date(s)
                    } else {
                        String::from_utf8_lossy(s).to_string()
                    }
                }
                Object::Name(n) => String::from_utf8_lossy(n).to_string(),
                Object::Integer(i) => i.to_string(),
                Object::Real(f) => f.to_string(),
                Object::Boolean(b) => b.to_string(),
                _ => "未知".to_string(),
            };

            info.push(format!("{}: {}", key_str, value_str));
        }
    }

    // 获取页数作为基本信息
    info.push(format!("总页数: {}", pdf.get_pages().len()));

    Ok(info.join("\n"))
}

/// 解析 PDF 日期格式
fn parse_pdf_date(date: &[u8]) -> String {
    // PDF 日期格式: D:YYYYMMDDHHmmSS
    let date_str = String::from_utf8_lossy(date);
    if let Some(rest) = date_str.strip_prefix("D:")
        && rest.len() >= 8
    {
        let year = &rest[0..4];
        let month = &rest[4..6];
        let day = &rest[6..8];
        return format!("{}-{}-{}", year, month, day);
    }
    date_str.to_string()
}

/// 解析页面范围字符串
fn parse_page_range(range: &str, total_pages: usize, limits: &ResourceLimits) -> Result<Vec<u32>> {
    if range == "all" {
        // 限制最大预览页数
        let max_pages = limits.max_preview_pages.min(total_pages);
        return Ok((1..=max_pages as u32).collect());
    }

    let mut pages = Vec::new();

    // 处理逗号分隔的单页
    for part in range.split(',') {
        if part.contains('-') {
            // 处理范围
            let bounds: Vec<&str> = part.split('-').collect();
            if bounds.len() != 2 {
                return Err(ToolError::InvalidParameter {
                    name: "pages".to_string(),
                    message: format!("无效的页面范围: {}", part),
                }
                .into());
            }

            let start: u32 = bounds[0].parse().map_err(|_| ToolError::InvalidParameter {
                name: "pages".to_string(),
                message: format!("无效的起始页: {}", bounds[0]),
            })?;

            let end: u32 = bounds[1].parse().map_err(|_| ToolError::InvalidParameter {
                name: "pages".to_string(),
                message: format!("无效的结束页: {}", bounds[1]),
            })?;

            if start > end || end > total_pages as u32 {
                return Err(ToolError::InvalidParameter {
                    name: "pages".to_string(),
                    message: format!("页面范围无效或超出文档页数 ({} 页)", total_pages),
                }
                .into());
            }

            // 限制提取页数不超过 max_preview_pages
            let limited_end = (end - start + 1).min(limits.max_preview_pages as u32);
            for p in start..(start + limited_end) {
                if !pages.contains(&p) {
                    pages.push(p);
                }
            }
        } else {
            // 单页
            let page: u32 = part.parse().map_err(|_| ToolError::InvalidParameter {
                name: "pages".to_string(),
                message: format!("无效的页码: {}", part),
            })?;

            if page > total_pages as u32 {
                return Err(ToolError::InvalidParameter {
                    name: "pages".to_string(),
                    message: format!("页码 {} 超出文档页数 ({} 页)", page, total_pages),
                }
                .into());
            }

            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }

    // 检查总页数限制
    if pages.len() > limits.max_preview_pages {
        pages = pages.into_iter().take(limits.max_preview_pages).collect();
    }

    pages.sort();
    Ok(pages)
}

/// 提取指定页面的文本内容
fn extract_pages_text(
    pdf: &lopdf::Document,
    page_numbers: &[u32],
    limits: &ResourceLimits,
) -> Result<String> {
    use lopdf::Object;

    let mut all_text = Vec::new();
    let mut total_chars = 0;

    for page_num in page_numbers {
        if total_chars >= limits.max_preview_chars {
            all_text.push(format!(
                "... (已达到最大预览字符数 {})",
                limits.max_preview_chars
            ));
            break;
        }

        let page_id = *pdf.get_pages().get(page_num).unwrap_or(&(0, 0));

        if let Ok(page_obj) = pdf.get_object(page_id)
            && let Object::Dictionary(dict) = page_obj
        {
            // 获取页面内容流
            if let Ok(contents_ref) = dict.get(b"Contents") {
                let content_stream: Option<lopdf::Stream> = match contents_ref {
                    Object::Reference(id) => pdf.get_object(*id).ok().and_then(|obj| {
                        if let Object::Stream(stream) = obj {
                            Some(stream.clone())
                        } else {
                            None
                        }
                    }),
                    Object::Array(arr) => {
                        // 多个内容流，合并处理
                        let mut combined = Vec::new();
                        for obj_ref in arr.iter() {
                            if let Object::Reference(id) = obj_ref
                                && let Ok(obj) = pdf.get_object(*id)
                                && let Object::Stream(stream) = obj
                            {
                                combined.extend_from_slice(&stream.content);
                            }
                        }
                        // 解析合并后的内容
                        let text = extract_text_from_stream(&combined, limits);
                        total_chars += text.len();
                        all_text.push(format!("--- 第 {} 页 ---\n{}", page_num, text));
                        continue;
                    }
                    Object::Stream(stream) => Some(stream.clone()),
                    _ => None,
                };

                if let Some(stream) = content_stream {
                    let text = extract_text_from_stream(&stream.content, limits);
                    total_chars += text.len();
                    all_text.push(format!("--- 第 {} 页 ---\n{}", page_num, text));
                }
            }
        }
    }

    Ok(all_text.join("\n\n"))
}

/// 从 PDF 内容流中提取文本
fn extract_text_from_stream(content: &[u8], limits: &ResourceLimits) -> String {
    // 简化的文本提取：查找 Tj 和 TJ 操作符
    let content_str = String::from_utf8_lossy(content);
    let mut text_parts = Vec::new();

    // 使用安全正则表达式，限制大小
    let tj_regex = regex::RegexBuilder::new(r"\(([^)]*)\)\s*Tj")
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .unwrap();
    for cap in tj_regex.captures_iter(&content_str) {
        if let Some(text) = cap.get(1) {
            text_parts.push(text.as_str().to_string());
        }
    }

    // 匹配 <...>Tj 格式的十六进制文本
    let hex_regex = regex::RegexBuilder::new(r"<([0-9a-fA-F]*)>\s*Tj")
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .unwrap();
    for cap in hex_regex.captures_iter(&content_str) {
        if let Some(hex) = cap.get(1) {
            // 尝试解码十六进制为文本
            if let Ok(decoded) = hex_decode(hex.as_str()) {
                text_parts.push(decoded);
            }
        }
    }

    // 匹配数组格式 [... TJ]
    let tj_array_regex = regex::RegexBuilder::new(r"\[(.*?)\]\s*TJ")
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .unwrap();
    let str_regex = regex::RegexBuilder::new(r"\(([^)]*)\)")
        .size_limit(limits.regex_max_size)
        .dfa_size_limit(limits.regex_max_size)
        .build()
        .unwrap();
    for cap in tj_array_regex.captures_iter(&content_str) {
        if let Some(arr_content) = cap.get(1) {
            for str_cap in str_regex.captures_iter(arr_content.as_str()) {
                if let Some(s) = str_cap.get(1) {
                    text_parts.push(s.as_str().to_string());
                }
            }
        }
    }

    // 限制输出长度
    let result = text_parts.join(" ");
    if result.len() > limits.max_preview_chars {
        result.chars().take(limits.max_preview_chars).collect()
    } else {
        result
    }
}

/// 解码 PDF 十六进制字符串
fn hex_decode(hex: &str) -> Result<String> {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i.min(i + 2)], 16).unwrap_or(0))
        .collect();

    Ok(String::from_utf8_lossy(&bytes).to_string())
}
