//! Repo map tool — generate codebase structure summary.
//!
//! Static analysis: extracts file-level symbols (functions, structs, classes, modules)
//! and generates a concise structure map. NOT RAG — pure regex-based symbol extraction.
//!
//! Inspired by Aider's repo-map approach.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;

// ── RepoMapTool ────────────────────────────────────────────────────

/// Generate a codebase structure map by extracting symbols from source files.
///
/// Uses regex to identify definitions (fn, struct, class, def, etc.) without
/// requiring a language server. Outputs a concise tree or symbol list.
pub struct RepoMapTool {
    base_dir: Option<PathBuf>,
}

impl RepoMapTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for RepoMapTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for RepoMapTool {
    fn name(&self) -> &str {
        "repo_map"
    }

    fn description(&self) -> &str {
        "Generate a codebase structure map showing files and their symbols (functions, structs, classes, etc.). \
         Useful for quickly understanding project layout without reading every file. \
         Use format='tree' for directory tree, format='symbols' for symbol listing."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Root directory to map (default: current directory)"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to traverse (default: 3)"
                },
                "format": {
                    "type": "string",
                    "enum": ["tree", "symbols"],
                    "description": "Output format: 'tree' for directory tree, 'symbols' for symbol listing (default: tree)"
                }
            }
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let path_str = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let max_depth = parameters
                .get("max_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(3) as usize;

            let format = parameters
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("tree");

            // Effective base: construction-time base_dir (with confinement)
            // takes priority; otherwise fall back to runtime working_dir.
            let effective_base: Option<PathBuf> = self
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_path_buf()));

            let root = if let Some(ref base) = effective_base {
                let resolved = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    base.join(path_str)
                };
                if !resolved.starts_with(base) {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "Path '{}' is outside the allowed directory scope",
                        path_str
                    )));
                }
                resolved
            } else {
                PathBuf::from(path_str)
            };

            if !root.exists() {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path does not exist: {}",
                    root.display()
                )));
            }

            let output = match format {
                "symbols" => build_symbol_map(&root, max_depth).await?,
                _ => build_tree(&root, max_depth).await?,
            };

            if output.is_empty() {
                return Ok(ToolResult::success(
                    "Empty directory or no source files found.".to_string(),
                ));
            }

            Ok(ToolResult::success(output))
        })
    }
}

// ── Tree builder ───────────────────────────────────────────────────

async fn build_tree(root: &Path, max_depth: usize) -> Result<String> {
    let mut lines = Vec::new();
    let root_name = root.file_name().and_then(|n| n.to_str()).unwrap_or(".");
    lines.push(format!("{}/", root_name));
    walk_tree(root, "", max_depth, 0, &mut lines).await?;
    Ok(lines.join("\n"))
}

fn walk_tree<'a>(
    dir: &'a Path,
    prefix: &'a str,
    max_depth: usize,
    depth: usize,
    lines: &'a mut Vec<String>,
) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        if depth >= max_depth {
            return Ok(());
        }

        let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
        let mut dir_entries = fs::read_dir(dir)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "repo_map".to_string(),
                message: format!("Cannot read directory: {}", e),
            })?;

        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Skip hidden, build artifacts, dependencies
            if should_skip(&name) {
                continue;
            }

            let is_dir = path.is_dir();
            entries.push((name, path, is_dir));
        }

        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (i, (name, path, is_dir)) in entries.iter().enumerate() {
            let is_last = i == entries.len() - 1;
            let connector = if is_last { "└── " } else { "├── " };
            let child_prefix = if is_last { "    " } else { "│   " };

            if *is_dir {
                lines.push(format!("{}{}{}/", prefix, connector, name));
                walk_tree(
                    path,
                    &format!("{}{}", prefix, child_prefix),
                    max_depth,
                    depth + 1,
                    lines,
                )
                .await?;
            } else {
                // Count symbols in source files
                let symbol_count = if is_source_file(name) {
                    count_symbols(path).await.unwrap_or(0)
                } else {
                    0
                };
                if symbol_count > 0 {
                    lines.push(format!(
                        "{}{}{} ({} symbols)",
                        prefix, connector, name, symbol_count
                    ));
                } else {
                    lines.push(format!("{}{}{}", prefix, connector, name));
                }
            }
        }

        Ok(())
    })
}

// ── Symbol map builder ─────────────────────────────────────────────

async fn build_symbol_map(root: &Path, max_depth: usize) -> Result<String> {
    let mut symbols: BTreeMap<String, Vec<String>> = BTreeMap::new();
    collect_symbols(root, max_depth, 0, &mut symbols).await?;

    let mut lines = Vec::new();
    for (file, syms) in &symbols {
        if !syms.is_empty() {
            let file_path = Path::new(file.as_str());
            let rel = file_path.strip_prefix(root).unwrap_or(file_path).display();
            lines.push(format!("{}:", rel));
            for sym in syms {
                lines.push(format!("  {}", sym));
            }
        }
    }

    Ok(lines.join("\n"))
}

