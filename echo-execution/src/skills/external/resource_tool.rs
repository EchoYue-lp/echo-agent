//! ReadSkillResourceTool -- on-demand resource loading (Tier 3).
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

use crate::skills::registry::SkillRegistry;
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

/// Default maximum resource file size (1 MB).
const DEFAULT_MAX_RESOURCE_BYTES: usize = 1024 * 1024;

/// Tool for reading resource files from activated skill directories.
///
/// Only allows reading from skills that have been activated via `activate_skill`.
/// Rejects path traversal attempts for security.
pub struct ReadSkillResourceTool {
    registry: Arc<RwLock<SkillRegistry>>,
    max_resource_bytes: usize,
}

impl ReadSkillResourceTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self {
            registry,
            max_resource_bytes: DEFAULT_MAX_RESOURCE_BYTES,
        }
    }

    /// Set the maximum allowed resource file size in bytes.
    pub fn with_max_bytes(mut self, bytes: usize) -> Self {
        self.max_resource_bytes = bytes;
        self
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

            // Path safety check using Path::components (robust against encoded traversal)
            let rel_path_obj = std::path::Path::new(&rel_path);
            if !is_path_traversal_safe(rel_path_obj) {
                return Ok(ToolResult::error(
                    "Path traversal ('..') is not allowed in resource paths",
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

            if !descriptor.permits_tool(self.name()) {
                return Ok(ToolResult::error(format!(
                    "Skill '{}' does not permit tool '{}'; allowed-tools: {}",
                    skill_name,
                    self.name(),
                    descriptor.allowed_tools.join(", ")
                )));
            }

            let skill_dir = match descriptor.location.parent() {
                Some(d) => d,
                None => {
                    return Ok(ToolResult::error(format!(
                        "Cannot determine skill directory for '{}'",
                        skill_name
                    )));
                }
            };

            // Canonicalize skill dir first (must exist)
            let canonical_skill =
                skill_dir
                    .canonicalize()
                    .map_err(|_| ToolError::ExecutionFailed {
                        tool: self.name().to_string(),
                        message: format!("Cannot resolve skill directory for '{}'", skill_name),
                    })?;

            // Check existence before canonicalizing the resource path
            let resource_path = skill_dir.join(&rel_path);
            if !resource_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Resource file not found: {} (in skill '{}')",
                    rel_path, skill_name
                )));
            }

            // Verify the resolved path is still under the skill directory
            let canonical_resource =
                resource_path
                    .canonicalize()
                    .map_err(|_| ToolError::ExecutionFailed {
                        tool: self.name().to_string(),
                        message: "Cannot resolve resource path".to_string(),
                    })?;
            if !canonical_resource.starts_with(&canonical_skill) {
                return Ok(ToolResult::error(
                    "Resolved path escapes the skill directory",
                ));
            }

            let metadata = match tokio::fs::metadata(&resource_path).await {
                Ok(m) => m,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Cannot stat resource '{}': {}",
                        rel_path, e
                    )));
                }
            };

            if metadata.len() > self.max_resource_bytes as u64 {
                return Ok(ToolResult::error(format!(
                    "Resource file '{}' exceeds maximum size ({} bytes, limit: {} bytes)",
                    rel_path,
                    metadata.len(),
                    self.max_resource_bytes
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

/// Check whether a path contains parent-dir traversal components.
///
/// Uses `Path::components` instead of string matching to handle
/// encoded or unusual traversal attempts.
fn is_path_traversal_safe(path: &std::path::Path) -> bool {
    use std::path::Component;
    path.components()
        .all(|c| !matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::external::types::SkillDescriptor;
    use crate::skills::registry::SkillRegistry;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn read_skill_resource_remains_available_to_restricted_skill() {
        let root =
            std::env::temp_dir().join(format!("echo-skill-resource-test-{}", std::process::id()));
        let skill_dir = root.join("locked-skill");
        tokio::fs::create_dir_all(skill_dir.join("references"))
            .await
            .unwrap();
        tokio::fs::write(skill_dir.join("SKILL.md"), "body")
            .await
            .unwrap();
        tokio::fs::write(skill_dir.join("references/guide.md"), "hello")
            .await
            .unwrap();

        let mut registry = SkillRegistry::new();
        registry.register_descriptor(SkillDescriptor {
            source: None,
            name: "locked-skill".into(),
            description: "desc".into(),
            location: skill_dir.join("SKILL.md"),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec!["run_skill_script".into()],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        });
        registry.mark_activated("locked-skill");

        let tool = ReadSkillResourceTool::new(Arc::new(RwLock::new(registry)));
        let result = tool
            .execute(
                [
                    ("skill_name".to_string(), json!("locked-skill")),
                    ("path".to_string(), json!("references/guide.md")),
                ]
                .into(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("hello"));

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
