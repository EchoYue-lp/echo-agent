use echo_core::error::ToolError;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolFailureCategory, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const DEFAULT_ARTIFACT_CONTENT_TOKENS: usize = 3_500;
const MAX_ARTIFACT_CONTENT_TOKENS: usize = 3_500;
const ARTIFACT_READ_AHEAD_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactCursor {
    offset: u64,
    file_bytes: u64,
    modified_nanos: u128,
    sha256: String,
}

impl ArtifactCursor {
    fn encode(self) -> String {
        format!(
            "v2:{}:{}:{}:{}",
            self.offset, self.file_bytes, self.modified_nanos, self.sha256
        )
    }

    fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split(':');
        if parts.next() != Some("v2") {
            return Err("artifact cursor has an unsupported version".to_string());
        }
        let offset = parse_cursor_number(parts.next(), "offset")?;
        let file_bytes = parse_cursor_number(parts.next(), "file size")?;
        let modified_nanos = parse_cursor_number(parts.next(), "modified time")?;
        let sha256 = parts
            .next()
            .filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
            .ok_or_else(|| "artifact cursor has an invalid SHA-256".to_string())?
            .to_ascii_lowercase();
        if parts.next().is_some() {
            return Err("artifact cursor contains unexpected fields".to_string());
        }
        Ok(Self {
            offset,
            file_bytes,
            modified_nanos,
            sha256,
        })
    }
}

fn parse_cursor_number<T>(value: Option<&str>, label: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .ok_or_else(|| format!("artifact cursor is missing {label}"))?
        .parse::<T>()
        .map_err(|_| format!("artifact cursor has an invalid {label}"))
}

fn positive_token_limit(parameters: &ToolParameters) -> Result<usize, String> {
    let Some(value) = parameters.get("max_tokens") else {
        return Ok(DEFAULT_ARTIFACT_CONTENT_TOKENS);
    };
    let raw = value
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| "'max_tokens' must be a positive integer".to_string())?;
    let requested = usize::try_from(raw).map_err(|_| "'max_tokens' is too large".to_string())?;
    Ok(requested.min(MAX_ARTIFACT_CONTENT_TOKENS))
}

fn modified_nanos(modified: SystemTime) -> u128 {
    modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

async fn resolve_artifact_path(
    requested: &str,
    ctx: &echo_core::tools::ToolContext,
) -> Result<PathBuf, String> {
    let config = ctx.output_artifacts.as_ref().ok_or_else(|| {
        "read_artifact is unavailable because this run has no artifact store".to_string()
    })?;
    let root = tokio::fs::canonicalize(&config.root_dir)
        .await
        .map_err(|error| format!("cannot resolve artifact root: {error}"))?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        config.root_dir.join(requested_path)
    };
    let path = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|error| format!("cannot resolve artifact '{}': {error}", candidate.display()))?;
    if !path.starts_with(&root) {
        return Err(format!(
            "artifact '{}' is outside the configured artifact root",
            path.display()
        ));
    }
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| format!("cannot inspect artifact '{}': {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("artifact '{}' is not a file", path.display()));
    }
    Ok(path)
}

fn decode_utf8_prefix(bytes: Vec<u8>) -> Result<String, String> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            String::from_utf8(bytes)
                .map_err(|nested| format!("artifact contains invalid UTF-8: {nested}"))
        }
        Err(error) => Err(format!("artifact contains invalid UTF-8: {error}")),
    }
}

