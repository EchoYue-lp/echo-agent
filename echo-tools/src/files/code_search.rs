//! Code Search tool — search for code across the project
//!
//! Primary backend: ripgrep (`rg`) with full flag support and JSON output parsing.
//! Fallback: built-in symbol search using language-specific regex patterns.
//!
//! When `rg` is available on PATH, the tool supports:
//! - `--glob` / `--type` for file filtering
//! - `-i` (case insensitive), `-F` (fixed strings), `-w` (word regexp)
//! - `-C` / `-A` / `-B` for context lines
//! - `-m` for max matches per file
//! - Total result count limiting via `max_results`

use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;

/// Maximum bytes of output before truncation (prevents token overflow).
const MAX_OUTPUT_BYTES: usize = 50_000;

// ── CodeSearchTool ──────────────────────────────────────────────────────────

/// Search for code patterns across the project.
///
/// Uses ripgrep (`rg`) when available for fast, feature-rich searching.
/// Falls back to built-in symbol search when `rg` is not on PATH.
pub struct CodeSearchTool {
    base_dir: Option<PathBuf>,
}

impl CodeSearchTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for CodeSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "code_search"
    }

    fn description(&self) -> &str {
        "Search for code patterns across the project using ripgrep. \
         Supports regex, literal strings, glob filters, file type filters, \
         context lines, and case-insensitive matching. \
         Falls back to symbol search (functions, classes, types) if ripgrep is unavailable."
    }

    fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
        vec![echo_core::tools::permission::ToolPermission::Read]
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search pattern (regex by default, literal if fixed_strings=true)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "File glob filter (e.g., '*.rs', '*.py')"
                },
                "file_type": {
                    "type": "string",
                    "description": "File type filter recognized by ripgrep (e.g., 'rust', 'python', 'js')"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive search (-i flag)",
                    "default": false
                },
                "fixed_strings": {
                    "type": "boolean",
                    "description": "Treat pattern as literal string, not regex (-F flag)",
                    "default": false
                },
                "word_regexp": {
                    "type": "boolean",
                    "description": "Only match whole words (-w flag)",
                    "default": false
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines to show around each match (-C N)"
                },
                "max_count": {
                    "type": "integer",
                    "description": "Maximum matches per file (-m N)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum total results to return (default: 50)"
                },
                "symbol": {
                    "type": "string",
                    "description": "(Legacy) Symbol name or pattern — used as fallback query"
                },
                "symbol_type": {
                    "type": "string",
                    "enum": ["function", "class", "struct", "enum", "trait", "interface", "type", "method", "any"],
                    "description": "(Legacy) Type of symbol to search for (default: any)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            // Parse new-style query, falling back to legacy "symbol" parameter
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .or_else(|| parameters.get("symbol").and_then(|v| v.as_str()))
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let glob_pattern = parameters.get("glob").and_then(|v| v.as_str());

            let file_type = parameters.get("file_type").and_then(|v| v.as_str());

            let case_insensitive = parameters
                .get("case_insensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let fixed_strings = parameters
                .get("fixed_strings")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let word_regexp = parameters
                .get("word_regexp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let context = parameters
                .get("context")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);

            let max_count = parameters
                .get("max_count")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);

            let max_results = parameters
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(50) as usize;

            // Resolve search path
            let search_path = super::resolve_path("code_search", path_str, &self.base_dir)?;

            if !search_path.exists() {
                return Err(ToolError::ExecutionFailed {
                    tool: "code_search".to_string(),
                    message: format!("Path does not exist: {}", search_path.display()),
                }
                .into());
            }

            // Try ripgrep first; fall back to built-in symbol search
            match try_ripgrep_search(
                &search_path,
                query,
                glob_pattern,
                file_type,
                case_insensitive,
                fixed_strings,
                word_regexp,
                context,
                max_count,
                max_results,
            )
            .await
            {
                Ok(output) => Ok(ToolResult::success(output)),
                Err(RgError::NotAvailable) => {
                    // Fallback to built-in symbol search
                    let symbol_type = parameters
                        .get("symbol_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("any");

                    let file_filter = glob_pattern.map(|g| glob_to_regex(g));

                    let results = search_symbols(
                        &search_path,
                        query,
                        symbol_type,
                        file_filter.as_ref(),
                        max_results,
                    )
                    .await?;

                    if results.is_empty() {
                        return Ok(ToolResult::success(format!(
                            "No {} symbols matching '{}' found in {}",
                            symbol_type,
                            query,
                            search_path.display()
                        )));
                    }

                    let mut output = format!(
                        "Found {} {} symbol(s) matching '{}' (fallback mode, rg not available):\n\n",
                        results.len(),
                        symbol_type,
                        query
                    );

                    for result in &results {
                        output.push_str(&format!(
                            "{}:{} - {} {}\n",
                            result.file.display(),
                            result.line,
                            result.symbol_type,
                            result.name
                        ));
                        if let Some(ref ctx) = result.context {
                            output.push_str(&format!("  {}\n", ctx.trim()));
                        }
                    }

                    Ok(ToolResult::success(output))
                }
                Err(RgError::Failed(msg)) => Err(ToolError::ExecutionFailed {
                    tool: "code_search".to_string(),
                    message: msg,
                }
                .into()),
            }
        })
    }
}

