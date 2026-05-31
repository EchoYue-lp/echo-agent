//! Prompt template management system
//!
//! Provides a centralized `PromptTemplateManager` for registering, storing, and
//! rendering prompt templates with variable substitution. Templates use
//! `{{variable_name}}` syntax and support nested variable resolution, default
//! values, and conditional sections.
//!
//! # Quick Start
//!
//! ```rust
//! use echo_core::agent::prompt_template::PromptTemplateManager;
//!
//! let manager = PromptTemplateManager::new();
//! manager.register("greeting", "Hello, {{name}}! Welcome to {{project}}.");
//!
//! let result = manager.render("greeting", &[("name", "Alice"), ("project", "EchoAgent")]);
//! assert_eq!(result.unwrap(), "Hello, Alice! Welcome to EchoAgent.");
//! ```
//!
//! # Template Syntax
//!
//! - **Variable**: `{{variable_name}}` — substituted with the provided value.
//! - **Default value**: `{{variable_name:default_value}}` — uses `default_value`
//!   when the variable is not provided.
//! - **Conditional block**: `{{#if variable_name}}...{{#endif}}` — includes the
//!   block only when `variable_name` is present and non-empty.
//! - **Conditional with alternative**: `{{#if variable_name}}...{{#else}}...{{#endif}}`
//!   — includes the first block when the variable is present, otherwise the
//!   alternative block.

use crate::error::{ReactError, Result};
use std::collections::HashMap;
use std::sync::RwLock;

// ── Template Syntax Parsing ───────────────────────────────────────────────────

/// A parsed template element within a prompt template string.
#[derive(Debug, Clone, PartialEq)]
enum TemplateElement {
    /// Literal text segment (no substitution needed).
    Literal(String),
    /// Variable placeholder: `{{name}}` or `{{name:default}}`.
    Variable {
        name: String,
        default: Option<String>,
    },
    /// Conditional block start: `{{#if name}}`.
    ConditionalStart { name: String },
    /// Conditional else marker: `{{#else}}`.
    ConditionalElse,
    /// Conditional block end: `{{#endif}}`.
    ConditionalEnd,
}

/// Parse a template string into a sequence of `TemplateElement`s.
fn parse_template(template: &str) -> Vec<TemplateElement> {
    let mut elements = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        // Find the next `{{` marker
        let open_pos = remaining.find("{{");
        match open_pos {
            None => {
                // No more markers; the rest is literal text
                if !remaining.is_empty() {
                    elements.push(TemplateElement::Literal(remaining.to_string()));
                }
                break;
            }
            Some(pos) => {
                // Emit literal text before the marker
                if pos > 0 {
                    elements.push(TemplateElement::Literal(remaining[..pos].to_string()));
                }
                // Find the closing `}}`
                let after_open = &remaining[pos + 2..];
                let close_pos = after_open.find("}}");
                match close_pos {
                    None => {
                        // Unclosed marker — treat the rest as literal
                        elements.push(TemplateElement::Literal(remaining.to_string()));
                        break;
                    }
                    Some(cpos) => {
                        let tag_content = &after_open[..cpos];
                        let tag_content_trimmed = tag_content.trim();

                        if tag_content_trimmed.starts_with("#if ") {
                            // Conditional start: {{#if variable_name}}
                            let var_name = tag_content_trimmed[4..].trim();
                            elements.push(TemplateElement::ConditionalStart {
                                name: var_name.to_string(),
                            });
                        } else if tag_content_trimmed == "#else" {
                            elements.push(TemplateElement::ConditionalElse);
                        } else if tag_content_trimmed == "#endif" {
                            elements.push(TemplateElement::ConditionalEnd);
                        } else if !tag_content_trimmed.is_empty() {
                            // Variable: {{name}} or {{name:default}}
                            let colon_pos = tag_content_trimmed.find(':');
                            match colon_pos {
                                None => {
                                    elements.push(TemplateElement::Variable {
                                        name: tag_content_trimmed.to_string(),
                                        default: None,
                                    });
                                }
                                Some(cp) => {
                                    let name = tag_content_trimmed[..cp].trim();
                                    let default = tag_content_trimmed[cp + 1..].trim();
                                    elements.push(TemplateElement::Variable {
                                        name: name.to_string(),
                                        default: Some(default.to_string()),
                                    });
                                }
                            }
                        }
                        // else: empty {{}} tag, skip it

                        remaining = &after_open[cpos + 2..];
                    }
                }
            }
        }
    }

    elements
}