fn token_bounded_prefix(text: &str, max_tokens: usize) -> String {
    let tokenizer = HeuristicTokenizer;
    if tokenizer.count_tokens(text) <= max_tokens {
        return text.to_string();
    }

    let mut low = 0_usize;
    let mut high = text.chars().count();
    while low < high {
        let middle = low.saturating_add(high).saturating_add(1) / 2;
        let candidate = text.chars().take(middle).collect::<String>();
        if tokenizer.count_tokens(&candidate) <= max_tokens {
            low = middle;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    let character_limit = if low == 0 && !text.is_empty() { 1 } else { low };
    text.chars().take(character_limit).collect()
}

fn file_identity(metadata: &std::fs::Metadata) -> (u64, u128) {
    (
        metadata.len(),
        metadata.modified().map(modified_nanos).unwrap_or(0),
    )
}

async fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("cannot open artifact for hashing: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; ARTIFACT_READ_AHEAD_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| format!("cannot hash artifact: {error}"))?;
        if read == 0 {
            break;
        }
        if let Some(chunk) = buffer.get(..read) {
            hasher.update(chunk);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Read immutable tool-output artifacts by an opaque byte cursor.
pub struct ReadArtifactTool;

impl Tool for ReadArtifactTool {
    fn name(&self) -> &str {
        "read_artifact"
    }

    fn description(&self) -> &str {
        "Read the complete spilled output of a previous tool call in bounded UTF-8 pages. Use the returned next_cursor until complete."
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
                    "description": "Exact artifact path returned by a previous tool result"
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque next_cursor returned by the previous read_artifact page"
                },
                "max_tokens": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_ARTIFACT_CONTENT_TOKENS,
                    "description": "Maximum content tokens for this page (default and hard maximum: 3500)"
                },
                "expected_sha256": {
                    "type": "string",
                    "description": "Optional full SHA-256 from the spill notice; verified on the first page"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, echo_core::error::Result<ToolResult>> {
        Box::pin(async move {
            let allowed_parameters = ["path", "cursor", "max_tokens", "expected_sha256"];
            let mut unknown = parameters
                .keys()
                .filter(|key| !allowed_parameters.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            unknown.sort();
            if !unknown.is_empty() {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Unknown read_artifact parameter(s): {}", unknown.join(", ")),
                ));
            }

            let path_value = parameters
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;
            let max_tokens = match positive_token_limit(&parameters) {
                Ok(value) => value,
                Err(message) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        message,
                    ));
                }
            };
            let path = match resolve_artifact_path(path_value, ctx).await {
                Ok(path) => path,
                Err(message) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        message,
                    ));
                }
            };
            let metadata =
                tokio::fs::metadata(&path)
                    .await
                    .map_err(|error| ToolError::ExecutionFailed {
                        tool: "read_artifact".to_string(),
                        message: format!("cannot inspect artifact: {error}"),
                    })?;
            let (file_bytes, modified) = file_identity(&metadata);

            let cursor = match parameters.get("cursor").and_then(Value::as_str) {
                Some(value) => match ArtifactCursor::parse(value) {
                    Ok(cursor) => Some(cursor),
                    Err(message) => {
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::InvalidArguments,
                            message,
                        ));
                    }
                },
                None => None,
            };
            if let Some(cursor) = cursor.as_ref()
                && (cursor.file_bytes != file_bytes || cursor.modified_nanos != modified)
            {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    "Artifact changed after the previous page; restart from the first page.",
                ));
            }
            let start = cursor.as_ref().map(|value| value.offset).unwrap_or(0);
            if start > file_bytes {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    format!("Artifact cursor exceeds file size ({file_bytes} bytes)"),
                ));
            }

            let sha256 = match cursor.as_ref() {
                Some(cursor) => cursor.sha256.clone(),
                None => sha256_file(&path)
                    .await
                    .map_err(|message| ToolError::ExecutionFailed {
                        tool: "read_artifact".to_string(),
                        message,
                    })?,
            };
            if cursor.is_none() {
                let after_hash = tokio::fs::metadata(&path).await.map_err(|error| {
                    ToolError::ExecutionFailed {
                        tool: "read_artifact".to_string(),
                        message: format!("cannot re-inspect artifact after hashing: {error}"),
                    }
                })?;
                if file_identity(&after_hash) != (file_bytes, modified) {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::Transient,
                        "Artifact changed while its first page was being verified; retry from the first page.",
                    ));
                }
                if let Some(expected) = parameters.get("expected_sha256").and_then(Value::as_str)
                    && !sha256.eq_ignore_ascii_case(expected)
                {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        format!("Artifact SHA-256 mismatch: expected {expected}, actual {sha256}"),
                    ));
                }
            }

            let mut file =
                tokio::fs::File::open(&path)
                    .await
                    .map_err(|error| ToolError::ExecutionFailed {
                        tool: "read_artifact".to_string(),
                        message: format!("cannot open artifact: {error}"),
                    })?;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|error| ToolError::ExecutionFailed {
                    tool: "read_artifact".to_string(),
                    message: format!("cannot seek artifact: {error}"),
                })?;
            let remaining = file_bytes.saturating_sub(start);
            let read_limit =
                remaining.min(u64::try_from(ARTIFACT_READ_AHEAD_BYTES).unwrap_or(u64::MAX));
            let mut bytes = Vec::new();
            file.take(read_limit)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| ToolError::ExecutionFailed {
                    tool: "read_artifact".to_string(),
                    message: format!("cannot read artifact: {error}"),
                })?;
            let decoded = match decode_utf8_prefix(bytes) {
                Ok(text) => text,
                Err(message) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::InvalidArguments,
                        message,
                    ));
                }
            };
            let after_read =
                tokio::fs::metadata(&path)
                    .await
                    .map_err(|error| ToolError::ExecutionFailed {
                        tool: "read_artifact".to_string(),
                        message: format!("cannot re-inspect artifact after reading: {error}"),
                    })?;
            if file_identity(&after_read) != (file_bytes, modified) {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::Transient,
                    "Artifact changed while a page was being read; restart from the first page.",
                ));
            }
            let content = token_bounded_prefix(&decoded, max_tokens);
            let consumed = u64::try_from(content.len()).unwrap_or(u64::MAX);
            if start < file_bytes && consumed == 0 {
                return Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    "Artifact cursor is not on a UTF-8 character boundary.",
                ));
            }
            let end = start.saturating_add(consumed).min(file_bytes);
            let has_more = end < file_bytes;
            let next_cursor = has_more.then(|| {
                ArtifactCursor {
                    offset: end,
                    file_bytes,
                    modified_nanos: modified,
                    sha256: sha256.clone(),
                }
                .encode()
            });
            let notice = match next_cursor.as_deref() {
                Some(next) => format!(
                    "\n\n[Artifact page: bytes {start}-{end} of {file_bytes}; continue with cursor={next}.]"
                ),
                None => format!("\n\n[Artifact complete: bytes {start}-{end} of {file_bytes}.]"),
            };
            let mut result =
                ToolResult::success(format!("{content}{notice}")).with_truncated(has_more);
            result
                .metadata
                .insert("artifact_path".to_string(), path.display().to_string());
            result
                .metadata
                .insert("start_byte".to_string(), start.to_string());
            result
                .metadata
                .insert("end_byte".to_string(), end.to_string());
            result
                .metadata
                .insert("total_bytes".to_string(), file_bytes.to_string());
            result
                .metadata
                .insert("total_known".to_string(), "true".to_string());
            result.metadata.insert("sha256".to_string(), sha256);
            if let Some(next) = next_cursor {
                result.metadata.insert("next_cursor".to_string(), next);
            }
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::artifact::{
        ToolOutputArtifactConfig, ToolOutputArtifactIdentity, persist_tool_output,
    };

    fn test_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "echo-read-artifact-{label}-{}-{}",
            std::process::id(),
            nonce
        ))
    }

    fn create_artifact(
        root: &Path,
        content: &str,
    ) -> echo_core::error::Result<echo_core::tools::artifact::ToolOutputArtifactRef> {
        let config = ToolOutputArtifactConfig::new(root, "test").threshold_bytes(1);
        persist_tool_output(
            config,
            ToolOutputArtifactIdentity {
                conversation_id: Some("conversation".to_string()),
                run_id: Some("run".to_string()),
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
            content,
        )?
        .ok_or_else(|| echo_core::error::ReactError::Other("artifact was not created".to_string()))
    }

    #[tokio::test]
    async fn reads_large_single_line_artifact_to_completion() -> echo_core::error::Result<()> {
        let root = test_root("single-line");
        let content = format!(r#"{{"payload":"{}"}}"#, "数据🙂".repeat(110_000));
        assert!(content.len() >= 1024 * 1024);
        let artifact = create_artifact(&root, &content)?;
        let config = ToolOutputArtifactConfig::new(&root, "test").threshold_bytes(1);
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(config),
            ..Default::default()
        };
        let tool = ReadArtifactTool;
        let mut cursor = None;
        let mut recovered = String::new();
        loop {
            let mut parameters = ToolParameters::from([
                (
                    "path".to_string(),
                    Value::String(artifact.path.display().to_string()),
                ),
                ("max_tokens".to_string(), Value::from(128_u64)),
            ]);
            if let Some(value) = cursor.clone() {
                parameters.insert("cursor".to_string(), Value::String(value));
            }
            let result = tool.execute_with_context(parameters, &ctx).await?;
            assert!(result.success, "{}", result.error.unwrap_or_default());
            assert!(HeuristicTokenizer.count_tokens(&result.output) <= 4_000);
            assert_eq!(
                result.metadata.get("sha256").map(String::as_str),
                Some(artifact.sha256.as_str())
            );
            let (page, _) = result.output.split_once("\n\n[Artifact ").ok_or_else(|| {
                echo_core::error::ReactError::Other("missing page notice".to_string())
            })?;
            recovered.push_str(page);
            cursor = result.metadata.get("next_cursor").cloned();
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(recovered, content);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_cursor_after_artifact_changes() -> echo_core::error::Result<()> {
        let root = test_root("changed");
        let artifact = create_artifact(&root, &"中文🙂".repeat(1_000))?;
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        let first = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([
                    (
                        "path".to_string(),
                        Value::String(artifact.path.display().to_string()),
                    ),
                    ("max_tokens".to_string(), Value::from(16_u64)),
                ]),
                &ctx,
            )
            .await?;
        let cursor = first.metadata.get("next_cursor").cloned().ok_or_else(|| {
            echo_core::error::ReactError::Other("first page had no cursor".to_string())
        })?;
        std::fs::write(&artifact.path, "changed")?;

        let second = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([
                    (
                        "path".to_string(),
                        Value::String(artifact.path.display().to_string()),
                    ),
                    ("cursor".to_string(), Value::String(cursor)),
                ]),
                &ctx,
            )
            .await?;
        assert!(!second.success);
        assert!(second.error.unwrap_or_default().contains("changed"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn reports_deleted_artifact() -> echo_core::error::Result<()> {
        let root = test_root("deleted");
        let artifact = create_artifact(&root, "temporary")?;
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        std::fs::remove_file(&artifact.path)?;
        let result = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([(
                    "path".to_string(),
                    Value::String(artifact.path.display().to_string()),
                )]),
                &ctx,
            )
            .await?;
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("cannot resolve"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape_from_artifact_root() -> echo_core::error::Result<()> {
        let root = test_root("symlink-root");
        let outside = test_root("symlink-outside");
        let artifact = create_artifact(&outside, "outside")?;
        std::fs::create_dir_all(&root)?;
        let link = root.join("artifact-link");
        std::os::unix::fs::symlink(&artifact.path, &link)?;
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        let result = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([(
                    "path".to_string(),
                    Value::String(link.display().to_string()),
                )]),
                &ctx,
            )
            .await?;
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("outside"));
        std::fs::remove_dir_all(root)?;
        std::fs::remove_dir_all(outside)?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_artifacts_outside_configured_root() -> echo_core::error::Result<()> {
        let root = test_root("root");
        let outside = test_root("outside");
        let artifact = create_artifact(&outside, "secret")?;
        std::fs::create_dir_all(&root)?;
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        let result = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([(
                    "path".to_string(),
                    Value::String(artifact.path.display().to_string()),
                )]),
                &ctx,
            )
            .await?;
        assert!(!result.success);
        std::fs::remove_dir_all(root)?;
        std::fs::remove_dir_all(outside)?;
        Ok(())
    }

    #[tokio::test]
    async fn verifies_first_page_hash() -> echo_core::error::Result<()> {
        let root = test_root("hash");
        let artifact = create_artifact(&root, "complete result")?;
        let ctx = echo_core::tools::ToolContext {
            output_artifacts: Some(ToolOutputArtifactConfig::new(&root, "test")),
            ..Default::default()
        };
        let result = ReadArtifactTool
            .execute_with_context(
                ToolParameters::from([
                    (
                        "path".to_string(),
                        Value::String(artifact.path.display().to_string()),
                    ),
                    (
                        "expected_sha256".to_string(),
                        Value::String("wrong".to_string()),
                    ),
                ]),
                &ctx,
            )
            .await?;
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("mismatch"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