// ── Ripgrep backend ─────────────────────────────────────────────────────────

/// Internal error type for ripgrep operations.
enum RgError {
    /// `rg` binary not found on PATH — triggers fallback.
    NotAvailable,
    /// `rg` ran but failed with an error.
    Failed(String),
}

/// Attempt to search using ripgrep (`rg --json`).
///
/// Returns `Err(RgError::NotAvailable)` when the binary is not found,
/// signaling the caller to fall back to the built-in search.
async fn try_ripgrep_search(
    search_path: &Path,
    query: &str,
    glob: Option<&str>,
    file_type: Option<&str>,
    case_insensitive: bool,
    fixed_strings: bool,
    word_regexp: bool,
    context: Option<u32>,
    max_count: Option<u32>,
    max_results: usize,
) -> std::result::Result<String, RgError> {
    // Build rg command arguments
    let mut args: Vec<String> = vec!["--json".to_string()];

    if case_insensitive {
        args.push("-i".to_string());
    }
    if fixed_strings {
        args.push("-F".to_string());
    }
    if word_regexp {
        args.push("-w".to_string());
    }
    if let Some(ctx) = context {
        args.push("-C".to_string());
        args.push(ctx.to_string());
    }
    if let Some(mc) = max_count {
        args.push("-m".to_string());
        args.push(mc.to_string());
    }
    if let Some(g) = glob {
        args.push("--glob".to_string());
        args.push(g.to_string());
    }
    if let Some(ft) = file_type {
        args.push("--type".to_string());
        args.push(ft.to_string());
    }

    // The search pattern
    args.push(query.to_string());

    // Search path
    args.push(search_path.to_string_lossy().to_string());

    // Execute rg
    let output = Command::new("rg")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RgError::NotAvailable
            } else {
                RgError::Failed(format!("Failed to execute rg: {}", e))
            }
        })?;

    // rg exits with code 1 when no matches found — that is not an error
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check for real errors (exit code 2 = error)
    if let Some(code) = output.status.code() {
        if code == 2 {
            return Err(RgError::Failed(format!(
                "rg error: {}",
                stderr.trim()
            )));
        }
    }

    // Parse JSON output
    let results = parse_rg_json(&stdout, max_results);

    if results.is_empty() {
        return Ok(format!(
            "No matches found for '{}' in {}",
            query,
            search_path.display()
        ));
    }

    // Format output
    let mut output = format!(
        "Found {} match(es) for '{}' in {}:\n\n",
        results.len(),
        query,
        search_path.display()
    );

    for entry in &results {
        output.push_str(&format!(
            "{}:{}: {}\n",
            entry.path, entry.line_number, entry.text
        ));
        // Include context lines if present
        for ctx_line in &entry.context_lines {
            output.push_str(&format!("  {}\n", ctx_line));
        }
    }

    // Truncate to prevent token overflow
    if output.len() > MAX_OUTPUT_BYTES {
        output.truncate(MAX_OUTPUT_BYTES);
        output.push_str("\n\n... [output truncated to prevent overflow]");
    }

    Ok(output)
}

/// A single result from ripgrep JSON output.
struct RgResult {
    path: String,
    line_number: usize,
    text: String,
    context_lines: Vec<String>,
}

