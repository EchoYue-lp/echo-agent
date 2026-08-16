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
use echo_core::tools::{Tool, ToolContext, ToolParameters, ToolResult, ToolResultKind};

fn activate_allowed_tools(ctx: &ToolContext, allowed_tools: &[String]) -> Vec<String> {
    let Some(visibility) = ctx.tool_visibility.as_ref() else {
        return Vec::new();
    };
    let matches = visibility
        .available_names()
        .into_iter()
        .filter(|tool_name| {
            allowed_tools
                .iter()
                .any(|matcher| super::types::tool_matcher(matcher, tool_name))
        })
        .collect::<Vec<_>>();
    visibility.extend_eligibility_and_activate(matches)
}

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

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
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

            let registry = self.registry.write().await;

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

            let allowed_tools = registry
                .get_descriptor(&name)
                .map(|descriptor| descriptor.allowed_tools.clone())
                .unwrap_or_default();

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
                    let activated_tools = activate_allowed_tools(ctx, &allowed_tools);
                    let mut result = ToolResult::success_with_kind(
                        ToolResultKind::SkillActivation { name: name.clone() },
                        block,
                    );
                    result
                        .metadata
                        .insert("activated_tools".to_string(), activated_tools.join(","));
                    Ok(result)
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to activate skill '{}': {}",
                    name, e
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::external::SkillLoader;

    #[tokio::test]
    async fn activation_promotes_matching_tool_schemas() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-skill-activation-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let skill_dir = root.join("git-skill");
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: git-skill\ndescription: Inspect git state\nallowed-tools: [git_*]\n---\nUse git tools.\n",
        )?;
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir(&root).await?;
        let mut registry = SkillRegistry::new();
        for descriptor in descriptors {
            registry.register_descriptor(descriptor);
        }
        let registry = Arc::new(RwLock::new(registry));
        let tool = ActivateSkillTool::new(registry, vec!["git-skill".to_string()]);
        let visibility = Arc::new(echo_core::tools::ToolVisibilityState::with_available(
            ["activate_skill", "git_status", "shell"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ["activate_skill"].into_iter().map(str::to_string).collect(),
            ["activate_skill"].into_iter().map(str::to_string).collect(),
        ));
        let ctx = ToolContext {
            tool_visibility: Some(Arc::clone(&visibility)),
            ..Default::default()
        };

        let result = tool
            .execute_with_context(
                ToolParameters::from([(
                    "name".to_string(),
                    serde_json::Value::String("git-skill".to_string()),
                )]),
                &ctx,
            )
            .await?;

        assert!(result.success);
        assert!(visibility.is_visible("git_status"));
        assert!(!visibility.is_visible("shell"));
        assert_eq!(
            result.metadata.get("activated_tools").map(String::as_str),
            Some("git_status")
        );
        assert_eq!(
            result.kind,
            ToolResultKind::SkillActivation {
                name: "git-skill".to_string()
            }
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