/// Render a sequence of parsed template elements using the provided variables.
fn render_elements(
    elements: &[TemplateElement],
    variables: &HashMap<String, String>,
) -> String {
    let mut result = String::new();
    let mut i = 0;

    while i < elements.len() {
        match &elements[i] {
            TemplateElement::Literal(text) => {
                result.push_str(text);
                i += 1;
            }
            TemplateElement::Variable { name, default } => {
                let value = variables
                    .get(name)
                    .map(|s| s.as_str())
                    .or_else(|| default.as_deref());
                result.push_str(value.unwrap_or(""));
                i += 1;
            }
            TemplateElement::ConditionalStart { name } => {
                // Find matching #else and #endif
                let mut else_index = None;
                let mut end_index = None;
                let mut depth = 0;
                for j in (i + 1)..elements.len() {
                    match &elements[j] {
                        TemplateElement::ConditionalStart { .. } => depth += 1,
                        TemplateElement::ConditionalEnd => {
                            if depth == 0 {
                                end_index = Some(j);
                                break;
                            }
                            depth -= 1;
                        }
                        TemplateElement::ConditionalElse if depth == 0 => {
                            else_index = Some(j);
                        }
                        _ => {}
                    }
                }

                let has_value = variables.contains_key(name) && !variables[name].is_empty();
                let end = end_index.unwrap_or(elements.len());

                if has_value {
                    // Render the block between #if and #else/#endif
                    let block_end = else_index.unwrap_or(end);
                    let block = &elements[i + 1..block_end];
                    result.push_str(&render_elements(block, variables));
                } else if let Some(else_idx) = else_index {
                    // Render the block between #else and #endif
                    let block = &elements[else_idx + 1..end];
                    result.push_str(&render_elements(block, variables));
                }

                i = end + 1;
            }
            TemplateElement::ConditionalElse | TemplateElement::ConditionalEnd => {
                // These should be consumed by ConditionalStart rendering above.
                // If we encounter them at the top level, just skip.
                i += 1;
            }
        }
    }

    result
}

// ── PromptTemplateManager ──────────────────────────────────────────────────────

/// Centralized prompt template registry and rendering engine.
///
/// `PromptTemplateManager` stores named prompt templates and renders them with
/// variable substitution. It supports:
///
/// - Simple variable substitution: `{{name}}`
/// - Default values: `{{name:default}}`
/// - Conditional blocks: `{{#if var}}...{{#else}}...{{#endif}}`
///
/// The manager is thread-safe (uses `RwLock` internally) and is typically
/// shared across agents via `Arc<PromptTemplateManager>`.
///
/// # Example
///
/// ```rust
/// use echo_core::agent::prompt_template::PromptTemplateManager;
/// use std::sync::Arc;
///
/// let manager = Arc::new(PromptTemplateManager::new());
/// manager.register("system_prompt", "You are a {{role}} assistant for {{domain}}.");
///
/// let rendered = manager.render("system_prompt", &[
///     ("role", "coding"),
///     ("domain", "Rust development"),
/// ]);
/// assert_eq!(rendered.unwrap(), "You are a coding assistant for Rust development.");
/// ```
pub struct PromptTemplateManager {
    templates: RwLock<HashMap<String, String>>,
}

