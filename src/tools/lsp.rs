//! LSP tools — expose language server capabilities as agent tools.
//!
//! These tools require an `LspManager` to be initialized and passed
//! during registration. They are gated behind the `lsp` feature.

use echo_core::error::Result;
use echo_core::lsp::{DiagnosticSeverity, LspClient, Position};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use echo_integration::lsp::LspManager;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

type SharedLspManager = Arc<RwLock<LspManager>>;

// ── lsp_diagnostics ─────────────────────────────────────────────────

/// Get diagnostics (errors, warnings) for a file from the language server.
pub struct LspDiagnosticsTool {
    lsp_manager: SharedLspManager,
}

impl LspDiagnosticsTool {
    pub fn new(lsp_manager: SharedLspManager) -> Self {
        Self { lsp_manager }
    }
}

impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        "Get diagnostics (errors, warnings, hints) for a file from the language server. \
         The file must be open in a language server. Returns a list of diagnostics \
         with severity, line numbers, and messages."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to get diagnostics for"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return Ok(ToolResult::error("file_path is required")),
            };

            let uri = path_to_uri(file_path);
            let manager = self.lsp_manager.read().await;

            let Some((_lang, client)) = manager.get_client_for_file(file_path).await else {
                return Ok(ToolResult::error(format!(
                    "No language server running for file: {file_path}. \
                     Start one with /lsp start <language>."
                )));
            };

            let client = client.read().await;
            match client.diagnostics(&uri).await {
                Ok(diagnostics) => {
                    if diagnostics.is_empty() {
                        return Ok(ToolResult::success("No diagnostics found. File is clean."));
                    }

                    let mut output = format!(
                        "Diagnostics for {file_path} ({} issues):\n\n",
                        diagnostics.len()
                    );
                    for diag in &diagnostics {
                        let severity = match diag.severity {
                            DiagnosticSeverity::Error => "ERROR",
                            DiagnosticSeverity::Warning => "WARNING",
                            DiagnosticSeverity::Information => "INFO",
                            DiagnosticSeverity::Hint => "HINT",
                        };
                        let line = diag.range.start.line + 1;
                        let col = diag.range.start.character + 1;
                        output.push_str(&format!(
                            "  [{severity}] line {line}:{col} — {}\n",
                            diag.message
                        ));
                    }

                    Ok(ToolResult::success(output))
                }
                Err(e) => Ok(ToolResult::error(format!("LSP diagnostics error: {e}"))),
            }
        })
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}

// ── lsp_goto_definition ─────────────────────────────────────────────

/// Find the definition of a symbol at a given position.
pub struct LspGotoDefinitionTool {
    lsp_manager: SharedLspManager,
}

impl LspGotoDefinitionTool {
    pub fn new(lsp_manager: SharedLspManager) -> Self {
        Self { lsp_manager }
    }
}

impl Tool for LspGotoDefinitionTool {
    fn name(&self) -> &str {
        "lsp_goto_definition"
    }

    fn description(&self) -> &str {
        "Find the definition of a symbol (function, class, variable) at a given position \
         in a file. Returns the file path and line number of the definition."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "line": { "type": "integer", "description": "Line number (1-indexed)" },
                "column": { "type": "integer", "description": "Column number (1-indexed)" }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return Ok(ToolResult::error("file_path is required")),
            };
            let line = match params.get("line").and_then(|v| v.as_u64()) {
                Some(l) => l as u32,
                None => return Ok(ToolResult::error("line is required")),
            };
            let column = match params.get("column").and_then(|v| v.as_u64()) {
                Some(c) => c as u32,
                None => return Ok(ToolResult::error("column is required")),
            };

            let uri = path_to_uri(file_path);
            let position = Position {
                line: line.saturating_sub(1),
                character: column.saturating_sub(1),
            };

            let manager = self.lsp_manager.read().await;
            let Some((_lang, client)) = manager.get_client_for_file(file_path).await else {
                return Ok(ToolResult::error(format!(
                    "No language server for: {file_path}"
                )));
            };

            let client = client.read().await;
            match client.goto_definition(&uri, position).await {
                Ok(locations) => {
                    if locations.is_empty() {
                        return Ok(ToolResult::success("No definition found."));
                    }

                    let mut output = String::from("Definitions found:\n\n");
                    for loc in &locations {
                        let path = uri_to_path(&loc.uri);
                        output.push_str(&format!(
                            "  {}:{}:{}\n",
                            path,
                            loc.range.start.line + 1,
                            loc.range.start.character + 1
                        ));
                    }

                    Ok(ToolResult::success(output))
                }
                Err(e) => Ok(ToolResult::error(format!("LSP goto_definition error: {e}"))),
            }
        })
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}

// ── lsp_find_references ──────────────────────────────────────────────

/// Find all references to a symbol.
pub struct LspFindReferencesTool {
    lsp_manager: SharedLspManager,
}

impl LspFindReferencesTool {
    pub fn new(lsp_manager: SharedLspManager) -> Self {
        Self { lsp_manager }
    }
}

impl Tool for LspFindReferencesTool {
    fn name(&self) -> &str {
        "lsp_find_references"
    }

    fn description(&self) -> &str {
        "Find all references to a symbol (function, class, variable) across the codebase. \
         Returns file paths and line numbers of all usages."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "line": { "type": "integer", "description": "Line number (1-indexed)" },
                "column": { "type": "integer", "description": "Column number (1-indexed)" }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return Ok(ToolResult::error("file_path is required")),
            };
            let line = match params.get("line").and_then(|v| v.as_u64()) {
                Some(l) => l as u32,
                None => return Ok(ToolResult::error("line is required")),
            };
            let column = match params.get("column").and_then(|v| v.as_u64()) {
                Some(c) => c as u32,
                None => return Ok(ToolResult::error("column is required")),
            };