async fn collect_symbols(
    dir: &Path,
    max_depth: usize,
    depth: usize,
    symbols: &mut BTreeMap<String, Vec<String>>,
) -> Result<()> {
    if depth >= max_depth {
        return Ok(());
    }

    let mut dir_entries = fs::read_dir(dir)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "repo_map".to_string(),
            message: format!("Cannot read directory: {}", e),
        })?;

    while let Ok(Some(entry)) = dir_entries.next_entry().await {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if should_skip(&name) {
            continue;
        }

        if path.is_dir() {
            Box::pin(collect_symbols(&path, max_depth, depth + 1, symbols)).await?;
        } else if is_source_file(&name) {
            let syms = extract_symbols(&path).await;
            if !syms.is_empty() {
                symbols.insert(path.display().to_string(), syms);
            }
        }
    }

    Ok(())
}

// ── Symbol extraction ──────────────────────────────────────────────

async fn extract_symbols(path: &Path) -> Vec<String> {
    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "rs" => extract_rust_symbols(&content),
        "py" => extract_python_symbols(&content),
        "ts" | "tsx" | "js" | "jsx" => extract_js_symbols(&content),
        "go" => extract_go_symbols(&content),
        "java" | "kt" => extract_java_symbols(&content),
        _ => vec![],
    }
}

fn extract_rust_symbols(content: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("async fn ")
        {
            if let Some(name) = extract_fn_name(trimmed) {
                symbols.push(format!("fn {}", name));
            }
        } else if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
            if let Some(name) = extract_type_name(trimmed, "struct") {
                symbols.push(format!("struct {}", name));
            }
        } else if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
            if let Some(name) = extract_type_name(trimmed, "enum") {
                symbols.push(format!("enum {}", name));
            }
        } else if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
            if let Some(name) = extract_type_name(trimmed, "trait") {
                symbols.push(format!("trait {}", name));
            }
        } else if trimmed.starts_with("pub type ") || trimmed.starts_with("type ") {
            if let Some(name) = extract_type_name(trimmed, "type") {
                symbols.push(format!("type {}", name));
            }
        } else if trimmed.starts_with("impl ") {
            let rest = &trimmed[5..];
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                symbols.push(format!("impl {}", name));
            }
        }
    }
    symbols
}

fn extract_python_symbols(content: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let prefix = if trimmed.starts_with("async ") {
                "async def"
            } else {
                "def"
            };
            let rest = if trimmed.starts_with("async ") {
                &trimmed[10..]
            } else {
                &trimmed[4..]
            };
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                symbols.push(format!("{} {}", prefix, name));
            }
        } else if trimmed.starts_with("class ") {
            let rest = &trimmed[6..];
            let name = rest
                .split(|c: char| c == '(' || c == ':' || c == '{')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                symbols.push(format!("class {}", name));
            }
        }
    }
    symbols
}

fn extract_js_symbols(content: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("function ") || trimmed.starts_with("async function ") {
            let prefix = if trimmed.starts_with("async ") {
                "async function"
            } else {
                "function"
            };
            let rest = if trimmed.starts_with("async ") {
                &trimmed[15..]
            } else {
                &trimmed[9..]
            };
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                symbols.push(format!("{} {}", prefix, name));
            }
        } else if trimmed.starts_with("export class ") || trimmed.starts_with("class ") {
            let rest = if trimmed.starts_with("export ") {
                &trimmed[13..]
            } else {
                &trimmed[6..]
            };
            let name = rest
                .split(|c: char| c == ' ' || c == '{')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                symbols.push(format!("class {}", name));
            }
        } else if trimmed.starts_with("export interface ") || trimmed.starts_with("interface ") {
            let rest = if trimmed.starts_with("export ") {
                &trimmed[16..]
            } else {
                &trimmed[10..]
            };
            let name = rest
                .split(|c: char| c == ' ' || c == '{')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                symbols.push(format!("interface {}", name));
            }
        }
    }
    symbols
}

fn extract_go_symbols(content: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("func ") {
            let rest = &trimmed[5..];
            // Handle method receivers: func (r *Receiver) Name(...)
            let name_part = if rest.starts_with('(') {
                // Method with receiver
                if let Some(close) = rest.find(')') {
                    let after = rest[close + 1..].trim();
                    after.split('(').next().unwrap_or("").trim()
                } else {
                    rest.split('(').next().unwrap_or("").trim()
                }
            } else {
                rest.split('(').next().unwrap_or("").trim()
            };
            if !name_part.is_empty() {
                symbols.push(format!("func {}", name_part));
            }
        } else if trimmed.starts_with("type ") && trimmed.ends_with(" struct {") {
            let name = trimmed[5..].split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                symbols.push(format!("type {} struct", name));
            }
        } else if trimmed.starts_with("type ") && trimmed.ends_with(" interface {") {
            let name = trimmed[5..].split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                symbols.push(format!("type {} interface", name));
            }
        }
    }
    symbols
}

