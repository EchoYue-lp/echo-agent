//! Tool family adapters (plan 07, todo 5).
//!
//! The tool families (`_echo_agent/{files,shell,git,database,rag,chart,
//! media,data,statistics,research,web}/op`) share one dispatcher: each
//! operation names a real framework tool (`<family>.<tool_name>`), the
//! positional arguments are bound to the tool's own JSON Schema parameter
//! order, and execution goes through `Tool::execute_with_context` with
//! the session's working directory — the exact path the in-conversation
//! agent uses. Permission, sandbox, cwd and timeout behavior stay with
//! the tools themselves; the Host adds no second policy layer.
//!
//! `content-guard` and `project-rules` are library surfaces rather than
//! `Tool` implementations and get explicit handlers over the framework's
//! own functions.

use echo_agent::tools::{Tool, ToolContext, ToolParameters};
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::path::Path;

use super::super::wire;

/// The tool families this module serves; one entry per family method the
/// catalog owns. `testing` is deliberately absent: that module is mock
/// infrastructure for in-process tests, not a remote surface, so its
/// method stays the official method-not-found.
pub(crate) const TOOL_FAMILIES: &[&str] = &[
    "chart",
    "content-guard",
    "data",
    "database",
    "files",
    "git",
    "media",
    "project-rules",
    "rag",
    "research",
    "shell",
    "statistics",
    "web",
];

fn invalid(operation: &str, message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        "_echo_agent/tool/op",
    )
    .with_operation(operation)
}

