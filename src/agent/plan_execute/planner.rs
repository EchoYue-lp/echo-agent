//! Planner — responsible for decomposing user tasks into structured execution plans

use super::types::Plan;
use super::types::{PlanOutput, PlanStep, PlanStepOutput, plan_output_schema};
use crate::error::Result;
use crate::llm::types::Message;
use crate::llm::{self, LlmConfig, ResponseFormat};
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Planner trait — accepts a task description and returns an execution plan
pub trait Planner: Send + Sync {
    /// Generate an execution plan based on the task description
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// ── LlmPlanner ───────────────────────────────────────────────────────────────

/// Planner output mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlannerOutputMode {
    /// Use JSON Schema to enforce structured output (recommended, highest reliability)
    #[default]
    JsonSchema,
    /// Use JsonObject + free-text parsing (better compatibility)
    JsonText,
}

/// LLM-based Planner: calls a large language model to decompose tasks
pub struct LlmPlanner {
    model: String,
    client: Arc<Client>,
    llm_config: Option<LlmConfig>,
    system_prompt: String,
    output_mode: PlannerOutputMode,
}

impl LlmPlanner {
    /// Create an LLM-based planner
    ///
    /// # Parameters
    /// * `model` - LLM model identifier for planning tasks
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            client: Arc::new(
                Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .unwrap_or_default(),
            ),
            llm_config: None,
            system_prompt: Self::default_system_prompt().to_string(),
            output_mode: PlannerOutputMode::JsonSchema,
        }
    }

    /// Use custom LLM configuration
    pub fn with_llm_config(mut self, config: LlmConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    /// Custom system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set output mode
    pub fn with_output_mode(mut self, mode: PlannerOutputMode) -> Self {
        self.output_mode = mode;
        self
    }

    fn default_system_prompt() -> &'static str {
        "You are a task planning expert. Given a task, you need to decompose it into concrete, executable steps.\n\n\
        Rules:\n\
        1. Each step must be explicit and executable\n\
        2. Steps must have a logical order\n\
        3. Each step should do only one thing\n\
        4. Step descriptions should be concise but sufficient\n\
        5. If a step depends on the result of another step, fill in dependency description keywords in the dependencies field\n\
        6. Mutually independent steps should not have dependency relationships\n\n\
        Please strictly return structured data according to the JSON Schema."
    }

    /// Attempt to parse LLM raw response as PlanOutput
    fn parse_structured_output(content: &str) -> Result<PlanOutput> {
        // 1. Direct parse
        if let Ok(output) = serde_json::from_str::<PlanOutput>(content) {
            return Ok(output);
        }

        // 2. Extract from markdown code block
        let json_str = crate::utils::json_parse::extract_json_from_markdown(content);
        if let Ok(output) = serde_json::from_str::<PlanOutput>(&json_str) {
            return Ok(output);
        }

        // 3. Try to fix common issues
        Self::try_auto_fix(&json_str)
    }

    /// Try to auto-fix common JSON issues
    fn try_auto_fix(json_str: &str) -> Result<PlanOutput> {
        let trimmed = json_str.trim();

        // Fix: bare array → wrap as {"steps": [...]}
        if trimmed.starts_with('[') {
            let wrapped = format!("{{\"steps\": {}}}", trimmed);
            let fixed = crate::utils::json_parse::clean_json(&wrapped);
            if let Ok(output) = serde_json::from_str::<PlanOutput>(&fixed) {
                info!("Auto-fix: wrapped bare array into PlanOutput");
                return Ok(output);
            }
        }

        // Fix: trailing commas, quotes, etc.
        let fixed = crate::utils::json_parse::clean_json(trimmed);

        match serde_json::from_str::<PlanOutput>(&fixed) {
            Ok(output) => {
                info!("Auto-fix succeeded for malformed LLM plan output");
                Ok(output)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse plan output even after auto-fix");
                // Last resort fallback: construct single-step plan
                Ok(PlanOutput {
                    steps: vec![PlanStepOutput {
                        description: trimmed.to_string(),
                        dependencies: vec![],
                        expected_output: None,
                    }],
                })
            }
        }
    }

    /// Parse legacy format (compatible with JsonText mode)
    fn parse_steps_legacy(response: &str) -> Vec<PlanStep> {
        let json_str = crate::utils::json_parse::extract_json_from_markdown(response);

        #[derive(serde::Deserialize)]
        struct StepJson {
            description: String,
        }

        if let Ok(steps) = serde_json::from_str::<Vec<StepJson>>(&json_str) {
            return steps
                .into_iter()
                .map(|s| PlanStep::new(s.description))
                .collect();
        }

        // Fallback: split by lines
        response
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .filter(|line| !line.starts_with("```") && !line.starts_with('#'))
            .map(|line| {
                let cleaned = line.trim_start_matches(|c: char| {
                    c.is_ascii_digit() || c == '.' || c == '-' || c == ' '
                });
                PlanStep::new(if cleaned.is_empty() { line } else { cleaned })
            })
            .collect()
    }

    /// Convert PlanOutput to Plan, resolving dependency relationships
    fn resolve_plan_output(output: PlanOutput, goal: &str) -> Plan {
        // Build description→index mapping (for fuzzy matching dependencies)
        let desc_to_idx: Vec<(String, usize)> = output
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.description.clone(), i))
            .collect();

        let steps: Vec<PlanStep> = output
            .steps
            .into_iter()
            .map(|step_output| {
                // Resolve dependencies: attempt fuzzy matching to step indices
                let deps: Vec<String> = step_output
                    .dependencies
                    .iter()
                    .filter_map(|dep_desc| {
                        // Exact match first
                        let exact = desc_to_idx.iter().find(|(d, _)| d == dep_desc);
                        if let Some((_, idx)) = exact {
                            return Some(format!("step_{}", idx));
                        }
                        // Fuzzy match: contains keyword
                        let fuzzy = desc_to_idx
                            .iter()
                            .find(|(d, _)| d.contains(dep_desc) || dep_desc.contains(d));
                        fuzzy.map(|(_, idx)| format!("step_{}", idx))
                    })
                    .collect();

                let mut step = PlanStep::new(step_output.description);
                if !deps.is_empty() {
                    step = step.with_dependencies(deps);
                }
                if let Some(eo) = step_output.expected_output {
                    step = step.with_expected_output(eo);
                }
                step
            })
            .collect();

        Plan::new(steps).with_goal(goal)
    }
}

