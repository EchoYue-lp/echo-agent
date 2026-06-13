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

/// Text search tool
pub struct TextSearchTool;

impl Tool for TextSearchTool {
    fn name(&self) -> &str {
        "search_text"
    }

    fn description(&self) -> &str {
        "Search content in text files, supports regular expressions."
    }

    fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
        vec![echo_core::tools::permission::ToolPermission::Read]
    }
    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
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

    fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
        vec![echo_core::tools::permission::ToolPermission::Write]
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

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::Tool;
    use std::io::Write;

    /// Helper to create a temporary file with content (unique per test)
    fn create_temp_file(content: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join("echo_tools_text_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test_{}_{}.txt", std::process::id(), id));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path.to_string_lossy().to_string()
    }

    fn cleanup_temp_file(path: &str) {
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_text_search_basic() {
        let path = create_temp_file("hello world\nfoo bar\nhello again\n");

        let tool = super::TextSearchTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("pattern".to_string(), serde_json::json!("hello"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["match_count"], 2);
        assert_eq!(output["pattern"], "hello");

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_search_case_insensitive() {
        let path = create_temp_file("Hello World\nhello world\nHELLO WORLD\n");

        let tool = super::TextSearchTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("pattern".to_string(), serde_json::json!("hello"));
        params.insert("ignore_case".to_string(), serde_json::json!(true));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["match_count"], 3);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_search_regex() {
        let path = create_temp_file("error: 404\nwarning: slow\nerror: 500\ninfo: ok\n");

        let tool = super::TextSearchTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("pattern".to_string(), serde_json::json!("error: \\d+"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["match_count"], 2);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_stats() {
        let path = create_temp_file("hello world\n你好世界\nthird line\n");

        let tool = super::TextStatsTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["lines"], 3);
        assert!(output["chinese_chars"].as_u64().unwrap() > 0);
        assert!(output["words"].as_u64().unwrap() > 0);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_process_unique() {
        let path = create_temp_file("apple\nbanana\napple\ncherry\nbanana\n");

        let tool = super::TextProcessTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("unique"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["original_lines"], 5);
        assert_eq!(output["result_lines"], 3);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_process_sort() {
        let path = create_temp_file("cherry\napple\nbanana\n");

        let tool = super::TextProcessTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("sort"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let preview = output["preview"].as_array().unwrap();
        assert_eq!(preview[0], "apple");
        assert_eq!(preview[1], "banana");
        assert_eq!(preview[2], "cherry");

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_process_head_tail() {
        let path = create_temp_file("line1\nline2\nline3\nline4\nline5\n");

        let tool = super::TextProcessTool;

        // Test head
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("head"));
        params.insert("count".to_string(), serde_json::json!(2));

        let result = tool.execute(params).await.unwrap();
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["result_lines"], 2);

        // Test tail
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("tail"));
        params.insert("count".to_string(), serde_json::json!(2));

        let result = tool.execute(params).await.unwrap();
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["result_lines"], 2);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_process_trim() {
        let path = create_temp_file("hello\n\n\nworld\n\n");

        let tool = super::TextProcessTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("trim"));

        let result = tool.execute(params).await.unwrap();
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["result_lines"], 2);

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_process_invalid_operation() {
        let path = create_temp_file("hello\n");

        let tool = super::TextProcessTool;
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!(path));
        params.insert("operation".to_string(), serde_json::json!("nonexistent"));

        let result = tool.execute(params).await;
        assert!(result.is_err());

        cleanup_temp_file(&path);
    }

    #[tokio::test]
    async fn test_text_search_missing_params() {
        let tool = super::TextSearchTool;

        // Missing file_path
        let params = std::collections::HashMap::new();
        let result = tool.execute(params).await;
        assert!(result.is_err());

        // Missing pattern
        let mut params = std::collections::HashMap::new();
        params.insert("file_path".to_string(), serde_json::json!("/tmp/test.txt"));
        let result = tool.execute(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_text_export() {
        let input_path = create_temp_file("cherry\napple\nbanana\n");
        let output_path = format!("{}_export", input_path);

        let tool = super::TextExportTool;
        let mut params = std::collections::HashMap::new();
        params.insert("input_file".to_string(), serde_json::json!(input_path));
        params.insert("output_file".to_string(), serde_json::json!(output_path));
        params.insert("operation".to_string(), serde_json::json!("sort"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);

        // Verify output file exists and is sorted
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.starts_with("apple"));
        assert!(content.contains("banana"));
        assert!(content.ends_with("cherry"));

        cleanup_temp_file(&input_path);
        cleanup_temp_file(&output_path);
    }
}