/// Build this family's tool set. Tools are stateless per call, exactly
/// like the `StandardToolPack` constructs them; the only difference is
/// that the file-write tools are included so the facade surface matches
/// the framework's actual capability menu.
fn family_tools(family: &str) -> Vec<(String, std::sync::Arc<dyn Tool>)> {
    let tools: Vec<Box<dyn Tool>> = match family {
        #[cfg(feature = "framework-files")]
        "files" => vec![
            Box::new(echo_agent::tools::files::files::ReadFileTool::new()),
            Box::new(echo_agent::tools::files::files::ListDirTool::new()),
            Box::new(echo_agent::tools::files::grep::GrepTool::new()),
            Box::new(echo_agent::tools::files::glob::GlobTool::new()),
            Box::new(echo_agent::tools::files::apply_patch::ApplyPatchTool::new()),
            Box::new(echo_agent::tools::files::diff::DiffTool::new()),
            Box::new(echo_agent::tools::files::repo_map::RepoMapTool::new()),
            Box::new(echo_agent::tools::files::code_search::CodeSearchTool::new()),
            Box::new(echo_agent::tools::files::files::CreateFileTool::new()),
            Box::new(echo_agent::tools::files::files::DeleteFileTool::new()),
            Box::new(echo_agent::tools::files::files::WriteFileTool::new()),
            Box::new(echo_agent::tools::files::files::AppendFileTool::new()),
            Box::new(echo_agent::tools::files::files::UpdateFileTool::new()),
            Box::new(echo_agent::tools::files::files::MoveFileTool::new()),
        ],
        #[cfg(feature = "framework-shell")]
        "shell" => vec![Box::new(echo_agent::tools::shell::ShellTool::new())],
        #[cfg(feature = "framework-git")]
        "git" => vec![
            Box::new(echo_agent::tools::git::GitStatusTool),
            Box::new(echo_agent::tools::git::GitDiffTool),
            Box::new(echo_agent::tools::git::GitLogTool),
            Box::new(echo_agent::tools::git::GitBlameTool),
            Box::new(echo_agent::tools::git::GitBranchTool),
            Box::new(echo_agent::tools::git::GitCommitTool),
            Box::new(echo_agent::tools::git::EnterWorktreeTool),
            Box::new(echo_agent::tools::git::ExitWorktreeTool),
            Box::new(echo_agent::tools::git::ListWorktreesTool),
        ],
        #[cfg(feature = "framework-database")]
        "database" => vec![
            Box::new(echo_agent::tools::database::SqlQueryTool),
            Box::new(echo_agent::tools::database::ListTablesTool),
            Box::new(echo_agent::tools::database::DescribeTableTool),
        ],
        #[cfg(feature = "framework-rag")]
        "rag" => vec![Box::new(echo_agent::tools::rag::RagChunkDocumentTool)],
        #[cfg(feature = "framework-chart")]
        "chart" => vec![Box::new(echo_agent::tools::chart::GenerateChartTool)],
        #[cfg(feature = "framework-web")]
        "web" => vec![
            Box::new(echo_agent::tools::web::WebFetchTool::new()),
            Box::new(echo_agent::tools::web::WebExtractTool),
            Box::new(echo_agent::tools::web::WebSearchTool::with_duckduckgo()),
        ],
        #[cfg(feature = "framework-statistics")]
        "statistics" => {
            vec![Box::new(
                echo_agent::tools::statistics::ExploratoryStatisticsTool::default(),
            )]
        }
        #[cfg(feature = "framework-research")]
        "research" => vec![
            Box::new(echo_agent::tools::research::ArxivSearchTool),
            Box::new(echo_agent::tools::research::SemanticScholarSearchTool),
            Box::new(echo_agent::tools::research::PubMedSearchTool),
            Box::new(echo_agent::tools::research::ClinicalTrialsSearchTool),
            Box::new(echo_agent::tools::research::PdfFetchTool),
            Box::new(echo_agent::tools::research::BibtexGenerateTool),
        ],
        #[cfg(feature = "framework-data")]
        "data" => vec![
            Box::new(echo_agent::tools::data::DataReadTool),
            Box::new(echo_agent::tools::data::DataFilterTool),
            Box::new(echo_agent::tools::data::DataAggregateTool),
            Box::new(echo_agent::tools::data::DataStatsTool),
            Box::new(echo_agent::tools::data::DataTransformTool),
            Box::new(echo_agent::tools::data::DataExportTool),
            Box::new(echo_agent::tools::data::DataProfileTool),
            Box::new(echo_agent::tools::data::DataTopNTool),
            Box::new(echo_agent::tools::data::DataContributionTool),
            Box::new(echo_agent::tools::data::DataBinTool),
            Box::new(echo_agent::tools::data::DataRatioTool),
            Box::new(echo_agent::tools::data::DataMultiReadTool),
            Box::new(echo_agent::tools::data::DataJoinTool),
            Box::new(echo_agent::tools::data::CorrelateTool),
            Box::new(echo_agent::tools::data::PivotTool),
            Box::new(echo_agent::tools::data_quality::MissingValueAnalysisTool),
            Box::new(echo_agent::tools::data_quality::OutlierDetectionTool),
            Box::new(echo_agent::tools::data_quality::ConsistencyCheckTool),
        ],
        #[cfg(feature = "framework-media")]
        "media" => {
            let mut tools: Vec<Box<dyn Tool>> = vec![
                Box::new(echo_agent::tools::media::image::ViewImageTool::new()),
                Box::new(echo_agent::tools::media::pdf::PdfExtractTool),
                Box::new(echo_agent::tools::media::pdf::PdfInfoTool),
                Box::new(echo_agent::tools::media::excel::ExcelReadTool),
                Box::new(echo_agent::tools::media::excel::ExcelInfoTool),
                Box::new(echo_agent::tools::media::excel::ExcelToCsvTool),
                Box::new(echo_agent::tools::media::excel::ExcelProfileTool),
                Box::new(echo_agent::tools::media::excel::ExcelWriteTool),
                Box::new(echo_agent::tools::media::word::WordReadTool),
                Box::new(echo_agent::tools::media::word::WordInfoTool),
                Box::new(echo_agent::tools::media::word::WordStructureTool),
                Box::new(echo_agent::tools::media::text::TextSearchTool),
                Box::new(echo_agent::tools::media::text::TextStatsTool),
                Box::new(echo_agent::tools::media::text::TextProcessTool),
                Box::new(echo_agent::tools::media::text::TextExportTool),
            ];
            if let Ok(fetch) = echo_agent::tools::media::ImageFetchTool::new() {
                tools.push(Box::new(fetch));
            }
            #[cfg(feature = "framework-data")]
            tools.push(Box::new(echo_agent::tools::media::excel::ExcelLoadTool));
            tools
        }
        _ => Vec::new(),
    };
    tools
        .into_iter()
        .map(|tool| {
            let name = tool.name().to_string();
            (name, std::sync::Arc::from(tool))
        })
        .collect()
}