impl Planner for LlmPlanner {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            info!(model = %self.model, mode = ?self.output_mode, "LlmPlanner: generating plan");

            let messages = vec![
                Message::system(self.system_prompt.clone()),
                Message::user(format!(
                    "Please create an execution plan for the following task:\n\n{}",
                    task
                )),
            ];

            let response_format = match self.output_mode {
                PlannerOutputMode::JsonSchema => Some(ResponseFormat::json_schema(
                    "plan_output",
                    plan_output_schema(),
                )),
                PlannerOutputMode::JsonText => None,
            };

            let response = llm::chat(
                self.client.clone(),
                &self.model,
                &messages,
                Some(0.3),
                Some(4096u32),
                Some(false),
                None,
                None,
                response_format,
            )
            .await?;

            let content = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_text())
                .unwrap_or_default();

            debug!(response = %content, "LlmPlanner raw response");

            let plan = match self.output_mode {
                PlannerOutputMode::JsonSchema => {
                    let output = Self::parse_structured_output(&content)?;
                    Self::resolve_plan_output(output, task)
                }
                PlannerOutputMode::JsonText => {
                    let steps = Self::parse_steps_legacy(&content);
                    if steps.is_empty() {
                        Plan::new(vec![PlanStep::new(task)]).with_goal(task)
                    } else {
                        Plan::new(steps).with_goal(task)
                    }
                }
            };

            info!(
                steps = plan.steps.len(),
                "Plan generated with {} steps",
                plan.steps.len()
            );

            Ok(plan)
        })
    }
}

