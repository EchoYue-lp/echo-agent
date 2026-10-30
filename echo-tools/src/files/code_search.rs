//! Structure-oriented code search over language symbols.

use echo_core::error::{Result, ToolError};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

// ── CodeSearchTool ──────────────────────────────────────────────────────────

/// Search for named code symbols across the project.
#[derive(Clone)]
pub struct CodeSearchTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl CodeSearchTool {
    pub fn with_file_system(
        mut self,
        file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = file_system;
        self
    }

    pub fn new() -> Self {
        Self {
            base_dir: None,
            file_system: std::sync::Arc::new(super::LocalFileSystem),
        }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
            file_system: std::sync::Arc::new(super::LocalFileSystem),
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
        "Find code definitions by symbol name and kind; use grep for exact text lookup."
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
                    "description": "Symbol name, regex, or * wildcard pattern"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "File glob filter (e.g., '*.rs', '*.py')"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum matched records per page (default: 30)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque cursor from page.next_cursor; reuse only with identical parameters"
                },
                "symbol_type": {
                    "type": "string",
                    "enum": ["function", "class", "struct", "enum", "trait", "interface", "type", "method", "any"],
                    "description": "Definition kind to find (default: any)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        let this = self.clone();
        let ctx = ctx.clone();
        super::run_file_operation("code_search", &ctx.clone(), move || {
            let file_system = this.file_system.as_ref();
            let cancel = ctx.cancel.as_deref();
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let glob_pattern = parameters.get("glob").and_then(|v| v.as_str());
            let symbol_type = parameters
                .get("symbol_type")
                .and_then(|v| v.as_str())
                .unwrap_or("any");
            if !matches!(
                symbol_type,
                "function"
                    | "class"
                    | "struct"
                    | "enum"
                    | "trait"
                    | "interface"
                    | "type"
                    | "method"
                    | "any"
            ) {
                return Ok(ToolResult::invalid_arguments(format!(
                    "unsupported symbol_type '{symbol_type}'"
                )));
            }

            let page_request = match PageRequest::from_parameters(&parameters, 30, 100) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            // Resolve search path
            let search_path = super::resolve_path(
                this.file_system.as_ref(),
                "code_search",
                path_str,
                &this.base_dir,
                ctx.working_dir.as_deref(),
            )?;

            if !file_system.exists(&search_path)? {
                return Err(ToolError::ExecutionFailed {
                    tool: "code_search".to_string(),
                    message: format!("Path does not exist: {}", search_path.display()),
                }
                .into());
            }

            let file_filter = glob_pattern.map(glob_to_regex).transpose()?;
            let mut results = search_symbols(
                file_system,
                cancel,
                &search_path,
                &this.base_dir,
                query,
                symbol_type,
                file_filter.as_ref(),
            )?;
            results.sort_by(|left, right| {
                left.file
                    .cmp(&right.file)
                    .then(left.line.cmp(&right.line))
                    .then(left.name.cmp(&right.name))
            });
            let records = results
                .into_iter()
                .map(|result| {
                    let mut record = format!(
                        "{}:{} - {} {}",
                        result.file.display(),
                        result.line,
                        result.symbol_type,
                        result.name
                    );
                    if let Some(context) = result.context {
                        record.push_str(&format!("\n  {}", context.trim()));
                    }
                    record
                })
                .collect();
            let query_identity = serde_json::json!({
                "query": query,
                "path": search_path,
                "glob": glob_pattern,
                "symbol_type": symbol_type,
                "backend": "symbol_structure",
            });
            paginated_search_result(&page_request, records, &query_identity, query, &search_path)
        })
    }
}

fn paginated_search_result(
    request: &PageRequest,
    records: Vec<String>,
    query_identity: &Value,
    query: &str,
    search_path: &Path,
) -> Result<ToolResult> {
    let (page, page_info) = match request.paginate(records, query_identity) {
        Ok(page) => page,
        Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
    };
    let output = if page.is_empty() {
        format!(
            "No matches found for '{}' in {}",
            query,
            search_path.display()
        )
    } else {
        format!(
            "Found {} match(es) on this page ({} total) for '{}' in {}:\n\n{}",
            page_info.returned,
            page_info.total.unwrap_or(0),
            query,
            search_path.display(),
            page.join("\n")
        )
    };
    let mut result = ToolResult::success(output);
    page_info.apply_to(&mut result);
    Ok(result)
}

// ── Symbol search implementation ─────────────────────────────────────────────

#[derive(Debug)]
struct SymbolResult {
    file: PathBuf,
    line: usize,
    name: String,
    symbol_type: String,
    context: Option<String>,
}

