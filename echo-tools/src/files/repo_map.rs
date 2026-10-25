//! AST-aware repository map with a lightweight text fallback.
//!
//! Tree-sitter extracts real declarations for the primary coding languages.
//! Unsupported files and parser failures fall back to conservative line-based
//! extraction so repo mapping remains available without a language server.

use echo_core::error::{Result, ToolError};
use echo_core::tools::pagination::PageRequest;
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node, Parser};

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct RepoMapTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl RepoMapTool {
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
        "Generate a codebase tree or AST-aware symbol map. Rust, Python, JavaScript/TypeScript, Go, and Java use Tree-sitter; unsupported syntax falls back to conservative text extraction."
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
                    "description": "Output format (default: tree)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Maximum tree or symbol lines per page (default: 100)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque cursor from page.next_cursor; reuse only with identical parameters"
                }
            }
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        let this = self.clone();
        let ctx = ctx.clone();
        super::run_file_operation("repo_map", &ctx.clone(), move || {
            let file_system = this.file_system.as_ref();
            let cancel = ctx.cancel.as_deref();
            let path_value = parameters
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".");
            let max_depth = parameters
                .get("max_depth")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(3);
            let format = parameters
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("tree");
            let page_request = match PageRequest::from_parameters(&parameters, 100, 200) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let effective_base = this
                .base_dir
                .clone()
                .or_else(|| ctx.working_dir.as_ref().map(|path| path.to_path_buf()));
            let root = resolve_root(effective_base.as_deref(), path_value)?;
            if !file_system.exists(&root)? {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Path does not exist: {}",
                    root.display()
                )));
            }
            let output = if format == "symbols" {
                build_symbol_map(file_system, cancel, &root, max_depth)?
            } else {
                build_tree(file_system, cancel, &root, max_depth)?
            };
            let lines = if output.is_empty() {
                Vec::new()
            } else {
                output.lines().map(str::to_string).collect()
            };
            let query = serde_json::json!({
                "path": root,
                "max_depth": max_depth,
                "format": format,
            });
            let (page, page_info) = match page_request.paginate(lines, &query) {
                Ok(page) => page,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let output = if page.is_empty() {
                "Empty directory or no source files found.".to_string()
            } else {
                page.join("\n")
            };
            let mut result = ToolResult::success(output);
            page_info.apply_to(&mut result);
            Ok(result)
        })
    }
}

fn resolve_root(base: Option<&Path>, path_value: &str) -> Result<PathBuf> {
    let requested = Path::new(path_value);
    let Some(base) = base else {
        return Ok(requested.to_path_buf());
    };
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    if !resolved.starts_with(base) {
        return Err(ToolError::InvalidParameter {
            name: "path".to_string(),
            message: format!("Path '{path_value}' is outside the allowed directory scope"),
        }
        .into());
    }
    Ok(resolved)
}

fn build_tree(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    root: &Path,
    max_depth: usize,
) -> Result<String> {
    super::check_file_cancel(cancel, "repo_map")?;
    let mut lines = Vec::new();
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(".");
    lines.push(format!("{root_name}/"));
    walk_tree(file_system, cancel, root, "", max_depth, 0, &mut lines)?;
    Ok(lines.join("\n"))
}

fn walk_tree(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    directory: &Path,
    prefix: &str,
    max_depth: usize,
    depth: usize,
    lines: &mut Vec<String>,
) -> Result<()> {
    super::check_file_cancel(cancel, "repo_map")?;
    if depth >= max_depth {
        return Ok(());
    }
    let mut entries = Vec::new();
    let directory_entries = file_system.read_dir(directory).map_err(|error| {
        super::file_error(error, |error| {
            execution_error(format!("Cannot read directory: {error}"))
        })
    })?;
    for entry in directory_entries {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_string();
        if should_skip(&name) {
            continue;
        }
        entries.push((name, path.clone(), file_system.is_dir(&path)?));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let entry_count = entries.len();
    for (position, (name, path, is_directory)) in entries.into_iter().enumerate() {
        let is_last = position.saturating_add(1) == entry_count;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };
        if is_directory {
            lines.push(format!("{prefix}{connector}{name}/"));
            walk_tree(
                file_system,
                cancel,
                &path,
                &format!("{prefix}{child_prefix}"),
                max_depth,
                depth.saturating_add(1),
                lines,
            )?;
        } else {
            let symbol_count = if is_source_file(&name) {
                extract_symbols(file_system, cancel, &path)?.len()
            } else {
                0
            };
            if symbol_count > 0 {
                lines.push(format!(
                    "{prefix}{connector}{name} ({symbol_count} symbols)"
                ));
            } else {
                lines.push(format!("{prefix}{connector}{name}"));
            }
        }
    }
    Ok(())
}

