//! 文本文件处理工具
//!
//! 提供文本文件读取和处理能力，支持：
//! - 读取各种文本格式文件
//! - 文本搜索和统计
//! - 编码检测

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{SecurityConfig, create_safe_regex};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "text_tools";

/// 文本文件读取工具
pub struct TextReadTool;

impl Tool for TextReadTool {
    fn name(&self) -> &str {
        "read_text"
    }

    fn description(&self) -> &str {
        "读取文本文件内容，支持各种文本格式。自动检测编码。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "文本文件的绝对路径"
                },
                "start_line": {
                    "type": "integer",
                    "description": "起始行号（默认 1）"
                },
                "line_count": {
                    "type": "integer",
                    "description": "读取行数（默认 100，-1 表示全部）"
                },
                "encoding": {
                    "type": "string",
                    "description": "文件编码（如 'utf-8', 'gbk'），默认自动检测"
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

            let start_line = parameters
                .get("start_line")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as usize; // 确保至少为 1

            let line_count = parameters
                .get("line_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(100);

            let _encoding = parameters.get("encoding").and_then(|v| v.as_str());

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // 读取文件
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取文件失败: {}", e),
            })?;

            // 尝试解码（优先 UTF-8，失败则尝试其他编码）
            let content = String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                // 尝试 GBK 解码
                encoding_rs::GBK.decode(&bytes).0.into_owned()
            });

            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            // 应用预览行数限制
            let max_preview = security.limits.max_preview_rows;
            let effective_line_count = if line_count < 0 {
                max_preview
            } else {
                (line_count as usize).min(max_preview)
            };

            // 计算读取范围
            let start = (start_line - 1).min(total_lines);
            let end = (start + effective_line_count).min(total_lines);

            // 格式化输出
            let mut result = Vec::new();
            result.push(format!("文件: {}", file_path));
            result.push(format!("总行数: {}", total_lines));
            result.push(String::new());

            if start > 0 {
                result.push(format!("--- 显示第 {}-{} 行 ---", start + 1, end));
            }

            for (idx, line) in lines[start..end].iter().enumerate() {
                result.push(format!("{:5} | {}", start + idx + 1, line));
            }

            if end < total_lines {
                result.push(format!("... (还有 {} 行)", total_lines - end));
            }

            Ok(ToolResult::success(result.join("\n")))
        })
    }
}

/// 文本搜索工具
pub struct TextSearchTool;

impl Tool for TextSearchTool {
    fn name(&self) -> &str {
        "search_text"
    }

    fn description(&self) -> &str {
        "在文本文件中搜索内容，支持正则表达式。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "文本文件的绝对路径"
                },
                "pattern": {
                    "type": "string",
                    "description": "搜索模式（支持正则表达式）"
                },
                "context": {
                    "type": "integer",
                    "description": "显示匹配行前后的上下文行数（默认 0）"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "是否忽略大小写（默认 false）"
                }
            },
            "required": ["file_path", "pattern"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let pattern = parameters
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("pattern".to_string()))?;

            let context = parameters
                .get("context")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let ignore_case = parameters
                .get("ignore_case")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // 读取文件
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取文件失败: {}", e),
            })?;

            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // 使用安全的正则表达式构建
            let re = if ignore_case {
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .size_limit(security.limits.regex_max_size)
                    .dfa_size_limit(security.limits.regex_max_size)
                    .build()
                    .map_err(|e| ToolError::InvalidParameter {
                        name: "pattern".to_string(),
                        message: format!("无效的正则表达式: {}", e),
                    })?
            } else {
                create_safe_regex(pattern, &security.limits)?
            };

            let lines: Vec<&str> = content.lines().collect();
            let mut matches = Vec::new();
            let mut match_count = 0;

            // 限制匹配数量
            let max_matches = security.limits.max_preview_rows;

            for (idx, line) in lines.iter().enumerate() {
                if match_count >= max_matches {
                    break;
                }

                if re.is_match(line) {
                    match_count += 1;

                    // 添加上下文
                    if context > 0 {
                        let start = idx.saturating_sub(context);
                        let end = (idx + context + 1).min(lines.len());

                        matches.push(String::new());
                        for (i, context_line) in lines[start..end].iter().enumerate() {
                            let line_idx = start + i;
                            let prefix = if line_idx == idx { ">>>" } else { "   " };
                            matches.push(format!(
                                "{} {:5} | {}",
                                prefix,
                                line_idx + 1,
                                context_line
                            ));
                        }
                    } else {
                        matches.push(format!("{:5} | {}", idx + 1, line));
                    }
                }
            }

            let result = if matches.is_empty() {
                format!("未找到匹配: '{}'", pattern)
            } else {
                let more = if match_count >= max_matches {
                    format!("\n(已达到最大匹配数 {})", max_matches)
                } else {
                    String::new()
                };
                format!(
                    "搜索结果: 共 {} 处匹配\n\n{}{}",
                    match_count,
                    matches.join("\n"),
                    more
                )
            };

            Ok(ToolResult::success(result))
        })
    }
}

/// 文本统计工具
pub struct TextStatsTool;

impl Tool for TextStatsTool {
    fn name(&self) -> &str {
        "text_stats"
    }