/// Search for symbols in the given path.
fn search_symbols(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    path: &Path,
    base_dir: &Option<PathBuf>,
    symbol_pattern: &str,
    symbol_type: &str,
    file_filter: Option<&Regex>,
) -> Result<Vec<SymbolResult>> {
    super::check_file_cancel(cancel, "code_search")?;
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
        file_system,
        cancel,
        path,
        base_dir,
        (&symbol_regex, symbol_type),
        file_filter,
        &mut results,
    )?;

    Ok(results)
}

/// Recursively search directory for symbols
fn search_directory(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    dir: &Path,
    base_dir: &Option<PathBuf>,
    symbol_query: (&Regex, &str),
    file_filter: Option<&Regex>,
    results: &mut Vec<SymbolResult>,
) -> Result<()> {
    super::check_file_cancel(cancel, "code_search")?;
    let (symbol_regex, symbol_type) = symbol_query;
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current_dir) = stack.pop() {
        super::check_file_cancel(cancel, "code_search")?;
        let entries = match file_system.read_dir(&current_dir) {
            Ok(e) => e,
            Err(echo_core::error::ReactError::Io(_)) => continue, // Skip unreadable directories
            Err(error) => return Err(error),
        };

        for entry in entries {
            super::check_file_cancel(cancel, "code_search")?;
            let path = entry.path();

            // Skip hidden directories and common build artifacts
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name.starts_with('.')
                    || name == "node_modules"
                    || name == "target"
                    || name == "__pycache__"
                    || name == ".git")
            {
                continue;
            }

            if base_dir.is_some() {
                super::resolve_path_buf(file_system, "code_search", &path, base_dir)?;
            }
            if file_system.is_dir(&path)? {
                stack.push(path);
            } else if file_system.is_file(&path)? {
                // Check file filter
                if let Some(filter) = file_filter {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !filter.is_match(file_name) {
                        continue;
                    }
                }

                // Search file for symbols
                search_file(
                    file_system,
                    cancel,
                    &path,
                    symbol_regex,
                    symbol_type,
                    results,
                )?;
            }
        }
    }

    Ok(())
}

/// Search a single file for symbol definitions
fn search_file(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    file: &Path,
    symbol_regex: &Regex,
    symbol_type: &str,
    results: &mut Vec<SymbolResult>,
) -> Result<()> {
    super::check_file_cancel(cancel, "code_search")?;
    // Detect language from extension
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");

    let patterns = get_language_patterns(extension);
    if patterns.is_empty() {
        return Ok(()); // Unsupported language
    }

    // Read file
    let content = match file_system.read_to_string(file) {
        Ok(c) => c,
        Err(echo_core::error::ReactError::Io(_)) => return Ok(()),
        Err(error) => return Err(error), // Skip unreadable files
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
            super::check_file_cancel(cancel, "code_search")?;
            if let Some(captures) = regex.captures(line)
                && let Some(name_match) = captures.name("name")
            {
                let name = name_match.as_str();

                // Check if symbol matches the search pattern
                if symbol_regex.is_match(name) {
                    results.push(SymbolResult {
                        file: file.to_path_buf(),
                        line: line_num.saturating_add(1),
                        name: name.to_string(),
                        symbol_type: sym_type.to_string(),
                        context: Some(line.to_string()),
                    });
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

/// Convert a glob pattern to the symbol-search file filter.
fn glob_to_regex(glob: &str) -> Result<Regex> {
    let regex_str = regex::escape(glob).replace(r"\*", ".*").replace(r"\?", ".");
    Regex::new(&format!("^{regex_str}$")).map_err(|error| {
        ToolError::InvalidParameter {
            name: "glob".to_string(),
            message: format!("Invalid glob pattern: {error}"),
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::ToolContext;
    use tokio::fs;

    #[tokio::test]
    async fn searches_symbol_definitions_instead_of_text_mentions()
    -> std::result::Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        fs::write(
            directory.path().join("sample.rs"),
            "fn target_symbol() {}\nfn unrelated() { let _ = \"target_symbol\"; }\n",
        )
        .await
        .map_err(|error| error.to_string())?;
        let tool = CodeSearchTool::with_base_dir(directory.path());
        let mut parameters = ToolParameters::new();
        parameters.insert("query".to_string(), json!("^target_symbol$"));
        parameters.insert("path".to_string(), json!("."));

        let result = tool
            .execute_with_context(parameters, &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success, "unexpected result: {}", result.output);
        assert_eq!(
            result.metadata.get("page.total").map(String::as_str),
            Some("1")
        );
        assert!(result.output.contains("function target_symbol"));
        Ok(())
    }
}
