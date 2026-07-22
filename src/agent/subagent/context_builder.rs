//! Context builder for scoped subagent contexts
//!
//! This module provides the `ContextBuilder` for building scoped contexts
//! that don't inherit the full parent context.

use crate::agent::subagent::context::{MemoryScope, OutputSchema, SubagentContext};
use crate::tasks::Evidence;
use echo_core::llm::ToolDefinition;
use echo_core::llm::types::Message;
use std::sync::Arc;

use crate::memory::store::Store;

/// Builder for creating scoped subagent contexts
pub struct ContextBuilder {
    /// Parent's tool definitions
    parent_tools: Vec<ToolDefinition>,
    /// Parent's conversation messages
    parent_messages: Vec<Message>,
    /// Parent's memory store
    parent_store: Option<Arc<dyn Store>>,

    // ── Scoped Context Fields ──────────────────────────────────────
    /// Parent's overall goal
    parent_goal: Option<String>,
    /// Task assigned to this subagent
    assigned_task: Option<String>,
    /// Relevant files for this task
    relevant_files: Vec<String>,
    /// Relevant artifacts from parent execution
    relevant_artifacts: Vec<String>,
    /// Constraints for this subagent
    constraints: Vec<String>,
    /// Allowed tools for this subagent
    allowed_tools: Option<Vec<String>>,
    /// Memory scope for this subagent
    memory_scope: MemoryScope,
    /// Output schema specification
    output_schema: OutputSchema,
}

impl ContextBuilder {
    /// Create a new context builder
    pub fn new() -> Self {
        Self {
            parent_tools: Vec::new(),
            parent_messages: Vec::new(),
            parent_store: None,
            parent_goal: None,
            assigned_task: None,
            relevant_files: Vec::new(),
            relevant_artifacts: Vec::new(),
            constraints: Vec::new(),
            allowed_tools: None,
            memory_scope: MemoryScope::default(),
            output_schema: OutputSchema::default(),
        }
    }