    fn description(&self) -> &str {
        "统计文本文件的信息：行数、字数、字符数等。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "文本文件的绝对路径"
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
            let path = security.validate_file(file_path)?;

            // 读取文件
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取文件失败: {}", e),
            })?;

            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // 统计
            let lines = content.lines().count();
            let chars = content.chars().count();
            let words = content.split_whitespace().count();
            let chinese_chars = content
                .chars()
                .filter(|c| '\u{4E00}' <= *c && *c <= '\u{9FFF}')
                .count();
            let english_words = content
                .split(|c: char| !c.is_ascii_alphabetic())
                .filter(|s| s.len() >= 2)
                .count();

            let mut stats = Vec::new();
            stats.push(format!("文件: {}", file_path));
            stats.push(String::new());
            stats.push("文本统计:".to_string());
            stats.push(format!("  行数: {}", lines));
            stats.push(format!("  字符数: {}", chars));
            stats.push(format!("  单词/词组数: {}", words));
            stats.push(format!("  中文字符: {}", chinese_chars));
            stats.push(format!("  英文单词: {}", english_words));

            // 文件大小
            if let Ok(metadata) = std::fs::metadata(&path) {
                let size_kb = metadata.len() as f64 / 1024.0;
                stats.push(format!("  文件大小: {:.2} KB", size_kb));
            }

            // 行长度统计
            let line_lengths: Vec<usize> = content.lines().map(|l| l.len()).collect();
            if !line_lengths.is_empty() {
                let avg_len = line_lengths.iter().sum::<usize>() as f64 / line_lengths.len() as f64;
                let max_len = line_lengths.iter().max().unwrap_or(&0);
                stats.push(format!("  平均行长: {:.1} 字符", avg_len));
                stats.push(format!("  最长行: {} 字符", max_len));
            }

            Ok(ToolResult::success(stats.join("\n")))
        })
    }
}

/// 文本处理工具
pub struct TextProcessTool;

impl Tool for TextProcessTool {
    fn name(&self) -> &str {
        "process_text"
    }

    fn description(&self) -> &str {
        "对文本进行处理：提取行、合并、去重等操作。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "文本文件的绝对路径"
                },
                "operation": {
                    "type": "string",
                    "description": "操作类型：'unique'（去重）、'sort'（排序）、'reverse'（反转行）、'trim'（去除空白行）、'head'（前N行）、'tail'（后N行）"
                },
                "count": {
                    "type": "integer",
                    "description": "用于 head/tail 操作的行数"
                }
            },
            "required": ["file_path", "operation"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let operation = parameters
                .get("operation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("operation".to_string()))?;

            let count = parameters
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // 读取文件
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取文件失败: {}", e),
            })?;

            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            let mut lines: Vec<&str> = content.lines().collect();
            let original_count = lines.len();
            let max_preview = security.limits.max_preview_rows;

            match operation {
                "unique" => {
                    use std::collections::HashSet;
                    let mut seen = HashSet::new();
                    lines.retain(|line| seen.insert(*line));
                }
                "sort" => {
                    lines.sort();
                }
                "reverse" => {
                    lines.reverse();
                }
                "trim" => {
                    lines.retain(|line| !line.trim().is_empty());
                }
                "head" => {
                    lines = lines.into_iter().take(count.min(max_preview)).collect();
                }
                "tail" => {
                    let start = lines.len().saturating_sub(count.min(max_preview));
                    lines = lines.into_iter().skip(start).collect();
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "operation".to_string(),
                        message: format!("不支持的操作: '{}'", operation),
                    }
                    .into());
                }
            }

            let preview_lines = lines.iter().take(max_preview).cloned().collect::<Vec<_>>();
            let result = format!(
                "操作: {}\n原始行数: {}\n结果行数: {}\n\n{}",
                operation,
                original_count,
                lines.len(),
                preview_lines.join("\n")
            );

            Ok(ToolResult::success(result))
        })
    }
}

/// 文本导出工具
pub struct TextExportTool;

impl Tool for TextExportTool {
    fn name(&self) -> &str {
        "export_text"
    }

    fn description(&self) -> &str {
        "将处理后的文本导出到新文件。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "输入文本文件路径"
                },
                "output_file": {
                    "type": "string",
                    "description": "输出文件路径"
                },
                "operation": {
                    "type": "string",
                    "description": "可选操作：'unique'、'sort'、'trim' 等"
                }
            },
            "required": ["input_file", "output_file"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let input_file = parameters
                .get("input_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("input_file".to_string()))?;

            let output_file = parameters
                .get("output_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("output_file".to_string()))?;

            let operation = parameters.get("operation").and_then(|v| v.as_str());

            let security = SecurityConfig::global();
            let path = security.validate_file(input_file)?;

            // 读取文件
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取文件失败: {}", e),
            })?;

            let mut content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // 执行操作
            if let Some(op) = operation {
                let mut lines: Vec<&str> = content.lines().collect();
                match op {
                    "unique" => {
                        use std::collections::HashSet;
                        let mut seen = HashSet::new();
                        lines.retain(|line| seen.insert(*line));
                    }
                    "sort" => lines.sort(),
                    "trim" => lines.retain(|line| !line.trim().is_empty()),
                    _ => {}
                }
                content = lines.join("\n");
            }

            // 创建输出目录
            let output_path = security.validate_output_file(output_file)?;
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("创建输出目录失败: {}", e),
                })?;
            }

            // 写入文件
            std::fs::write(output_path, content).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("写入文件失败: {}", e),
            })?;

            Ok(ToolResult::success(format!(
                "文本已导出: {} -> {}",
                input_file, output_file
            )))
        })
    }
}