fn extract_java_symbols(content: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if (trimmed.contains("public class ")
            || trimmed.contains("private class ")
            || trimmed.contains("protected class ")
            || trimmed.contains("class "))
            && !trimmed.starts_with("//")
        {
            let name = extract_java_type_name(trimmed, "class");
            if let Some(name) = name {
                symbols.push(format!("class {}", name));
            }
        } else if (trimmed.contains("public interface ") || trimmed.contains("interface "))
            && !trimmed.starts_with("//")
        {
            let name = extract_java_type_name(trimmed, "interface");
            if let Some(name) = name {
                symbols.push(format!("interface {}", name));
            }
        } else if (trimmed.starts_with("public ")
            || trimmed.starts_with("private ")
            || trimmed.starts_with("protected "))
            && trimmed.contains("(")
            && !trimmed.contains("class ")
            && !trimmed.contains("interface ")
            && !trimmed.starts_with("//")
        {
            // Method declaration
            if let Some(name) = extract_java_method_name(trimmed) {
                symbols.push(format!("fn {}", name));
            }
        }
    }
    symbols
}

// ── Helpers ────────────────────────────────────────────────────────

fn should_skip(name: &str) -> bool {
    name.starts_with('.')
        || name == "target"
        || name == "node_modules"
        || name == "__pycache__"
        || name == ".git"
        || name == "vendor"
        || name == "build"
        || name == "dist"
        || name == ".next"
        || name == ".nuxt"
        || name == "venv"
        || name == ".venv"
        || name == "env"
}

fn is_source_file(name: &str) -> bool {
    let exts = [
        "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "c", "cpp", "h", "hpp", "cs",
        "rb", "swift", "scala",
    ];
    if let Some(dot_pos) = name.rfind('.') {
        let ext = &name[dot_pos + 1..];
        exts.contains(&ext)
    } else {
        false
    }
}

async fn count_symbols(path: &Path) -> Result<usize> {
    let content = fs::read_to_string(path).await.unwrap_or_default();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let count = match ext {
        "rs" => extract_rust_symbols(&content).len(),
        "py" => extract_python_symbols(&content).len(),
        "ts" | "tsx" | "js" | "jsx" => extract_js_symbols(&content).len(),
        "go" => extract_go_symbols(&content).len(),
        "java" | "kt" => extract_java_symbols(&content).len(),
        _ => 0,
    };
    Ok(count)
}

fn extract_fn_name(line: &str) -> Option<String> {
    let rest = line
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("fn ");
    let name = rest.split('(').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_type_name(line: &str, keyword: &str) -> Option<String> {
    let rest = line
        .trim_start_matches("pub ")
        .trim_start_matches(&format!("{} ", keyword));
    let name = rest
        .split(|c: char| c == ' ' || c == '{' || c == '<' || c == ';')
        .next()?
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_java_type_name(line: &str, keyword: &str) -> Option<String> {
    let idx = line.find(keyword)?;
    let rest = &line[idx + keyword.len()..];
    let name = rest
        .split(|c: char| c == ' ' || c == '{' || c == '<')
        .next()?
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_java_method_name(line: &str) -> Option<String> {
    let paren_idx = line.find('(')?;
    let before = &line[..paren_idx];
    let name = before.split_whitespace().last()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rust_symbols() {
        let content = r#"
pub fn hello() {}
async fn world() {}
pub struct MyStruct;
enum MyEnum { A, B }
trait MyTrait {}
impl MyStruct {}
"#;
        let symbols = extract_rust_symbols(content);
        assert!(symbols.contains(&"fn hello".to_string()));
        assert!(symbols.contains(&"fn world".to_string()));
        assert!(symbols.contains(&"struct MyStruct".to_string()));
        assert!(symbols.contains(&"enum MyEnum".to_string()));
        assert!(symbols.contains(&"trait MyTrait".to_string()));
        assert!(symbols.contains(&"impl MyStruct".to_string()));
    }

    #[test]
    fn test_extract_python_symbols() {
        let content = r#"
def hello():
    pass
async def world():
    pass
class MyClass:
    pass
"#;
        let symbols = extract_python_symbols(content);
        assert!(symbols.contains(&"def hello".to_string()));
        assert!(symbols.contains(&"async def world".to_string()));
        assert!(symbols.contains(&"class MyClass".to_string()));
    }

    #[test]
    fn test_extract_go_symbols() {
        let content = r#"
func Hello() {}
func (r *Receiver) Method() {}
type MyStruct struct {
}
"#;
        let symbols = extract_go_symbols(content);
        assert!(symbols.contains(&"func Hello".to_string()));
        assert!(symbols.contains(&"func Method".to_string()));
        assert!(symbols.contains(&"type MyStruct struct".to_string()));
    }

    #[test]
    fn test_should_skip() {
        assert!(should_skip(".git"));
        assert!(should_skip("target"));
        assert!(should_skip("node_modules"));
        assert!(!should_skip("src"));
        assert!(!should_skip("main.rs"));
    }

    #[test]
    fn test_is_source_file() {
        assert!(is_source_file("main.rs"));
        assert!(is_source_file("app.tsx"));
        assert!(is_source_file("lib.go"));
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("Cargo.toml"));
    }
}