    /// Set parent's tool definitions
    pub fn with_parent_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.parent_tools = tools;
        self
    }

    /// Set parent's conversation messages
    pub fn with_parent_messages(mut self, messages: Vec<Message>) -> Self {
        self.parent_messages = messages;
        self
    }

    /// Set parent's memory store
    pub fn with_parent_store(mut self, store: Arc<dyn Store>) -> Self {
        self.parent_store = Some(store);
        self
    }

    /// Set parent's overall goal
    pub fn with_parent_goal(mut self, goal: impl Into<String>) -> Self {
        self.parent_goal = Some(goal.into());
        self
    }

    /// Set task assigned to this subagent
    pub fn with_assigned_task(mut self, task: impl Into<String>) -> Self {
        self.assigned_task = Some(task.into());
        self
    }

    /// Add relevant files for this task
    pub fn with_relevant_files(mut self, files: Vec<String>) -> Self {
        self.relevant_files = files;
        self
    }

    /// Add a single relevant file
    pub fn add_relevant_file(mut self, file: impl Into<String>) -> Self {
        self.relevant_files.push(file.into());
        self
    }

    /// Add relevant artifacts from parent execution
    pub fn with_relevant_artifacts(mut self, artifacts: Vec<String>) -> Self {
        self.relevant_artifacts = artifacts;
        self
    }

    /// Add a single relevant artifact
    pub fn add_relevant_artifact(mut self, artifact: impl Into<String>) -> Self {
        self.relevant_artifacts.push(artifact.into());
        self
    }

    /// Add constraints for this subagent
    pub fn with_constraints(mut self, constraints: Vec<String>) -> Self {
        self.constraints = constraints;
        self
    }

    /// Add a single constraint
    pub fn add_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.constraints.push(constraint.into());
        self
    }

    /// Set allowed tools for this subagent (overrides inheritance)
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Set memory scope for this subagent
    pub fn with_memory_scope(mut self, scope: MemoryScope) -> Self {
        self.memory_scope = scope;
        self
    }

    /// Set output schema specification
    pub fn with_output_schema(mut self, schema: OutputSchema) -> Self {
        self.output_schema = schema;
        self
    }

    /// Build the scoped context
    pub fn build_scoped_context(self) -> SubagentContext {
        // Apply allowed_tools filter to parent tools
        let filtered_tools = if let Some(allowed) = &self.allowed_tools {
            self.parent_tools
                .into_iter()
                .filter(|t| allowed.iter().any(|a| a == &t.function.name))
                .collect()
        } else {
            self.parent_tools
        };

        // Apply memory scope
        let store = match self.memory_scope {
            MemoryScope::None => None,
            MemoryScope::Relevant => {
                // For now, treat Relevant same as None (would need similarity search)
                // Future: implement similarity-based memory filtering
                None
            }
            MemoryScope::Full => self.parent_store,
        };

        SubagentContext {
            tool_definitions: filtered_tools,
            messages: self.parent_messages,
            store,
            parent_goal: self.parent_goal,
            assigned_task: self.assigned_task,
            relevant_files: self.relevant_files,
            relevant_artifacts: self.relevant_artifacts,
            constraints: self.constraints,
            allowed_tools: self.allowed_tools,
            memory_scope: self.memory_scope,
            output_schema: self.output_schema,
        }
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── SubagentOutput ──────────────────────────────────────────────────────────

/// Structured output from a subagent
#[derive(Debug, Clone)]
pub struct SubagentOutput {
    /// Summary of what the subagent did
    pub summary: String,
    /// Key findings from the subagent
    pub findings: Vec<String>,
    /// Evidence collected during execution
    pub evidence: Vec<Evidence>,
    /// Files read during execution
    pub files_read: Vec<String>,
    /// Recommendations from the subagent
    pub recommendations: Vec<String>,
    /// Blockers encountered
    pub blockers: Vec<String>,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

impl SubagentOutput {
    /// Create a new empty output
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            findings: Vec::new(),
            evidence: Vec::new(),
            files_read: Vec::new(),
            recommendations: Vec::new(),
            blockers: Vec::new(),
            confidence: 1.0,
        }
    }

    /// Add a finding
    pub fn add_finding(mut self, finding: impl Into<String>) -> Self {
        self.findings.push(finding.into());
        self
    }

    /// Add evidence
    pub fn add_evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }

    /// Add a file read
    pub fn add_file_read(mut self, file: impl Into<String>) -> Self {
        self.files_read.push(file.into());
        self
    }

    /// Add a recommendation
    pub fn add_recommendation(mut self, recommendation: impl Into<String>) -> Self {
        self.recommendations.push(recommendation.into());
        self
    }

    /// Add a blocker
    pub fn add_blocker(mut self, blocker: impl Into<String>) -> Self {
        self.blockers.push(blocker.into());
        self
    }

    /// Set confidence score
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Convert to JSON based on output schema
    pub fn to_json(&self, schema: &OutputSchema) -> serde_json::Value {
        let mut map = serde_json::Map::new();

        if schema.summary {
            map.insert(
                "summary".to_string(),
                serde_json::Value::String(self.summary.clone()),
            );
        }

        if schema.findings && !self.findings.is_empty() {
            map.insert(
                "findings".to_string(),
                serde_json::Value::Array(
                    self.findings
                        .iter()
                        .map(|f| serde_json::Value::String(f.clone()))
                        .collect(),
                ),
            );
        }

        if schema.evidence && !self.evidence.is_empty() {
            map.insert(
                "evidence".to_string(),
                serde_json::to_value(&self.evidence).unwrap_or_default(),
            );
        }

        if schema.files_read && !self.files_read.is_empty() {
            map.insert(
                "files_read".to_string(),
                serde_json::Value::Array(
                    self.files_read
                        .iter()
                        .map(|f| serde_json::Value::String(f.clone()))
                        .collect(),
                ),
            );
        }

        if schema.recommendations && !self.recommendations.is_empty() {
            map.insert(
                "recommendations".to_string(),
                serde_json::Value::Array(
                    self.recommendations
                        .iter()
                        .map(|r| serde_json::Value::String(r.clone()))
                        .collect(),
                ),
            );
        }

        if schema.blockers && !self.blockers.is_empty() {
            map.insert(
                "blockers".to_string(),
                serde_json::Value::Array(
                    self.blockers
                        .iter()
                        .map(|b| serde_json::Value::String(b.clone()))
                        .collect(),
                ),
            );
        }

        if schema.confidence {
            map.insert(
                "confidence".to_string(),
                serde_json::Value::Number(
                    // confidence is clamped to [0.0, 1.0]; from_f64 only fails on
                    // NaN/Inf, which are unreachable here. Fall back to 0.
                    serde_json::Number::from_f64(self.confidence as f64)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
            );
        }

        serde_json::Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_builder_default() {
        let ctx = ContextBuilder::new().build_scoped_context();
        assert!(ctx.tool_definitions.is_empty());
        assert!(ctx.messages.is_empty());
        assert!(ctx.store.is_none());
        assert!(ctx.parent_goal.is_none());
        assert!(ctx.assigned_task.is_none());
    }

    #[test]
    fn test_context_builder_with_fields() {
        let ctx = ContextBuilder::new()
            .with_parent_goal("Build a web app")
            .with_assigned_task("Implement login feature")
            .with_relevant_files(vec!["src/main.rs".to_string()])
            .with_constraints(vec!["Max 100 lines".to_string()])
            .build_scoped_context();

        assert_eq!(ctx.parent_goal, Some("Build a web app".to_string()));
        assert_eq!(
            ctx.assigned_task,
            Some("Implement login feature".to_string())
        );
        assert_eq!(ctx.relevant_files, vec!["src/main.rs"]);
        assert_eq!(ctx.constraints, vec!["Max 100 lines"]);
    }

    #[test]
    fn test_context_builder_filters_tools() {
        let tools = vec![
            ToolDefinition {
                tool_type: "function".to_string(),
                function: echo_core::llm::types::FunctionSpec {
                    name: "search".into(),
                    description: "Search".into(),
                    parameters: serde_json::json!({}),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: echo_core::llm::types::FunctionSpec {
                    name: "read".into(),
                    description: "Read".into(),
                    parameters: serde_json::json!({}),
                },
            },
        ];

        let ctx = ContextBuilder::new()
            .with_parent_tools(tools)
            .with_allowed_tools(vec!["search".to_string()])
            .build_scoped_context();

        assert_eq!(ctx.tool_definitions.len(), 1);
        assert_eq!(ctx.tool_definitions[0].function.name, "search");
    }

    #[test]
    fn test_subagent_output() {
        let output = SubagentOutput::new("Implemented login feature")
            .add_finding("Found auth module")
            .add_file_read("src/auth.rs")
            .add_recommendation("Add rate limiting")
            .with_confidence(0.95);

        assert_eq!(output.summary, "Implemented login feature");
        assert_eq!(output.findings.len(), 1);
        assert_eq!(output.files_read.len(), 1);
        assert_eq!(output.recommendations.len(), 1);
        assert_eq!(output.confidence, 0.95);
    }

    #[test]
    fn test_subagent_output_to_json() {
        let output = SubagentOutput::new("Test summary")
            .add_finding("Finding 1")
            .with_confidence(0.8);

        let schema = OutputSchema {
            summary: true,
            findings: true,
            evidence: false,
            files_read: false,
            recommendations: false,
            blockers: false,
            confidence: true,
        };

        let json = output.to_json(&schema);
        assert!(json.get("summary").is_some());
        assert!(json.get("findings").is_some());
        assert!(json.get("confidence").is_some());
        assert!(json.get("evidence").is_none());
    }
}