            let uri = path_to_uri(file_path);
            let position = Position {
                line: line.saturating_sub(1),
                character: column.saturating_sub(1),
            };

            let manager = self.lsp_manager.read().await;
            let Some((_lang, client)) = manager.get_client_for_file(file_path).await else {
                return Ok(ToolResult::error(format!(
                    "No language server for: {file_path}"
                )));
            };

            let client = client.read().await;
            match client.find_references(&uri, position).await {
                Ok(locations) => {
                    if locations.is_empty() {
                        return Ok(ToolResult::success("No references found."));
                    }

                    let mut output = format!("References found ({} total):\n\n", locations.len());
                    for loc in &locations {
                        let path = uri_to_path(&loc.uri);
                        output.push_str(&format!(
                            "  {}:{}:{}\n",
                            path,
                            loc.range.start.line + 1,
                            loc.range.start.character + 1
                        ));
                    }

                    Ok(ToolResult::success(output))
                }
                Err(e) => Ok(ToolResult::error(format!("LSP find_references error: {e}"))),
            }
        })
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}

// ── lsp_hover ────────────────────────────────────────────────────────

/// Get hover information (type, docs) for a symbol.
pub struct LspHoverTool {
    lsp_manager: SharedLspManager,
}

impl LspHoverTool {
    pub fn new(lsp_manager: SharedLspManager) -> Self {
        Self { lsp_manager }
    }
}

impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn description(&self) -> &str {
        "Get hover information for a symbol at a given position. \
         Returns type information, documentation, and signatures."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "line": { "type": "integer", "description": "Line number (1-indexed)" },
                "column": { "type": "integer", "description": "Column number (1-indexed)" }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return Ok(ToolResult::error("file_path is required")),
            };
            let line = match params.get("line").and_then(|v| v.as_u64()) {
                Some(l) => l as u32,
                None => return Ok(ToolResult::error("line is required")),
            };
            let column = match params.get("column").and_then(|v| v.as_u64()) {
                Some(c) => c as u32,
                None => return Ok(ToolResult::error("column is required")),
            };

            let uri = path_to_uri(file_path);
            let position = Position {
                line: line.saturating_sub(1),
                character: column.saturating_sub(1),
            };

            let manager = self.lsp_manager.read().await;
            let Some((_lang, client)) = manager.get_client_for_file(file_path).await else {
                return Ok(ToolResult::error(format!(
                    "No language server for: {file_path}"
                )));
            };

            let client = client.read().await;
            match client.hover(&uri, position).await {
                Ok(Some(info)) => Ok(ToolResult::success(info.contents)),
                Ok(None) => Ok(ToolResult::success("No hover information available.")),
                Err(e) => Ok(ToolResult::error(format!("LSP hover error: {e}"))),
            }
        })
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}

// ── lsp_status ───────────────────────────────────────────────────────

/// Show status of all language servers.
pub struct LspStatusTool {
    lsp_manager: SharedLspManager,
}

impl LspStatusTool {
    pub fn new(lsp_manager: SharedLspManager) -> Self {
        Self { lsp_manager }
    }
}

impl Tool for LspStatusTool {
    fn name(&self) -> &str {
        "lsp_status"
    }

    fn description(&self) -> &str {
        "Show the status of all configured and running language servers. \
         Displays which servers are active, their PID, and any errors."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn execute<'a>(&'a self, _params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let manager = self.lsp_manager.read().await;
            let statuses = manager.status_all().await;

            if statuses.is_empty() {
                return Ok(ToolResult::success(
                    "No language servers configured. Create a .lsp.yaml file to configure servers.",
                ));
            }

            let mut output = format!("Language Servers ({} configured):\n\n", statuses.len());
            for status in &statuses {
                let state = if status.running && status.initialized {
                    "running"
                } else if status.running {
                    "starting"
                } else {
                    "stopped"
                };

                output.push_str(&format!("  {} [{}]", status.language, state));
                if let Some(pid) = status.pid {
                    output.push_str(&format!(" (pid: {pid})"));
                }
                output.push('\n');

                if let Some(ref err) = status.last_error {
                    output.push_str(&format!("    Error: {err}\n"));
                }
            }

            Ok(ToolResult::success(output))
        })
    }

    fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
        echo_core::tools::ToolRiskLevel::ReadOnly
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Convert a file path to a file:// URI.
fn path_to_uri(path: &str) -> String {
    if path.starts_with("file://") {
        path.to_string()
    } else {
        format!("file://{path}")
    }
}

/// Convert a file:// URI back to a path.
fn uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// Register all LSP tools with the given LspManager.
pub fn register_lsp_tools(
    tool_manager: &mut dyn echo_core::tools::ToolRegistrar,
    lsp_manager: SharedLspManager,
) {
    tool_manager.register(Box::new(LspDiagnosticsTool::new(lsp_manager.clone())));
    tool_manager.register(Box::new(LspGotoDefinitionTool::new(lsp_manager.clone())));
    tool_manager.register(Box::new(LspFindReferencesTool::new(lsp_manager.clone())));
    tool_manager.register(Box::new(LspHoverTool::new(lsp_manager.clone())));
    tool_manager.register(Box::new(LspStatusTool::new(lsp_manager)));
}