/// Parse ripgrep `--json` output into structured results.
///
/// Each line of stdout is a JSON object with a `type` field.
/// We extract `match` events (containing path, line_number, lines).
fn parse_rg_json(stdout: &str, max_results: usize) -> Vec<RgResult> {
    let mut results = Vec::new();

    for line in stdout.lines() {
        if results.len() >= max_results {
            break;
        }

        let obj: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match event_type {
            "match" => {
                let data = match obj.get("data") {
                    Some(d) => d,
                    None => continue,
                };

                let path = data
                    .get("path")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("<unknown>")
                    .to_string();

                let line_number = data
                    .get("line_number")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0) as usize;

                let text = data
                    .get("lines")
                    .and_then(|l| l.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim_end()
                    .to_string();

                // Extract context lines if present
                let context_lines = data
                    .get("context")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|ctx| {
                                let ctx_line = ctx
                                    .get("lines")
                                    .and_then(|l| l.get("text"))
                                    .and_then(|t| t.as_str())?
                                    .trim_end()
                                    .to_string();
                                let ctx_num =
                                    ctx.get("line_number").and_then(|n| n.as_u64()).unwrap_or(0);
                                Some(format!("{}: {}", ctx_num, ctx_line))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                results.push(RgResult {
                    path,
                    line_number,
                    text,
                    context_lines,
                });
            }
            "context" => {
                // Context-only lines (from -A/-B/-C); attach to previous result if possible
                let data = match obj.get("data") {
                    Some(d) => d,
                    None => continue,
                };

                let ctx_text = data
                    .get("lines")
                    .and_then(|l| l.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim_end()
                    .to_string();

                let ctx_num = data
                    .get("line_number")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);

                if let Some(last) = results.last_mut() {
                    last.context_lines
                        .push(format!("{}- {}", ctx_num, ctx_text));
                }
            }
            _ => {
                // Ignore "begin", "end", "summary" events
            }
        }
    }

    results
}

// ── Symbol search implementation (fallback) ─────────────────────────────────

#[derive(Debug)]
struct SymbolResult {
    file: PathBuf,
    line: usize,
    name: String,
    symbol_type: String,
    context: Option<String>,
}

/// Search for symbols in the given path (fallback when rg is unavailable)
async fn search_symbols(
    path: &Path,
    symbol_pattern: &str,
    symbol_type: &str,
    file_filter: Option<&Regex>,
    max_results: usize,
) -> Result<Vec<SymbolResult>> {
    let mut results = Vec::new();

    // Build symbol pattern regex
    let symbol_regex_str = if symbol_pattern.contains('*') {
        format!("^{}$", symbol_pattern.replace("*", ".*"))
    } else {
        symbol_pattern.to_string()
    };

    let symbol_regex = Regex::new(&symbol_regex_str).map_err(|e| ToolError::ExecutionFailed {
        tool: "code_search".to_string(),
        message: format!("Invalid symbol pattern: {}", e),
    })?;

    // Walk directory
    search_directory(
        path,
        &symbol_regex,
        symbol_type,
        file_filter,
        max_results,
        &mut results,
    )
    .await?;

    Ok(results)
}

/// Recursively search directory for symbols
async fn search_directory(
    dir: &Path,
    symbol_regex: &Regex,
    symbol_type: &str,
    file_filter: Option<&Regex>,
    max_results: usize,
    results: &mut Vec<SymbolResult>,
) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current_dir) = stack.pop() {
        if results.len() >= max_results {
            break;
        }

        let mut entries = match fs::read_dir(&current_dir).await {
            Ok(e) => e,
            Err(_) => continue, // Skip unreadable directories
        };

        while let Some(entry) = match entries.next_entry().await {
            Ok(Some(e)) => Some(e),
            _ => None,
        } {
            if results.len() >= max_results {
                break;
            }

            let path = entry.path();

            // Skip hidden directories and common build artifacts
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.')
                    || name == "node_modules"
                    || name == "target"
                    || name == "__pycache__"
                    || name == ".git"
                {
                    continue;
                }
            }

            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                // Check file filter
                if let Some(filter) = file_filter {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !filter.is_match(file_name) {
                        continue;
                    }
                }

                // Search file for symbols
                search_file(&path, symbol_regex, symbol_type, max_results, results).await?;
            }
        }
    }

    Ok(())
}