fn build_symbol_map(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    root: &Path,
    max_depth: usize,
) -> Result<String> {
    super::check_file_cancel(cancel, "repo_map")?;
    let mut symbols = BTreeMap::new();
    collect_symbols(file_system, cancel, root, max_depth, 0, &mut symbols)?;
    let mut lines = Vec::new();
    for (file, file_symbols) in symbols {
        if file_symbols.is_empty() {
            continue;
        }
        let relative = file.strip_prefix(root).unwrap_or(file.as_path()).display();
        lines.push(format!("{relative}:"));
        lines.extend(file_symbols.into_iter().map(|symbol| format!("  {symbol}")));
    }
    Ok(lines.join("\n"))
}

fn collect_symbols(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    directory: &Path,
    max_depth: usize,
    depth: usize,
    symbols: &mut BTreeMap<PathBuf, Vec<String>>,
) -> Result<()> {
    super::check_file_cancel(cancel, "repo_map")?;
    if depth >= max_depth {
        return Ok(());
    }
    let entries = file_system.read_dir(directory).map_err(|error| {
        super::file_error(error, |error| {
            execution_error(format!("Cannot read directory: {error}"))
        })
    })?;
    for entry in entries {
        super::check_file_cancel(cancel, "repo_map")?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if should_skip(name) {
            continue;
        }
        if file_system.is_dir(&path)? {
            collect_symbols(
                file_system,
                cancel,
                &path,
                max_depth,
                depth.saturating_add(1),
                symbols,
            )?;
        } else if is_source_file(name) {
            let extracted = extract_symbols(file_system, cancel, &path)?;
            if !extracted.is_empty() {
                symbols.insert(path, extracted);
            }
        }
    }
    Ok(())
}