// ── StaticPlanner ────────────────────────────────────────────────────────────

/// Static Planner: uses a predefined step list (suitable for testing or fixed workflows)
pub struct StaticPlanner {
    steps: Vec<String>,
}

impl StaticPlanner {
    /// Create a static planner with a predefined step list
    ///
    /// # Parameters
    /// * `steps` - Predefined step list (strings or types that can be converted to strings)
    pub fn new(steps: Vec<impl Into<String>>) -> Self {
        Self {
            steps: steps.into_iter().map(|s| s.into()).collect(),
        }
    }
}

impl Planner for StaticPlanner {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            let steps = self.steps.iter().map(PlanStep::new).collect();
            Ok(Plan::new(steps).with_goal(task))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_structured_output_json() {
        let response = r#"{"steps":[{"description":"Analyze code structure","dependencies":[],"expected_output":"Code structure report"},{"description":"Identify performance bottlenecks","dependencies":["Analyze code structure"]}]}"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
        assert_eq!(output.steps[0].description, "Analyze code structure");
        assert_eq!(output.steps[1].dependencies, vec!["Analyze code structure"]);
    }

    #[test]
    fn test_parse_structured_output_markdown() {
        let response = r#"```json
{"steps":[{"description":"Step one"},{"description":"Step two"}]}
```"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
    }

    #[test]
    fn test_auto_fix_array_wrapping() {
        let response = r#"[{"description":"Step A"},{"description":"Step B"}]"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 2);
    }

    #[test]
    fn test_auto_fix_trailing_comma() {
        let response = r#"{"steps":[{"description":"Step A",}]}"#;
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 1);
    }

    #[test]
    fn test_auto_fix_fallback() {
        let response = "Unparseable text";
        let output = LlmPlanner::parse_structured_output(response).unwrap();
        assert_eq!(output.steps.len(), 1); // Fallback to single-step plan
    }

    #[test]
    fn test_resolve_plan_output_with_deps() {
        let output = PlanOutput {
            steps: vec![
                PlanStepOutput {
                    description: "Analyze code".into(),
                    dependencies: vec![],
                    expected_output: None,
                },
                PlanStepOutput {
                    description: "Optimize performance".into(),
                    dependencies: vec!["Analyze code".into()],
                    expected_output: Some("Optimization report".into()),
                },
            ],
        };
        let plan = LlmPlanner::resolve_plan_output(output, "test");
        assert_eq!(plan.steps.len(), 2);
        assert!(plan.steps[0].dependencies.is_empty());
        assert_eq!(plan.steps[1].dependencies, vec!["step_0"]);
        assert_eq!(
            plan.steps[1].expected_output,
            Some("Optimization report".to_string())
        );
    }

    #[test]
    fn test_parse_steps_legacy_json() {
        let response = r#"```json
[{"description": "Step one"}, {"description": "Step two"}]
```"#;
        let steps = LlmPlanner::parse_steps_legacy(response);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].description, "Step one");
        assert_eq!(steps[1].description, "Step two");
    }

    #[test]
    fn test_parse_steps_legacy_plain_json() {
        let response =
            r#"[{"description": "Analyze code"}, {"description": "Optimize performance"}]"#;
        let steps = LlmPlanner::parse_steps_legacy(response);
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn test_parse_steps_legacy_fallback() {
        let response = "1. First step\n2. Second step\n3. Third step";
        let steps = LlmPlanner::parse_steps_legacy(response);
        assert_eq!(steps.len(), 3);
    }

    #[tokio::test]
    async fn test_static_planner() {
        let planner = StaticPlanner::new(vec!["Step A", "Step B", "Step C"]);
        let plan = planner.plan("Test task").await.unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "Step A");
        assert_eq!(plan.goal.as_deref(), Some("Test task"));
    }

    #[test]
    fn test_plan_output_schema_valid() {
        let schema = plan_output_schema();
        assert!(schema.is_object());
        assert!(schema["properties"]["steps"].is_object());
    }
}
