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
#[cfg(feature = "framework-content-guard")]
use echo_sdk_protocol::scalar::WireField;
use echo_sdk_protocol::scalar::WireValue;
#[cfg(feature = "framework-project-rules")]
use std::path::Path;
use std::sync::Arc;

use crate::factory::SessionAuthorityServices;

use super::super::wire;
use super::SessionFacadeRuntime;

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
struct FamilyTools {
    tools: Vec<(String, Arc<dyn Tool>)>,
    unavailable: Vec<(String, String)>,
}

/// Session-owned RAG dependencies. The vector store must be shared by the
/// index and search tools of one ACP session, while remaining isolated from
/// every other session on the same Host connection.
#[cfg(feature = "framework-rag")]
pub(crate) struct RagToolSet {
    store: echo_agent::tools::rag::RagStore,
    embedder: Arc<dyn echo_agent::memory::Embedder>,
}

#[cfg(feature = "framework-rag")]
pub(crate) struct RagToolSetError {
    code: ExtensionErrorCode,
    message: String,
}

#[cfg(feature = "framework-rag")]
impl RagToolSetError {
    fn feature_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: ExtensionErrorCode::FeatureUnavailable,
            message: message.into(),
        }
    }

    fn invalid_config(message: impl Into<String>) -> Self {
        Self {
            code: ExtensionErrorCode::InvalidConfig,
            message: message.into(),
        }
    }

    pub(crate) fn into_sdk_error(self, operation: &str) -> EchoSdkError {
        wire::sdk_error(
            self.code,
            self.message,
            Retryability::Never,
            "_echo_agent/tool/op",
        )
        .with_operation(operation)
    }
}

#[cfg(feature = "framework-rag")]
impl RagToolSet {
    pub(crate) fn from_env() -> Result<Self, RagToolSetError> {
        let api_key = ["EMBEDDING_APIKEY", "EMBEDDING_API_KEY", "OPENAI_API_KEY"]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .ok_or_else(|| {
                RagToolSetError::feature_unavailable(
                    "RAG embedding is unavailable: configure EMBEDDING_APIKEY, EMBEDDING_API_KEY, or OPENAI_API_KEY",
                )
            })?;
        let model = std::env::var("EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());
        if model.trim().is_empty() {
            return Err(RagToolSetError::invalid_config(
                "RAG embedding configuration has an empty EMBEDDING_MODEL",
            ));
        }
        if let Ok(endpoint) = std::env::var("EMBEDDING_BASEURL")
            && endpoint.trim().is_empty()
        {
            return Err(RagToolSetError::invalid_config(
                "RAG embedding configuration has an empty EMBEDDING_BASEURL",
            ));
        }
        if let Ok(endpoint) = std::env::var("EMBEDDING_API_URL")
            && endpoint.trim().is_empty()
        {
            return Err(RagToolSetError::invalid_config(
                "RAG embedding configuration has an empty EMBEDDING_API_URL",
            ));
        }

        // `HttpEmbedder` reads the same validated environment. The key is
        // intentionally not retained in this set by value; the embedder owns
        // its request client and each Session gets its own instance.
        let _ = api_key;
        Ok(Self {
            store: echo_agent::tools::rag::RagStore::new(),
            embedder: Arc::new(echo_agent::memory::HttpEmbedder::from_env()),
        })
    }

    fn tools(&self) -> Vec<(String, Arc<dyn Tool>)> {
        vec![
            (
                "rag_index".to_string(),
                Arc::new(echo_agent::tools::rag::RagIndexTool::new(
                    self.embedder.clone(),
                    self.store.clone(),
                )),
            ),
            (
                "rag_search".to_string(),
                Arc::new(echo_agent::tools::rag::RagSearchTool::new(
                    self.embedder.clone(),
                    self.store.clone(),
                )),
            ),
        ]
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            store: echo_agent::tools::rag::RagStore::new(),
            embedder: Arc::new(TestEmbedder),
        }
    }
}

#[cfg(all(test, feature = "framework-rag"))]
struct TestEmbedder;

#[cfg(all(test, feature = "framework-rag"))]
impl echo_agent::memory::Embedder for TestEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<Vec<f32>>> {
        Box::pin(async move {
            if text.contains("alpha") {
                Ok(vec![1.0, 0.0])
            } else {
                Ok(vec![0.0, 1.0])
            }
        })
    }
}