fn extract_symbols(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    cancel: Option<&echo_core::agent::CancellationToken>,
    path: &Path,
) -> Result<Vec<String>> {
    super::check_file_cancel(cancel, "repo_map")?;
    let metadata = match file_system.metadata(path) {
        Ok(metadata) if metadata.len() <= MAX_SOURCE_BYTES => metadata,
        Ok(_) | Err(echo_core::error::ReactError::Io(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Ok(Vec::new());
    }
    let source = match file_system.read_to_string(path) {
        Ok(source) => source,
        Err(echo_core::error::ReactError::Io(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    Ok(
        extract_ast_symbols(extension, &source)
            .unwrap_or_else(|| extract_fallback_symbols(&source)),
    )
}

fn extract_ast_symbols(extension: &str, source: &str) -> Option<Vec<String>> {
    let language = language_for_extension(extension)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut symbols = Vec::new();
    collect_ast_symbols(tree.root_node(), source, extension, &mut symbols);
    Some(symbols)
}

fn language_for_extension(extension: &str) -> Option<Language> {
    match extension {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" | "pyi" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        _ => None,
    }
}

fn collect_ast_symbols(node: Node<'_>, source: &str, extension: &str, output: &mut Vec<String>) {
    if let Some(symbol) = symbol_for_node(node, source, extension) {
        output.push(symbol);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ast_symbols(child, source, extension, output);
    }
}

fn symbol_for_node(node: Node<'_>, source: &str, extension: &str) -> Option<String> {
    let kind = node.kind();
    let (label, name_node) = match extension {
        "rs" => match kind {
            "function_item" => ("fn", node.child_by_field_name("name")),
            "struct_item" => ("struct", node.child_by_field_name("name")),
            "enum_item" => ("enum", node.child_by_field_name("name")),
            "trait_item" => ("trait", node.child_by_field_name("name")),
            "type_item" => ("type", node.child_by_field_name("name")),
            "mod_item" => ("mod", node.child_by_field_name("name")),
            "impl_item" => ("impl", node.child_by_field_name("type")),
            _ => return None,
        },
        "py" | "pyi" => match kind {
            "function_definition" => ("fn", node.child_by_field_name("name")),
            "class_definition" => ("class", node.child_by_field_name("name")),
            _ => return None,
        },
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => match kind {
            "function_declaration" | "generator_function_declaration" | "method_definition" => {
                ("fn", node.child_by_field_name("name"))
            }
            "class_declaration" => ("class", node.child_by_field_name("name")),
            "interface_declaration" => ("interface", node.child_by_field_name("name")),
            "type_alias_declaration" => ("type", node.child_by_field_name("name")),
            "enum_declaration" => ("enum", node.child_by_field_name("name")),
            "variable_declarator" => {
                let value = node.child_by_field_name("value")?;
                if !matches!(value.kind(), "arrow_function" | "function_expression") {
                    return None;
                }
                ("fn", node.child_by_field_name("name"))
            }
            _ => return None,
        },
        "go" => match kind {
            "function_declaration" | "method_declaration" => {
                ("fn", node.child_by_field_name("name"))
            }
            "type_spec" => {
                let type_node = node.child_by_field_name("type");
                let label = match type_node.map(|item| item.kind()) {
                    Some("struct_type") => "struct",
                    Some("interface_type") => "interface",
                    _ => "type",
                };
                (label, node.child_by_field_name("name"))
            }
            _ => return None,
        },
        "java" => match kind {
            "class_declaration" | "record_declaration" => {
                ("class", node.child_by_field_name("name"))
            }
            "interface_declaration" => ("interface", node.child_by_field_name("name")),
            "enum_declaration" => ("enum", node.child_by_field_name("name")),
            "method_declaration" | "constructor_declaration" => {
                ("fn", node.child_by_field_name("name"))
            }
            _ => return None,
        },
        _ => return None,
    };
    let name = name_node
        .and_then(|name| name.utf8_text(source.as_bytes()).ok())
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    let line = node.start_position().row.saturating_add(1);
    Some(format!("{label} {name} (line {line})"))
}

fn extract_fallback_symbols(source: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let candidates = [
            ("fn ", "fn"),
            ("def ", "fn"),
            ("class ", "class"),
            ("struct ", "struct"),
            ("enum ", "enum"),
            ("trait ", "trait"),
            ("interface ", "interface"),
            ("func ", "fn"),
        ];
        for (prefix, label) in candidates {
            let Some(rest) = trimmed.strip_prefix(prefix) else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|character| {
                    character.is_alphanumeric() || *character == '_' || *character == '$'
                })
                .collect();
            if !name.is_empty() {
                symbols.push(format!(
                    "{label} {name} (line {})",
                    line_index.saturating_add(1)
                ));
            }
            break;
        }
    }
    symbols
}

fn should_skip(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target"
                | "node_modules"
                | "__pycache__"
                | "vendor"
                | "build"
                | "dist"
                | ".next"
                | ".nuxt"
                | "venv"
                | ".venv"
                | "env"
        )
}

fn is_source_file(name: &str) -> bool {
    let extension = Path::new(name).extension().and_then(|value| value.to_str());
    extension.is_some_and(|extension| {
        matches!(
            extension,
            "rs" | "py"
                | "pyi"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "go"
                | "java"
                | "kt"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "swift"
                | "scala"
        )
    })
}

fn execution_error(message: String) -> ToolError {
    ToolError::ExecutionFailed {
        tool: "repo_map".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_symbols_from_ast() -> Result<()> {
        let source = r#"
pub fn hello() {}
impl Demo { pub async fn run(&self) {} }
#[derive(Clone)]
pub struct Demo;
enum State { Ready }
trait Service {}
"#;
        let symbols = extract_ast_symbols("rs", source)
            .ok_or_else(|| execution_error("Rust parser unavailable".to_string()))?;
        assert!(symbols.iter().any(|symbol| symbol.starts_with("fn hello")));
        assert!(symbols.iter().any(|symbol| symbol.starts_with("fn run")));
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.starts_with("struct Demo"))
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.starts_with("enum State"))
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.starts_with("trait Service"))
        );
        Ok(())
    }

    #[test]
    fn extracts_typescript_arrow_functions_and_interfaces() -> Result<()> {
        let source = "export interface User { id: string }\nexport const load = async () => 1;";
        let symbols = extract_ast_symbols("ts", source)
            .ok_or_else(|| execution_error("TypeScript parser unavailable".to_string()))?;
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.starts_with("interface User"))
        );
        assert!(symbols.iter().any(|symbol| symbol.starts_with("fn load")));
        Ok(())
    }

    #[test]
    fn falls_back_without_byte_slicing() {
        let symbols = extract_fallback_symbols("class 研究结果:\n    pass\n");
        assert!(symbols.iter().any(|symbol| symbol.contains("研究结果")));
    }

    #[test]
    fn identifies_source_files_and_skipped_directories() {
        assert!(is_source_file("main.rs"));
        assert!(is_source_file("app.tsx"));
        assert!(!is_source_file("README.md"));
        assert!(should_skip(".git"));
        assert!(should_skip("node_modules"));
        assert!(!should_skip("src"));
    }
}