/// Bind positional wire arguments onto the tool's own JSON Schema
/// parameter order. The schema is the tool's single source of truth; the
/// Host never re-declares parameter names.
fn bind_parameters(
    operation: &str,
    tool: &dyn Tool,
    arguments: &[serde_json::Value],
) -> Result<ToolParameters, EchoSdkError> {
    let schema = tool.parameters();
    let keys: Vec<String> = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    let mut parameters = ToolParameters::new();
    for (key, value) in keys.into_iter().zip(arguments.iter()) {
        if !value.is_null() {
            parameters.insert(key, value.clone());
        }
    }
    if parameters.len() < arguments.iter().filter(|value| !value.is_null()).count() {
        return Err(invalid(
            operation,
            "more arguments than the tool's schema declares",
        ));
    }
    Ok(parameters)
}

/// Dispatch one tool-family operation. `working_dir` is the session's
/// cwd; tools resolve relative paths and sandboxes through it.
pub(crate) async fn dispatch_tool(
    family: &str,
    working_dir: &Path,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let operation = request.operation.as_str();
    let prefix = format!("{family}.");
    let tool_name = operation.strip_prefix(&prefix).ok_or_else(|| {
        invalid(
            operation,
            format!("operation must be prefixed with the family name ({prefix})"),
        )
    })?;
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            value.clone().into_json().map_err(|error| {
                invalid(
                    operation,
                    format!("argument {position} is not a lossless wire value: {error}"),
                )
            })
        })
        .collect::<Result<_, _>>()?;
    let tool = family_tools(family)
        .into_iter()
        .find(|(name, _)| name == tool_name)
        .map(|(_, tool)| tool)
        .ok_or_else(|| {
            invalid(
                operation,
                format!("unknown {family} tool {tool_name}; the family surface is closed"),
            )
        })?;
    let parameters = bind_parameters(operation, tool.as_ref(), &arguments)?;
    let context = ToolContext {
        working_dir: Some(working_dir.to_path_buf()),
        ..ToolContext::default()
    };
    let result = tool
        .execute_with_context(parameters, &context)
        .await
        .map_err(|error| {
            wire::sdk_error(
                ExtensionErrorCode::FrameworkError,
                wire::bounded_framework_message(&error.to_string()),
                Retryability::Never,
                "_echo_agent/tool/op",
            )
            .with_operation(operation)
        })?;
    let value = serde_json::json!({
        "success": result.success,
        "output": result.output,
        "error": result.error,
    });
    WireValue::from_json(value).map_err(|error| invalid(operation, error.to_string()))
}

/// Dispatch one content-guard operation (PII detection over the
/// framework's own ContentGuard).
#[cfg(feature = "framework-content-guard")]
pub(crate) fn dispatch_content_guard(
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    use echo_agent::guard::content::{ContentGuard, ContentGuardMode};
    let operation = request.operation.as_str();
    let first = |position: usize, what: &str| {
        request.arguments.get(position).cloned().ok_or_else(|| {
            invalid(
                operation,
                format!("operation requires {what} at argument {position}"),
            )
        })
    };
    let text = |position: usize| -> Result<String, EchoSdkError> {
        let value = first(position, "text")?;
        value
            .into_json()
            .map_err(|error| invalid(operation, error.to_string()))?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| invalid(operation, "text argument must be a string"))
    };
    let guard = ContentGuard::new(ContentGuardMode::Redact);
    let result = match operation {
        "content-guard.detect" => {
            let content = text(0)?;
            let matches = guard.detect(&content);
            serde_json::json!({"matches": matches})
        }
        "content-guard.redact" => {
            let content = text(0)?;
            serde_json::json!({"redacted": guard.redact(&content)})
        }
        "content-guard.is_clean" => {
            let content = text(0)?;
            serde_json::json!({"clean": guard.is_clean(&content)})
        }
        other => {
            return Err(invalid(
                other,
                "unknown content-guard operation; the family surface is closed",
            ));
        }
    };
    WireValue::from_json(result).map_err(|error| invalid(operation, error.to_string()))
}

/// Dispatch one project-rules operation (instruction resolution over the
/// framework's own InstructionResolver).
#[cfg(feature = "framework-project-rules")]
pub(crate) fn dispatch_project_rules(
    working_dir: &Path,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let operation = request.operation.as_str();
    match operation {
        "project-rules.resolve" => {
            let resolver = echo_agent::project_rules::InstructionResolver::new(working_dir);
            let resolved = resolver.resolve();
            let value = serde_json::json!({
                "content": resolved.content,
                "sources": resolved.sources,
                "is_empty": resolved.is_empty(),
            });
            WireValue::from_json(value).map_err(|error| invalid(operation, error.to_string()))
        }
        other => Err(invalid(
            other,
            "unknown project-rules operation; the family surface is closed",
        )),
    }
}