fn family_tools(family: &str) -> FamilyTools {
    #[allow(unused_mut)]
    let mut unavailable = Vec::new();
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
            match echo_agent::tools::media::ImageFetchTool::new() {
                Ok(fetch) => tools.push(Box::new(fetch)),
                Err(error) => unavailable.push(("image_fetch".to_string(), error.to_string())),
            }
            #[cfg(feature = "framework-data")]
            tools.push(Box::new(echo_agent::tools::media::excel::ExcelLoadTool));
            tools
        }
        _ => Vec::new(),
    };
    FamilyTools {
        tools: tools
            .into_iter()
            .map(|tool| {
                let name = tool.name().to_string();
                (name, Arc::from(tool))
            })
            .collect(),
        unavailable,
    }
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
#[allow(unused_variables)]
pub(crate) async fn dispatch_tool(
    family: &str,
    authorities: &SessionAuthorityServices,
    owner: &str,
    facade: &SessionFacadeRuntime,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let operation = request.operation.as_str();
    let prefix = format!("{family}.");
    // The canonical catalog uses the framework tool name (`git_status`),
    // while accepting the older family-qualified spelling (`git.git_status`)
    // would be a harmless source-compatible alias. Both resolve to the same
    // single framework Tool instance.
    let tool_name = operation.strip_prefix(&prefix).unwrap_or(operation);
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
    #[allow(unused_mut)]
    let mut family_tools = family_tools(family);
    #[cfg(feature = "framework-rag")]
    if family == "rag" && matches!(tool_name, "rag_index" | "rag_search") {
        let rag_tools = facade.rag_tools_for_session(owner, operation)?;
        family_tools.tools.extend(rag_tools.tools());
    }
    if let Some((_, reason)) = family_tools
        .unavailable
        .iter()
        .find(|(name, _)| name == tool_name)
    {
        return Err(wire::sdk_error(
            ExtensionErrorCode::FeatureUnavailable,
            format!("tool {family}.{tool_name} is unavailable: {reason}"),
            Retryability::Never,
            "_echo_agent/tool/op",
        )
        .with_operation(operation));
    }
    let tool = family_tools
        .tools
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
        working_dir: Some(authorities.working_dir.clone()),
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
        "content-guard.detect_exact" => {
            let content = text(0)?;
            return WireValue::from_json(
                serde_json::to_value(guard.detect(&content))
                    .map_err(|error| invalid(operation, error.to_string()))?,
            )
            .map_err(|error| invalid(operation, error.to_string()));
        }
        "content-guard.redact_exact" => {
            return Ok(WireValue::String(guard.redact(&text(0)?)));
        }
        "content-guard.is_clean_exact" => {
            return Ok(WireValue::Bool(guard.is_clean(&text(0)?)));
        }
        "content-guard.check" => {
            if request.arguments.len() != 2 {
                return Err(invalid(
                    operation,
                    "content-guard.check accepts [mode, content]",
                ));
            }
            let mode = match text(0)?.as_str() {
                "detect" => ContentGuardMode::Detect,
                "reject" => ContentGuardMode::Reject,
                "redact" => ContentGuardMode::Redact,
                _ => return Err(invalid(operation, "content guard mode is invalid")),
            };
            let checked = ContentGuard::new(mode)
                .check(&text(1)?)
                .map_err(|error| invalid(operation, error.to_string()))?;
            return Ok(match checked {
                echo_agent::guard::content::ContentGuardResult::Pass => WireValue::Variant {
                    type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                    variant: "pass".to_string(),
                    fields: Vec::new(),
                },
                echo_agent::guard::content::ContentGuardResult::Detected { pii_types } => {
                    WireValue::Variant {
                        type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                        variant: "detected".to_string(),
                        fields: vec![WireField {
                            name: "pii_types".to_string(),
                            value: WireValue::List(
                                pii_types.into_iter().map(WireValue::String).collect(),
                            ),
                        }],
                    }
                }
                echo_agent::guard::content::ContentGuardResult::Rejected { pii_types } => {
                    WireValue::Variant {
                        type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                        variant: "rejected".to_string(),
                        fields: vec![WireField {
                            name: "pii_types".to_string(),
                            value: WireValue::List(
                                pii_types.into_iter().map(WireValue::String).collect(),
                            ),
                        }],
                    }
                }
                echo_agent::guard::content::ContentGuardResult::Redacted(content) => {
                    WireValue::Variant {
                        type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                        variant: "redacted".to_string(),
                        fields: vec![WireField {
                            name: "content".to_string(),
                            value: WireValue::String(content),
                        }],
                    }
                }
            });
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
        "project-rules.load" => match echo_agent::project_rules::load_project_rules(working_dir) {
            Some((path, content)) => Ok(WireValue::Variant {
                type_id: "core::option::Option<(Path,String)>".to_string(),
                variant: "some".to_string(),
                fields: vec![
                    WireField {
                        name: "path".to_string(),
                        value: WireValue::Path(
                            wire::path_to_wire(&path).map_err(|error| invalid(operation, error))?,
                        ),
                    },
                    WireField {
                        name: "content".to_string(),
                        value: WireValue::String(content),
                    },
                ],
            }),
            None => Ok(WireValue::Variant {
                type_id: "core::option::Option<(Path,String)>".to_string(),
                variant: "none".to_string(),
                fields: Vec::new(),
            }),
        },
        other => Err(invalid(
            other,
            "unknown project-rules operation; the family surface is closed",
        )),
    }
}

#[cfg(all(test, feature = "framework-rag"))]
mod tests {
    use super::*;

    fn tool(set: &RagToolSet, name: &str) -> Result<Arc<dyn Tool>, Box<dyn std::error::Error>> {
        set.tools()
            .into_iter()
            .find(|(tool_name, _)| tool_name == name)
            .map(|(_, tool)| tool)
            .ok_or_else(|| format!("missing RAG tool {name}").into())
    }

    #[tokio::test]
    async fn rag_index_and_search_share_one_session_store() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = RagToolSet::for_test();
        let index = tool(&first, "rag_index")?;
        let index_parameters: ToolParameters = serde_json::from_value(serde_json::json!({
            "content": "alpha session document",
            "source": "first",
        }))?;
        let indexed = index.execute(index_parameters).await?;
        assert!(indexed.success, "index failed: {}", indexed.output);

        let search = tool(&first, "rag_search")?;
        let search_parameters: ToolParameters = serde_json::from_value(serde_json::json!({
            "query": "alpha",
            "top_k": 1,
        }))?;
        let found = search.execute(search_parameters).await?;
        assert!(found.success, "search failed: {}", found.output);
        assert!(found.output.contains("session document"));

        let second = RagToolSet::for_test();
        let isolated_parameters: ToolParameters = serde_json::from_value(serde_json::json!({
            "query": "alpha",
            "top_k": 1,
        }))?;
        let isolated = tool(&second, "rag_search")?
            .execute(isolated_parameters)
            .await?;
        assert!(isolated.success);
        assert!(isolated.output.contains("No relevant documents"));
        Ok(())
    }
}