impl PromptTemplateManager {
    /// Create an empty template manager.
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::new()),
        }
    }

    /// Create a manager pre-loaded with the default mode prompt templates.
    ///
    /// This registers templates named `"mode_general"`, `"mode_coding"`,
    /// `"mode_research"`, `"mode_data"`, and `"mode_writing"` with the
    /// default English-language system prompts from `DefaultModeEngine`.
    pub fn with_default_mode_templates() -> Self {
        use crate::agent::mode::{AgentMode, DefaultModeEngine, ModeEngine};

        let engine = DefaultModeEngine;
        let mut templates = HashMap::new();

        for mode in AgentMode::all() {
            let config = engine.mode_config(mode);
            // Use kebab-case key names (matching serde rename_all convention):
            // "mode_general", "mode_coding", "mode_research", "mode_data", "mode_writing"
            let key = match mode {
                AgentMode::General => "mode_general",
                AgentMode::Coding => "mode_coding",
                AgentMode::Research => "mode_research",
                AgentMode::Data => "mode_data",
                AgentMode::Writing => "mode_writing",
                // #[non_exhaustive] requires a wildcard arm even if all current
                // variants are handled. Future modes get a generic key.
                #[allow(unreachable_patterns)]
                _ => {
                    let name = mode.name().to_lowercase().replace(' ', "_");
                    &format!("mode_{name}")
                }
            };
            templates.insert(key.to_string(), config.system_prompt_template);
        }

        // Also register as the generic "mode" template with a mode variable
        templates.insert(
            "mode".into(),
            "{{#if mode_general}}{{mode_general}}{{#endif}}"
                .to_string(),
        );

        Self {
            templates: RwLock::new(templates),
        }
    }

    /// Register a named prompt template.
    ///
    /// If a template with the same name already exists, it is overwritten.
    pub fn register(&self, name: &str, template: &str) {
        // recover from poison — data is still valid
        let mut guard = self.templates.write().unwrap_or_else(|e| e.into_inner());
        guard.insert(name.to_string(), template.to_string());
    }

    /// Remove a named template.
    ///
    /// Returns `true` if the template existed and was removed.
    pub fn remove(&self, name: &str) -> bool {
        // recover from poison — data is still valid
        let mut guard = self.templates.write().unwrap_or_else(|e| e.into_inner());
        guard.remove(name).is_some()
    }

    /// Check whether a template with the given name is registered.
    pub fn contains(&self, name: &str) -> bool {
        // recover from poison — data is still valid
        let guard = self.templates.read().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(name)
    }

    /// List all registered template names.
    pub fn template_names(&self) -> Vec<String> {
        // recover from poison — data is still valid
        let guard = self.templates.read().unwrap_or_else(|e| e.into_inner());
        guard.keys().cloned().collect()
    }

    /// Render a named template with the provided variable substitutions.
    ///
    /// Variables are provided as a slice of `(name, value)` pairs. Missing
    /// variables are either replaced with their default value (if specified
    /// in the template via `{{name:default}}`) or left empty.
    ///
    /// # Errors
    ///
    /// Returns `ReactError::Other` if the template name is not registered.
    pub fn render(&self, name: &str, variables: &[(&str, &str)]) -> Result<String> {
        // recover from poison — data is still valid
        let guard = self.templates.read().unwrap_or_else(|e| e.into_inner());
        let template = guard
            .get(name)
            .ok_or_else(|| ReactError::Other(format!("Prompt template '{}' not found", name)))?;

        let vars: HashMap<String, String> = variables
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let elements = parse_template(template);
        Ok(render_elements(&elements, &vars))
    }

    /// Render a template string directly (without looking up by name).
    ///
    /// This is useful for one-off rendering where the template is not
    /// registered in the manager.
    pub fn render_template(&self, template: &str, variables: &[(&str, &str)]) -> String {
        let vars: HashMap<String, String> = variables
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let elements = parse_template(template);
        render_elements(&elements, &vars)
    }

    /// Get the raw template string for a registered template.
    ///
    /// Returns `None` if the template name is not registered.
    pub fn get_template(&self, name: &str) -> Option<String> {
        // recover from poison — data is still valid
        let guard = self.templates.read().unwrap_or_else(|e| e.into_inner());
        guard.get(name).cloned()
    }

    /// Render a named template, falling back to the raw template string
    /// if no substitution is needed (i.e., the template contains no `{{`
    /// markers).
    ///
    /// This is an optimization for templates that are static text.
    pub fn render_or_raw(&self, name: &str, variables: &[(&str, &str)]) -> Result<String> {
        // recover from poison — data is still valid
        let guard = self.templates.read().unwrap_or_else(|e| e.into_inner());
        let template = guard
            .get(name)
            .ok_or_else(|| ReactError::Other(format!("Prompt template '{}' not found", name)))?;

        if !template.contains("{{") {
            return Ok(template.clone());
        }

        let vars: HashMap<String, String> = variables
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let elements = parse_template(template);
        Ok(render_elements(&elements, &vars))
    }
}

