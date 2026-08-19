//! Local image viewing for multimodal agents.

use base64::Engine;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{
    Tool, ToolParameters, ToolResult, ToolResultContent, ToolResultKind, ToolRiskLevel,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::security::SecurityConfig;

/// Load a local image and project its pixels into the next model turn.
pub struct ViewImageTool {
    base_dir: Option<PathBuf>,
}

impl ViewImageTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for ViewImageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn required_input_modalities(&self) -> &'static [echo_core::llm::ModelInputModality] {
        &[echo_core::llm::ModelInputModality::Image]
    }

    fn description(&self) -> &str {
        "View a local image file when visual inspection is needed. The image pixels are sent directly to multimodal models."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local filesystem path to an image file"
                },
                "detail": {
                    "type": "string",
                    "enum": ["auto", "low", "high"],
                    "description": "Optional model image detail hint; defaults to high"
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
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let path_input = parameters
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?;
            let detail = parameters
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("high");
            if !matches!(detail, "auto" | "low" | "high") {
                return Ok(ToolResult::invalid_arguments(
                    "detail must be one of: auto, low, high",
                ));
            }

            let resolved = resolve_image_path(
                path_input,
                self.base_dir.as_deref(),
                ctx.working_dir.as_deref(),
            )?;
            let resolved_text = resolved.to_str().ok_or_else(|| ToolError::InvalidPath {
                path: resolved.display().to_string(),
                reason: "Image path is not valid UTF-8".to_string(),
            })?;
            let path = SecurityConfig::global().validate_file(resolved_text)?;
            if !path.is_file() {
                return Ok(ToolResult::invalid_arguments(format!(
                    "Image path is not a file: {}",
                    path.display()
                )));
            }

            let bytes =
                tokio::fs::read(&path)
                    .await
                    .map_err(|error| ToolError::ExecutionFailed {
                        tool: "view_image".to_string(),
                        message: format!("Failed to read image: {error}"),
                    })?;
            let Some(mime_type) = detect_supported_image_mime(&bytes) else {
                return Ok(ToolResult::invalid_arguments(
                    "Unsupported or invalid image. Supported formats: PNG, JPEG, GIF, WebP.",
                ));
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let data_url = format!("data:{mime_type};base64,{encoded}");

            Ok(ToolResult::success_with_kind(
                ToolResultKind::Image {
                    mime_type: mime_type.to_string(),
                },
                format!(
                    "Loaded image '{}' ({} bytes, {mime_type}).",
                    path.display(),
                    bytes.len()
                ),
            )
            .with_mime_type(mime_type)
            .with_meta("path", path.display().to_string())
            .with_meta("byte_size", bytes.len().to_string())
            .with_model_content(ToolResultContent::ImageUrl {
                url: data_url,
                detail: Some(detail.to_string()),
            }))
        })
    }
}

fn resolve_image_path(
    path: &str,
    base_dir: Option<&std::path::Path>,
    working_dir: Option<&std::path::Path>,
) -> Result<PathBuf> {
    let requested = std::path::Path::new(path);
    let root = match base_dir.or(working_dir) {
        Some(root) => root.to_path_buf(),
        None => std::env::current_dir().map_err(|error| ToolError::ExecutionFailed {
            tool: "view_image".to_string(),
            message: format!("Cannot resolve current directory: {error}"),
        })?,
    };
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let resolved = std::fs::canonicalize(&candidate).map_err(|error| ToolError::InvalidPath {
        path: candidate.display().to_string(),
        reason: format!("Image does not exist or cannot be accessed: {error}"),
    })?;
    if base_dir.is_some() {
        let canonical_root =
            std::fs::canonicalize(&root).map_err(|error| ToolError::ExecutionFailed {
                tool: "view_image".to_string(),
                message: format!("Cannot resolve image base directory: {error}"),
            })?;
        if !resolved.starts_with(&canonical_root) {
            return Err(ToolError::AccessDenied {
                path: resolved.display().to_string(),
                reason: "Image resolves outside the configured base directory".to_string(),
            }
            .into());
        }
    }
    Ok(resolved)
}

fn detect_supported_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF")
        && bytes
            .get(8..12)
            .is_some_and(|signature| signature == b"WEBP")
    {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tool_context(directory: &std::path::Path) -> echo_core::tools::ToolContext {
        echo_core::tools::ToolContext {
            working_dir: Some(directory.to_path_buf()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn view_image_projects_pixels_without_serializing_them() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("示例.png");
        let bytes = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        tokio::fs::write(&path, bytes).await?;
        let parameters = HashMap::from([
            ("path".to_string(), json!("示例.png")),
            ("detail".to_string(), json!("low")),
        ]);

        let result = ViewImageTool::new()
            .execute_with_context(parameters, &tool_context(directory.path()))
            .await?;

        assert!(result.success);
        assert!(matches!(
            result.kind,
            ToolResultKind::Image { ref mime_type } if mime_type == "image/png"
        ));
        let Some(ToolResultContent::ImageUrl { url, detail }) = result.model_content.first() else {
            return Err(std::io::Error::other("missing image model content").into());
        };
        assert!(url.starts_with("data:image/png;base64,"));
        assert_eq!(detail.as_deref(), Some("low"));

        let serialized = serde_json::to_string(&result)?;
        assert!(!serialized.contains("base64"));
        assert!(!serialized.contains("iVBOR"));
        Ok(())
    }

    #[tokio::test]
    async fn view_image_rejects_non_images() -> Result<()> {
        let directory = tempfile::tempdir()?;
        tokio::fs::write(directory.path().join("notes.txt"), "not an image").await?;
        let parameters = HashMap::from([("path".to_string(), json!("notes.txt"))]);

        let result = ViewImageTool::new()
            .execute_with_context(parameters, &tool_context(directory.path()))
            .await?;

        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Unsupported or invalid image"))
        );
        Ok(())
    }
}
