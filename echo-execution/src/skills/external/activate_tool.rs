//! ActivateSkillTool -- model-driven skill activation (Tier 2).
//!
//! When the LLM determines a task matches a skill's description, it calls this tool
//! to load the skill's full instructions into the conversation context.
//! The tool returns a structured XML-tagged block containing:
//!
//! 1. The skill's Markdown instructions (SKILL.md body)
//! 2. A listing of bundled resource files (scripts, references, assets)
//! 3. The skill directory path for resolving relative references

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::RwLock;

use crate::skills::registry::SkillRegistry;
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

/// Tool for model-driven skill activation.
///
/// Registered automatically when file-based skills are discovered.
/// The LLM calls this when a task matches a skill's catalog description.
pub struct ActivateSkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
    /// Cached list of available skill names for the parameter description.
    available_names: Vec<String>,
}

impl ActivateSkillTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, available_names: Vec<String>) -> Self {
        Self {
            registry,
            available_names,
        }
    }
}

impl Tool for ActivateSkillTool {
    fn name(&self) -> &str {
        "activate_skill"
    }

    fn description(&self) -> &str {
        "Activate a skill to load its full instructions and available resources. \
         Call this when a task matches one of the available skills listed in the system prompt. \
         For skills with `paths` constraints, also provide `context_path` for the touched file."
    }

    fn parameters(&self) -> serde_json::Value {
        let names_desc = if self.available_names.is_empty() {
            "(no skills available)".to_string()
        } else {
            format!("One of: {}", self.available_names.join(", "))
        };

        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": format!("The skill name to activate. {}", names_desc)
                },
                "arguments": {
                    "type": "string",
                    "description": "Optional arguments to pass to the skill (space-separated). \
                                    Available inside skill content as ${ARGUMENTS}, ${1}, ${2}, etc."
                },
                "context_path": {
                    "type": "string",
                    "description": "Optional touched file path for conditional activation. \
                                    Required when the target skill declares `paths` constraints."
                }
            },
            "required": ["name"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let name = parameters
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("name".to_string()))?
                .to_string();

            let args: Vec<String> = parameters
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();

            let context_path = parameters
                .get("context_path")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let mut registry = self.registry.write().await;

            if let Some(descriptor) = registry.get_descriptor(&name)
                && !descriptor.paths.is_empty()
            {
                let Some(path) = context_path.as_deref() else {
                    return Ok(ToolResult::error(format!(
                        "Skill '{}' requires a matching context_path because it declares activation paths: {}",
                        name,
                        descriptor.paths.join(", ")
                    )));
                };

                if !descriptor.matches_context_path(path) {
                    return Ok(ToolResult::error(format!(
                        "Skill '{}' cannot be activated for context_path '{}'; expected one of: {}",
                        name,
                        path,
                        descriptor.paths.join(", ")
                    )));
                }
            }

            if registry.is_activated(&name) {
                return Ok(ToolResult::success(format!(
                    "Skill '{}' is already activated in this session. \
                     Its instructions are already in context.",
                    name
                )));
            }

            match registry
                .activate_with_args(
                    &name,
                    &args,
                    crate::skills::external::prompt_exec::SkillSource::Local,
                )
                .await
            {
                Ok(content) => {
                    let block = content.to_prompt_block();
                    Ok(ToolResult::success(block))
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to activate skill '{}': {}",
                    name, e
                ))),
            }
        })
    }
}
