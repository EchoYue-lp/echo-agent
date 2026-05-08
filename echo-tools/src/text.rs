//! Text file processing tools
//!
//! Provides text file reading and processing capabilities, supporting:
//! - Reading various text format files
//! - Text search and statistics
//! - Encoding detection

use futures::future::BoxFuture;
use serde_json::Value;

use crate::security::{SecurityConfig, create_safe_regex};
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "text_tools";

/// Text file reading tool
pub struct TextReadTool;

impl Tool for TextReadTool {
    fn name(&self) -> &str {
        "read_text"
    }

    fn description(&self) -> &str {
        "Read text file content, supports various text formats. Auto-detects encoding."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the text file"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line number (default 1)"
                },
                "line_count": {
                    "type": "integer",
                    "description": "Number of lines to read (default 100, -1 means all)"
                },
                "encoding": {
                    "type": "string",
                    "description": "File encoding (e.g. 'utf-8', 'gbk'), default auto-detect"
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
                .max(1) as usize; // Ensure it's at least 1

            let line_count = parameters
                .get("line_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(100);

            let _encoding = parameters.get("encoding").and_then(|v| v.as_str());

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // Read file
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read file: {}", e),
            })?;

            // Try to decode (prefer UTF-8, fall back to other encodings)
            let content = String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                // Try GBK decoding
                encoding_rs::GBK.decode(&bytes).0.into_owned()
            });

            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            // Apply preview row limit
            let max_preview = security.limits.max_preview_rows;
            let effective_line_count = if line_count < 0 {
                max_preview
            } else {
                (line_count as usize).min(max_preview)
            };

            // Calculate read range
            let start = (start_line - 1).min(total_lines);
            let end = (start + effective_line_count).min(total_lines);

            // Structured output
            let preview_lines_data: Vec<Value> = lines[start..end]
                .iter()
                .enumerate()
                .map(|(idx, line)| {
                    serde_json::json!({
                        "line_number": start + idx + 1,
                        "content": line,
                    })
                })
                .collect();

            let result = serde_json::json!({
                "file": file_path,
                "total_lines": total_lines,
                "start_line": start + 1,
                "end_line": end,
                "truncated": end < total_lines,
                "remaining_lines": total_lines.saturating_sub(end),
                "lines": preview_lines_data,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

/// Text search tool
pub struct TextSearchTool;

impl Tool for TextSearchTool {
    fn name(&self) -> &str {
        "search_text"
    }

    fn description(&self) -> &str {
        "Search content in text files, supports regular expressions."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the text file"
                },
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (supports regular expressions)"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines before and after matches (default 0)"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Whether to ignore case (default false)"
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

            // Read file
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read file: {}", e),
            })?;

            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // Build safe regex
            let re = if ignore_case {
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .size_limit(security.limits.regex_max_size)
                    .dfa_size_limit(security.limits.regex_max_size)
                    .build()
                    .map_err(|e| ToolError::InvalidParameter {
                        name: "pattern".to_string(),
                        message: format!("Invalid regex: {}", e),
                    })?
            } else {
                create_safe_regex(pattern, &security.limits)?
            };

            let lines: Vec<&str> = content.lines().collect();
            let mut matches = Vec::new();
            let mut match_count = 0;

            // Limit match count
            let max_matches = security.limits.max_preview_rows;

            for (idx, line) in lines.iter().enumerate() {
                if match_count >= max_matches {
                    break;
                }

                if re.is_match(line) {
                    match_count += 1;

                    // Add context lines
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

            let result = serde_json::json!({
                "file": file_path,
                "pattern": pattern,
                "match_count": match_count,
                "truncated": match_count >= max_matches,
                "max_matches": max_matches,
                "matches": matches,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

/// Text statistics tool
pub struct TextStatsTool;

impl Tool for TextStatsTool {
    fn name(&self) -> &str {
        "text_stats"
    }

    fn description(&self) -> &str {
        "Statistics for text files: line count, word count, character count, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the text file"
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

            // Read file
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read file: {}", e),
            })?;

            let content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // Statistics
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

            let line_lengths: Vec<usize> = content.lines().map(|l| l.len()).collect();
            let avg_line_len = if !line_lengths.is_empty() {
                Some(line_lengths.iter().sum::<usize>() as f64 / line_lengths.len() as f64)
            } else {
                None
            };
            let max_line_len = line_lengths.iter().max().copied();

            let file_size_kb = std::fs::metadata(&path)
                .ok()
                .map(|m| m.len() as f64 / 1024.0);

            let result = serde_json::json!({
                "file": file_path,
                "lines": lines,
                "chars": chars,
                "words": words,
                "chinese_chars": chinese_chars,
                "english_words": english_words,
                "file_size_kb": file_size_kb,
                "avg_line_len": avg_line_len,
                "max_line_len": max_line_len,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

/// Text processing tool
pub struct TextProcessTool;

impl Tool for TextProcessTool {
    fn name(&self) -> &str {
        "process_text"
    }

    fn description(&self) -> &str {
        "Process text: extract, merge, deduplicate lines, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the text file"
                },
                "operation": {
                    "type": "string",
                    "description": "Operation type: 'unique' (deduplicate), 'sort' (sort), 'reverse' (reverse lines), 'trim' (remove blank lines), 'head' (first N lines), 'tail' (last N lines)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of lines for head/tail operations"
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

            // Read file
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read file: {}", e),
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
                        message: format!("Unsupported operation: '{}'", operation),
                    }
                    .into());
                }
            }

            let preview_lines: Vec<&str> = lines.iter().take(max_preview).copied().collect();
            let result = serde_json::json!({
                "file": file_path,
                "operation": operation,
                "original_lines": original_count,
                "result_lines": lines.len(),
                "preview": preview_lines,
                "truncated": lines.len() > max_preview,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

/// Text export tool
pub struct TextExportTool;

impl Tool for TextExportTool {
    fn name(&self) -> &str {
        "export_text"
    }

    fn description(&self) -> &str {
        "Export processed text to a new file."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "Input text file path"
                },
                "output_file": {
                    "type": "string",
                    "description": "Output file path"
                },
                "operation": {
                    "type": "string",
                    "description": "Optional operation: 'unique', 'sort', 'trim', etc."
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

            // Read file
            let bytes = std::fs::read(&path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read file: {}", e),
            })?;

            let mut content = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| encoding_rs::GBK.decode(&bytes).0.into_owned());

            // Execute operation
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

            // Create output directory
            let output_path = security.validate_output_file(output_file)?;
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create output directory: {}", e),
                })?;
            }

            // Write file
            std::fs::write(output_path, content).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to write file: {}", e),
            })?;

            Ok(ToolResult::success(format!(
                "Text exported: {} -> {}",
                input_file, output_file
            )))
        })
    }
}