/// Search a single file for symbol definitions
async fn search_file(
    file: &Path,
    symbol_regex: &Regex,
    symbol_type: &str,
    max_results: usize,
    results: &mut Vec<SymbolResult>,
) -> Result<()> {
    if results.len() >= max_results {
        return Ok(());
    }

    // Detect language from extension
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");

    let patterns = get_language_patterns(extension);
    if patterns.is_empty() {
        return Ok(()); // Unsupported language
    }

    // Read file
    let content = match fs::read_to_string(file).await {
        Ok(c) => c,
        Err(_) => return Ok(()), // Skip unreadable files
    };

    // Search for each pattern
    for (pattern, sym_type) in patterns {
        if symbol_type != "any" && sym_type != symbol_type {
            continue;
        }

        let regex = match Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            if results.len() >= max_results {
                return Ok(());
            }

            if let Some(captures) = regex.captures(line) {
                if let Some(name_match) = captures.name("name") {
                    let name = name_match.as_str();

                    // Check if symbol matches the search pattern
                    if symbol_regex.is_match(name) {
                        results.push(SymbolResult {
                            file: file.to_path_buf(),
                            line: line_num + 1,
                            name: name.to_string(),
                            symbol_type: sym_type.to_string(),
                            context: Some(line.to_string()),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

/// Get symbol patterns for a given language
fn get_language_patterns(extension: &str) -> Vec<(&'static str, &'static str)> {
    match extension {
        "rs" => vec![
            (
                r"^\s*pub\s+fn\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "function",
            ),
            (r"^\s*fn\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "function"),
            (
                r"^\s*pub\s+struct\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "struct",
            ),
            (r"^\s*struct\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "struct"),
            (r"^\s*pub\s+enum\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "enum"),
            (r"^\s*enum\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "enum"),
            (
                r"^\s*pub\s+trait\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "trait",
            ),
            (r"^\s*trait\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "trait"),
            (r"^\s*type\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "type"),
        ],
        "py" => vec![
            (r"^\s*def\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "function"),
            (r"^\s*class\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "class"),
        ],
        "js" | "jsx" | "ts" | "tsx" => vec![
            (
                r"^\s*(export\s+)?function\s+(?P<name>[a-zA-Z_$][a-zA-Z0-9_$]*)",
                "function",
            ),
            (
                r"^\s*(export\s+)?class\s+(?P<name>[a-zA-Z_$][a-zA-Z0-9_$]*)",
                "class",
            ),
            (
                r"^\s*(export\s+)?interface\s+(?P<name>[a-zA-Z_$][a-zA-Z0-9_$]*)",
                "interface",
            ),
            (
                r"^\s*(export\s+)?type\s+(?P<name>[a-zA-Z_$][a-zA-Z0-9_$]*)",
                "type",
            ),
            (
                r"^\s*(export\s+)?const\s+(?P<name>[a-zA-Z_$][a-zA-Z0-9_$]*)\s*=",
                "function",
            ),
        ],
        "go" => vec![
            (r"^\s*func\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)", "function"),
            (
                r"^\s*type\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s+struct",
                "struct",
            ),
            (
                r"^\s*type\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s+interface",
                "interface",
            ),
        ],
        "java" => vec![
            (
                r"^\s*(public|private|protected)?\s*(static)?\s*\w+\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s*\(",
                "method",
            ),
            (
                r"^\s*(public|private|protected)?\s*class\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "class",
            ),
            (
                r"^\s*(public|private|protected)?\s*interface\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "interface",
            ),
        ],
        "c" | "h" | "cpp" | "hpp" => vec![
            (
                r"^\s*\w+[\s\*]+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s*\([^)]*\)\s*\{",
                "function",
            ),
            (
                r"^\s*(struct|class)\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)",
                "class",
            ),
        ],
        _ => vec![], // Unsupported language
    }
}

/// Convert glob pattern to regex
fn glob_to_regex(glob: &str) -> Regex {
    let regex_str = glob
        .replace(".", r"\.")
        .replace("*", ".*")
        .replace("?", ".");
    let pattern = format!("^{}$", regex_str);
    Regex::new(&pattern).unwrap_or_else(|_| Regex::new(".*").unwrap())
}