impl Default for PromptTemplateManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_simple_variable_substitution() {
        let manager = PromptTemplateManager::new();
        manager.register("greeting", "Hello, {{name}}!");
        let result = manager.render("greeting", &[("name", "Alice")]).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_multiple_variables() {
        let manager = PromptTemplateManager::new();
        manager.register("intro", "I am {{name}}, a {{role}} at {{company}}.");
        let result = manager
            .render("intro", &[("name", "Bob"), ("role", "engineer"), ("company", "Acme")])
            .unwrap();
        assert_eq!(result, "I am Bob, a engineer at Acme.");
    }

    #[test]
    fn test_default_value() {
        let manager = PromptTemplateManager::new();
        manager.register("fallback", "Hello, {{name:Guest}}!");
        // Without providing 'name', defaults to "Guest"
        let result = manager.render("fallback", &[]).unwrap();
        assert_eq!(result, "Hello, Guest!");
        // With 'name' provided, uses the provided value
        let result = manager.render("fallback", &[("name", "Alice")]).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_missing_variable_empty() {
        let manager = PromptTemplateManager::new();
        manager.register("missing", "Hello, {{name}}!");
        let result = manager.render("missing", &[]).unwrap();
        assert_eq!(result, "Hello, !");
    }

    #[test]
    fn test_conditional_block_present() {
        let manager = PromptTemplateManager::new();
        manager.register(
            "cond",
            "Base text. {{#if detail}}Details: {{detail}}.{{#endif}} End.",
        );
        let result = manager
            .render("cond", &[("detail", "important info")])
            .unwrap();
        assert_eq!(result, "Base text. Details: important info. End.");
    }

    #[test]
    fn test_conditional_block_absent() {
        let manager = PromptTemplateManager::new();
        manager.register(
            "cond",
            "Base text. {{#if detail}}Details: {{detail}}.{{#endif}} End.",
        );
        let result = manager.render("cond", &[]).unwrap();
        assert_eq!(result, "Base text.  End.");
    }

    #[test]
    fn test_conditional_with_else() {
        let manager = PromptTemplateManager::new();
        manager.register(
            "cond_else",
            "{{#if premium}}Premium features enabled.{{#else}}Standard features only.{{#endif}}",
        );
        // Premium present
        let result = manager
            .render("cond_else", &[("premium", "true")])
            .unwrap();
        assert_eq!(result, "Premium features enabled.");
        // Premium absent
        let result = manager.render("cond_else", &[]).unwrap();
        assert_eq!(result, "Standard features only.");
    }

    #[test]
    fn test_conditional_empty_value_acts_as_absent() {
        let manager = PromptTemplateManager::new();
        manager.register(
            "cond_empty",
            "{{#if var}}Present{{#else}}Absent{{#endif}}",
        );
        // Empty string value triggers the else branch
        let result = manager.render("cond_empty", &[("var", "")]).unwrap();
        assert_eq!(result, "Absent");
    }

    #[test]
    fn test_nested_conditional() {
        let manager = PromptTemplateManager::new();
        manager.register(
            "nested",
            "{{#if a}}A is present. {{#if b}}B too.{{#else}}B missing.{{#endif}}{{#else}}A missing.{{#endif}}",
        );
        // Both present
        let result = manager
            .render("nested", &[("a", "yes"), ("b", "yes")])
            .unwrap();
        assert_eq!(result, "A is present. B too.");
        // Only A present
        let result = manager.render("nested", &[("a", "yes")]).unwrap();
        assert_eq!(result, "A is present. B missing.");
        // A absent
        let result = manager.render("nested", &[]).unwrap();
        assert_eq!(result, "A missing.");
    }

    #[test]
    fn test_template_not_found() {
        let manager = PromptTemplateManager::new();
        let result = manager.render("nonexistent", &[]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not found"));
    }

    #[test]
    fn test_register_overwrite() {
        let manager = PromptTemplateManager::new();
        manager.register("key", "Version 1");
        manager.register("key", "Version 2");
        let result = manager.render("key", &[]).unwrap();
        assert_eq!(result, "Version 2");
    }

    #[test]
    fn test_remove_template() {
        let manager = PromptTemplateManager::new();
        manager.register("key", "Value");
        assert!(manager.remove("key"));
        assert!(!manager.contains("key"));
        assert!(!manager.remove("key")); // Already removed
    }

    #[test]
    fn test_template_names() {
        let manager = PromptTemplateManager::new();
        manager.register("a", "A");
        manager.register("b", "B");
        manager.register("c", "C");
        let names = manager.template_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
        assert!(names.contains(&"c".to_string()));
    }

    #[test]
    fn test_render_template_direct() {
        let manager = PromptTemplateManager::new();
        let result = manager.render_template("Hello {{who}}!", &[("who", "World")]);
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_render_or_raw_static_template() {
        let manager = PromptTemplateManager::new();
        manager.register("static", "No variables here.");
        let result = manager.render_or_raw("static", &[]).unwrap();
        assert_eq!(result, "No variables here.");
    }

    #[test]
    fn test_render_or_raw_dynamic_template() {
        let manager = PromptTemplateManager::new();
        manager.register("dynamic", "Hello, {{name}}!");
        let result = manager
            .render_or_raw("dynamic", &[("name", "Alice")])
            .unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_arc_shared_manager() {
        let manager = Arc::new(PromptTemplateManager::new());
        manager.register("shared", "Hello, {{name}}!");

        // Can be used from multiple references
        let m1 = Arc::clone(&manager);
        let m2 = Arc::clone(&manager);
        let r1 = m1.render("shared", &[("name", "A")]).unwrap();
        let r2 = m2.render("shared", &[("name", "B")]).unwrap();
        assert_eq!(r1, "Hello, A!");
        assert_eq!(r2, "Hello, B!");
    }

    #[test]
    fn test_get_template() {
        let manager = PromptTemplateManager::new();
        manager.register("key", "Hello {{name}}");
        assert_eq!(manager.get_template("key"), Some("Hello {{name}}".to_string()));
        assert_eq!(manager.get_template("missing"), None);
    }

    #[test]
    fn test_default_mode_templates() {
        let manager = PromptTemplateManager::with_default_mode_templates();
        assert!(manager.contains("mode_general"));
        assert!(manager.contains("mode_coding"));
        assert!(manager.contains("mode_research"));
        assert!(manager.contains("mode_data"));
        assert!(manager.contains("mode_writing"));
    }

    #[test]
    fn test_whitespace_in_tags() {
        let manager = PromptTemplateManager::new();
        manager.register("ws", "Hello, {{ name }}!");
        let result = manager.render("ws", &[("name", "Alice")]).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_literal_with_no_markers() {
        let manager = PromptTemplateManager::new();
        manager.register("plain", "Just plain text.");
        let result = manager.render("plain", &[]).unwrap();
        assert_eq!(result, "Just plain text.");
    }
}