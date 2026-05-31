//! Code Search tool — search for code symbols across the project
//!
//! Searches for function definitions, class definitions, type definitions,
//! and other code symbols. Supports multiple programming languages.

use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::fs;

// ── CodeSearchTool ──────────────────────────────────────────────────────────

/// Search for code symbols (functions, classes, types) across the project.
///
/// Uses language-specific patterns to find symbol definitions. Returns
/// structured results with file paths, line numbers, and symbol types.
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
        "Search for code symbols (functions, classes, types, methods) across the project. \
         Returns definitions with file paths and line numbers. \
         Supports Rust, Python, JavaScript, TypeScript, Go, Java, and more."
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
                "symbol": {
                    "type": "string",
                    "description": "Symbol name or pattern to search for (e.g., 'calculate_total', 'User', 'process_*')"
                },
                "symbol_type": {
                    "type": "string",
                    "enum": ["function", "class", "struct", "enum", "trait", "interface", "type", "method", "any"],
                    "description": "Type of symbol to search for (default: any)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "File pattern to filter (e.g., '*.rs', '*.py')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 50)"
                }
            },
            "required": ["symbol"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let symbol = parameters
                .get("symbol")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("symbol".to_string()))?;

            let symbol_type = parameters
                .get("symbol_type")
                .and_then(|v| v.as_str())
                .unwrap_or("any");

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let glob_pattern = parameters.get("glob").and_then(|v| v.as_str());

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

            // Convert glob pattern to regex if provided
            let file_filter = glob_pattern.map(|g| glob_to_regex(g));

            // Search for symbols
            let results = search_symbols(
                &search_path,
                symbol,
                symbol_type,
                file_filter.as_ref(),
                max_results,
            )
            .await?;

            if results.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No {} symbols matching '{}' found in {}",
                    symbol_type,
                    symbol,
                    search_path.display()
                )));
            }

            // Format results
            let mut output = format!(
                "Found {} {} symbol(s) matching '{}':\n\n",
                results.len(),
                symbol_type,
                symbol
            );

            for result in &results {
                output.push_str(&format!(
                    "{}:{} - {} {}\n",
                    result.file.display(),
                    result.line,
                    result.symbol_type,
                    result.name
                ));
                if let Some(ref context) = result.context {
                    output.push_str(&format!("  {}\n", context.trim()));
                }
            }

            Ok(ToolResult::success(output))
        })
    }
}

// ── Symbol search implementation ────────────────────────────────────────────

#[derive(Debug)]
struct SymbolResult {
    file: PathBuf,
    line: usize,
    name: String,
    symbol_type: String,
    context: Option<String>,
}

/// Search for symbols in the given path
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
