//! ReadSkillResourceTool — on-demand resource loading (Tier 3).
//!
//! When a skill's instructions reference a bundled file (script, reference doc,
//! asset), the LLM calls this tool to load its content into context.
//!
//! Security: rejects path traversal (`..`) and only allows reading from
//! activated skills' directories.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::RwLock;

use crate::error::{Result, ToolError};
use crate::skills::registry::SkillRegistry;
use crate::tools::{Tool, ToolParameters, ToolResult};

/// Tool for reading resource files from activated skill directories.
///
/// Only allows reading from skills that have been activated via `activate_skill`.
/// Rejects path traversal attempts for security.
pub struct ReadSkillResourceTool {
    registry: Arc<RwLock<SkillRegistry>>,
}

impl ReadSkillResourceTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self { registry }
    }
}

impl Tool for ReadSkillResourceTool {
    fn name(&self) -> &str {
        "read_skill_resource"
    }

    fn description(&self) -> &str {
        "Read a resource file from an activated skill's directory. \
         Use this when a skill's instructions reference a file \
         (e.g., scripts/extract.py, references/guide.md). \
         The skill must be activated first via activate_skill."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the activated skill"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path to the resource file within the skill directory \
                                    (e.g., 'references/guide.md', 'scripts/run.py', 'checklist.md')"
                }
            },
            "required": ["skill_name", "path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let skill_name = parameters
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("skill_name".to_string()))?
                .to_string();

            let rel_path = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("path".to_string()))?
                .to_string();

            // Path safety check
            if rel_path.contains("..") {
                return Ok(ToolResult::error(
                    "Path traversal ('..') is not allowed in resource paths".into(),
                ));
            }

            let registry = self.registry.read().await;

            if !registry.is_activated(&skill_name) {
                return Ok(ToolResult::error(format!(
                    "Skill '{}' has not been activated. Call activate_skill first.",
                    skill_name
                )));
            }

            let descriptor = match registry.get_descriptor(&skill_name) {
                Some(d) => d,
                None => {
                    return Ok(ToolResult::error(format!(
                        "Skill '{}' not found in catalog",
                        skill_name
                    )));
                }
            };

            let skill_dir = match descriptor.location.parent() {
                Some(d) => d,
                None => {
                    return Ok(ToolResult::error(format!(
                        "Cannot determine skill directory for '{}'",
                        skill_name
                    )));
                }
            };

            let resource_path = skill_dir.join(&rel_path);

            // Verify the resolved path is still under the skill directory
            if let (Ok(canonical_skill), Ok(canonical_resource)) =
                (skill_dir.canonicalize(), resource_path.canonicalize())
                && !canonical_resource.starts_with(&canonical_skill)
            {
                return Ok(ToolResult::error(
                    "Resolved path escapes the skill directory".into(),
                ));
            }

            if !resource_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Resource file not found: {} (in skill '{}')",
                    rel_path, skill_name
                )));
            }

            match tokio::fs::read_to_string(&resource_path).await {
                Ok(content) => {
                    let header = format!(
                        "<skill_resource skill=\"{}\" path=\"{}\">\n",
                        skill_name, rel_path
                    );
                    let footer = "\n</skill_resource>";
                    Ok(ToolResult::success(format!(
                        "{}{}{}",
                        header, content, footer
                    )))
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to read '{}' in skill '{}': {}",
                    rel_path, skill_name, e
                ))),
            }
        })
    }
}

// Backward compatibility alias
#[deprecated(note = "Use ReadSkillResourceTool instead")]
pub type LoadSkillResourceTool = ReadSkillResourceTool;
