//! Plan-and-Execute core types, Planner trait, and PlanStore trait

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Compute simple similarity between two texts (0.0-1.0) ──────────────────

fn text_similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: std::collections::HashSet<char> = a.to_lowercase().chars().collect();
    let set_b: std::collections::HashSet<char> = b.to_lowercase().chars().collect();
    let intersection = set_a.intersection(&set_b).count() as f32;
    let union = set_a.union(&set_b).count() as f32;
    intersection / union
}

fn generate_plan_id() -> Option<String> {
    Some(format!("plan_{}", uuid::Uuid::new_v4().as_simple()))
}

fn now_secs() -> u64 {
    crate::utils::time::now_secs()
}

// ── Plan ────────────────────────────────────────────────────────────────────

/// Execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Plan unique ID (auto-generated)
    #[serde(default = "generate_plan_id")]
    pub id: Option<String>,
    /// Human-readable slug (e.g., "swift-fox")
    #[serde(default)]
    pub slug: Option<String>,
    /// Version number (incremented on each replan)
    #[serde(default)]
    pub version: u32,
    /// List of steps in the plan
    pub steps: Vec<PlanStep>,
    /// Overall goal description of the plan
    pub goal: Option<String>,
    /// Parent plan ID (for incremental replanning tracking)
    #[serde(default)]
    pub parent_plan_id: Option<String>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Creation timestamp (seconds)
    #[serde(default)]
    pub created_at: u64,
    /// Last update timestamp (seconds)
    #[serde(default)]
    pub updated_at: u64,
}

impl Plan {
    /// Create a new plan
    pub fn new(steps: Vec<PlanStep>) -> Self {
        let now = now_secs();
        Self {
            id: generate_plan_id(),
            slug: None,
            version: 1,
            steps,
            goal: None,
            parent_plan_id: None,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Set the plan goal description
    pub fn with_goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = Some(goal.into());
        self
    }

    /// Set a human-readable slug
    pub fn with_slug(mut self, slug: impl Into<String>) -> Self {
        self.slug = Some(slug.into());
        self
    }

    /// Set the parent plan ID (for incremental replanning)
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_plan_id = Some(parent_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Return the number of completed steps
    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count()
    }

    /// Check if the plan is fully completed
    pub fn is_completed(&self) -> bool {
        self.steps.iter().all(|s| s.status == StepStatus::Completed)
    }

    /// Touch the update timestamp
    pub fn touch(&mut self) {
        self.updated_at = now_secs();
    }

    // ── Validation & Auto-Fix ──────────────────────────────────────────────

    /// Validate the plan, returning all discovered issues
    pub fn validate(&self) -> Vec<PlanValidationIssue> {
        let mut issues = Vec::new();

        if self.steps.is_empty() {
            issues.push(PlanValidationIssue {
                severity: IssueSeverity::Error,
                message: "Plan has no steps".to_string(),
                fix: Some("Add at least one step".to_string()),
            });
        }

        let desc_set: std::collections::HashSet<&str> =
            self.steps.iter().map(|s| s.description.as_str()).collect();

        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.dependencies {
                if dep == &format!("step_{}", i) {
                    issues.push(PlanValidationIssue {
                        severity: IssueSeverity::Error,
                        message: format!("Step {} depends on itself", i),
                        fix: Some("Remove self-dependency".to_string()),
                    });
                }
            }
        }

        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.dependencies {
                if !dep.starts_with("step_") {
                    let matched = self.resolve_dependency(dep).is_some();
                    if !matched && !desc_set.contains(dep.as_str()) {
                        issues.push(PlanValidationIssue {
                            severity: IssueSeverity::Warning,
                            message: format!("Step {} has unresolvable dependency: {}", i, dep),
                            fix: Some("Remove or fix the dependency reference".to_string()),
                        });
                    }
                } else if let Ok(idx) = dep.trim_start_matches("step_").parse::<usize>()
                    && idx >= self.steps.len()
                {
                    issues.push(PlanValidationIssue {
                        severity: IssueSeverity::Error,
                        message: format!("Step {} depends on non-existent step index {}", i, idx),
                        fix: Some("Fix the dependency index".to_string()),
                    });
                }
            }
        }

        for (i, step) in self.steps.iter().enumerate() {
            if step.description.trim().is_empty() {
                issues.push(PlanValidationIssue {
                    severity: IssueSeverity::Warning,
                    message: format!("Step {} has empty description", i),
                    fix: Some("Provide a meaningful description".to_string()),
                });
            }
        }

        issues
    }

    /// Auto-fix fixable issues
    pub fn auto_fix(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        let steps_len = self.steps.len();

        use std::collections::{HashMap, HashSet};
        let mut all_deps = HashSet::new();
        for step in self.steps.iter() {
            for dep in &step.dependencies {
                all_deps.insert(dep.clone());
            }
        }

        let mut dependency_resolvable: HashMap<String, bool> = HashMap::new();
        for dep in &all_deps {
            let resolvable = self.resolve_dependency(dep).is_some();
            dependency_resolvable.insert(dep.clone(), resolvable);
        }

        for (i, step) in self.steps.iter_mut().enumerate() {
            let self_dep = format!("step_{}", i);
            let before = step.dependencies.len();
            step.dependencies.retain(|d| d != &self_dep);
            if step.dependencies.len() < before {
                fixes.push(format!("Removed self-dependency from step {}", i));
            }

            step.dependencies.retain(|d| {
                if let Some(idx_str) = d.strip_prefix("step_")
                    && let Ok(idx) = idx_str.parse::<usize>()
                {
                    return idx < steps_len;
                }
                *dependency_resolvable.get(d).unwrap_or(&false)
            });

            if step.description.trim().is_empty() {
                step.description = format!("Unnamed step {}", i);
                fixes.push(format!("Filled empty description for step {}", i));
            }
        }

        if !fixes.is_empty() {
            self.touch();
        }

        fixes
    }

    /// Resolve a dependency string to a step index, returning whether fuzzy matching was used
    pub fn resolve_dependency_with_fuzzy(&self, dep: &str) -> (Option<usize>, bool) {
        if let Some(idx_str) = dep.strip_prefix("step_")
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < self.steps.len()
        {
            return (Some(idx), false);
        }
        let result = self.resolve_dependency(dep);
        (result, result.is_some())
    }

    /// Resolve a dependency string to a step index
    ///
    /// Resolution strategy (in order of priority):
    /// 1. **Exact index reference** — `step_N` where N is a valid index
    /// 2. **Exact description match** — dep string exactly equals a step description
    /// 3. **Word-boundary substring** — dep is a word-bounded substring of a step description
    /// 4. **Reverse containment** — step description is a substring of dep
    /// 5. **Fuzzy text similarity** — Jaccard character-set similarity (threshold 0.6)
    pub fn resolve_dependency(&self, dep: &str) -> Option<usize> {
        // Strategy 1: Exact index reference (step_N)
        if let Some(idx_str) = dep.strip_prefix("step_")
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < self.steps.len()
        {
            return Some(idx);
        }

        // Strategy 2: Exact description match (case-insensitive)
        let dep_lower = dep.to_lowercase();
        for (idx, step) in self.steps.iter().enumerate() {
            if step.description.to_lowercase() == dep_lower {
                return Some(idx);
            }
        }

        // Strategies 3-5: fuzzy matching (with reduced confidence)
        let mut candidates = Vec::new();

        for (idx, step) in self.steps.iter().enumerate() {
            if dep.len() >= 3 && step.description.contains(dep) {
                let desc_lower = step.description.to_lowercase();

                let mut positions = Vec::new();
                let mut start = 0;
                while let Some(pos) = desc_lower[start..].find(&dep_lower) {
                    let actual_pos = start + pos;
                    positions.push(actual_pos);
                    start = actual_pos + 1;
                }

                let mut has_word_boundary = false;
                for &pos in &positions {
                    let prev_is_boundary = pos == 0
                        || !desc_lower
                            .chars()
                            .nth(pos - 1)
                            .is_some_and(|c| c.is_alphanumeric());
                    let next_pos = pos + dep.len();
                    let next_is_boundary = next_pos >= desc_lower.len()
                        || !desc_lower
                            .chars()
                            .nth(next_pos)
                            .is_some_and(|c| c.is_alphanumeric());

                    if prev_is_boundary && next_is_boundary {
                        has_word_boundary = true;
                        break;
                    }
                }

                if has_word_boundary {
                    candidates.push((idx, 1.0));
                } else if dep.len() >= 3 {
                    candidates.push((idx, 0.8));
                }
                continue;
            }

            if step.description.len() >= 3 && dep.contains(&step.description) {
                candidates.push((idx, 0.7));
                continue;
            }

            if dep.len() >= 5 {
                let similarity = text_similarity(dep, &step.description);
                if similarity >= 0.6 {
                    candidates.push((idx, similarity));
                }
            }
        }

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.first().map(|&(idx, _)| idx)
    }

    /// Get all downstream steps of a given step (steps that depend on it)
    pub fn downstream_steps(&self, step_idx: usize) -> Vec<usize> {
        let step_id = format!("step_{}", step_idx);
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.dependencies.contains(&step_id))
            .map(|(i, _)| i)
            .collect()
    }

    /// Recursively get all downstream steps (including transitive dependencies)
    pub fn downstream_steps_recursive(&self, step_idx: usize) -> Vec<usize> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.collect_downstream(step_idx, &mut result, &mut visited);
        result
    }

    fn collect_downstream(
        &self,
        step_idx: usize,
        result: &mut Vec<usize>,
        visited: &mut std::collections::HashSet<usize>,
    ) {
        if visited.contains(&step_idx) {
            return;
        }
        visited.insert(step_idx);
        for downstream in self.downstream_steps(step_idx) {
            if !result.contains(&downstream) {
                result.push(downstream);
            }
            self.collect_downstream(downstream, result, visited);
        }
    }
}

// ── Plan validation ─────────────────────────────────────────────────────────

/// Plan validation issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanValidationIssue {
    /// Severity level
    pub severity: IssueSeverity,
    /// Issue description
    pub message: String,
    /// Suggested fix
    #[serde(default)]
    pub fix: Option<String>,
}

/// Issue severity level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueSeverity {
    /// Error: plan cannot be executed
    Error,
    /// Warning: plan is executable but may have issues
    Warning,
}

// ── Plan step ───────────────────────────────────────────────────────────────

/// A single step in a plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step description
    pub description: String,
    /// Step status
    pub status: StepStatus,
    /// Expected input (dependency description on previous step results)
    pub expected_input: Option<String>,
    /// Expected output description
    pub expected_output: Option<String>,
    /// List of dependent step indices (generated from LLM planning output)
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl PlanStep {
    /// Create a new step
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            status: StepStatus::Pending,
            expected_input: None,
            expected_output: None,
            dependencies: Vec::new(),
        }
    }

    /// Set expected input description
    pub fn with_expected_input(mut self, input: impl Into<String>) -> Self {
        self.expected_input = Some(input.into());
        self
    }

    /// Set expected output description
    pub fn with_expected_output(mut self, output: impl Into<String>) -> Self {
        self.expected_output = Some(output.into());
        self
    }

    /// Set step dependencies
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }
}

/// Step execution status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Waiting to execute
    Pending,
    /// Currently executing
    Running,
    /// Completed
    Completed,
    /// Execution failed
    Failed,
}

// ── LLM structured output types ─────────────────────────────────────────────

/// Structured plan output returned by LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    /// List of steps
    pub steps: Vec<PlanStepOutput>,
}

/// Single step returned by LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepOutput {
    /// Step description
    pub description: String,
    /// Dependent step references (prefer 'step_N' IDs; description keywords accepted as fallback)
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Expected output
    #[serde(default)]
    pub expected_output: Option<String>,
}

/// Return JSON Schema for LLM structured output
pub fn plan_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {
                            "type": "string",
                            "description": "Detailed description of the step"
                        },
                        "dependencies": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Dependency references — prefer exact step IDs like 'step_0', 'step_1'. Only use description keywords when a step ID is unavailable."
                        },
                        "expected_output": {
                            "type": "string",
                            "description": "Expected output of the step"
                        }
                    },
                    "required": ["description"]
                },
                "minItems": 1
            }
        },
        "required": ["steps"]
    })
}

/// Execution result of a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step index in the plan
    pub step_index: usize,
    /// Step description
    pub description: String,
    /// Execution output
    pub output: String,
    /// Whether successful
    pub success: bool,
}

// ── Planner trait ───────────────────────────────────────────────────────────

/// Planner trait — accepts a task description and returns an execution plan
pub trait Planner: Send + Sync {
    /// Generate an execution plan based on the task description
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// ── StaticPlanner ───────────────────────────────────────────────────────────

/// Static Planner: uses a pre-defined list of steps (for testing)
pub struct StaticPlanner {
    steps: Vec<String>,
}

impl StaticPlanner {
    /// Create a StaticPlanner with predefined step descriptions
    pub fn new(steps: Vec<impl Into<String>>) -> Self {
        Self {
            steps: steps.into_iter().map(|s| s.into()).collect(),
        }
    }
}

impl Planner for StaticPlanner {
    fn plan<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<Plan>> {
        Box::pin(async move {
            let steps: Vec<PlanStep> = self
                .steps
                .iter()
                .map(|desc| PlanStep::new(desc.as_str()))
                .collect();
            Ok(Plan::new(steps))
        })
    }
}

// ── PlanStore trait ─────────────────────────────────────────────────────────

/// Lightweight plan summary for listing/search
#[derive(Debug, Clone)]
pub struct PlanSummary {
    /// Plan unique identifier
    pub id: String,
    /// Human-readable short identifier (for URLs, etc.)
    pub slug: Option<String>,
    /// Plan goal description
    pub goal: Option<String>,
    /// Plan version number (for optimistic locking)
    pub version: u32,
    /// Total number of steps in the plan
    pub total_steps: usize,
    /// Number of completed steps
    pub completed_steps: usize,
}

/// Trait for plan persistence operations
pub trait PlanStore: Send + Sync {
    /// Save plan to storage
    fn save_plan<'a>(&'a self, plan: &'a Plan) -> BoxFuture<'a, Result<()>>;
    /// Load plan by plan ID
    fn load_plan<'a>(&'a self, plan_id: &'a str) -> BoxFuture<'a, Result<Option<Plan>>>;
    /// Load plan by slug
    fn load_plan_by_slug<'a>(&'a self, slug: &'a str) -> BoxFuture<'a, Result<Option<Plan>>>;
    /// List plan summaries
    fn list_plans<'a>(&'a self, limit: usize) -> BoxFuture<'a, Result<Vec<PlanSummary>>>;
    /// Delete a plan
    fn delete_plan<'a>(&'a self, plan_id: &'a str) -> BoxFuture<'a, Result<bool>>;
    /// Search plans
    fn search_plans<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<PlanSummary>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_step_status() {
        let step = PlanStep::new("test step");
        assert_eq!(step.status, StepStatus::Pending);
        assert_eq!(step.description, "test step");
    }

    #[test]
    fn test_plan() {
        let plan = Plan::new(vec![
            PlanStep::new("step 1"),
            PlanStep::new("step 2"),
            PlanStep::new("step 3"),
        ]);
        assert_eq!(plan.steps.len(), 3);
        assert!(plan.id.is_some());
        assert_eq!(plan.version, 1);
    }

    #[test]
    fn test_plan_auto_id() {
        let plan = Plan::new(vec![PlanStep::new("test")]);
        assert!(plan.id.as_ref().unwrap().starts_with("plan_"));
    }

    #[test]
    fn test_plan_with_metadata() {
        let plan =
            Plan::new(vec![PlanStep::new("test")]).with_metadata("key", serde_json::json!("value"));
        assert_eq!(plan.metadata.get("key").unwrap(), "value");
    }

    #[test]
    fn test_validate_empty_plan() {
        let plan = Plan::new(vec![]);
        let issues = plan.validate();
        assert!(issues.iter().any(|i| i.message.contains("no steps")));
    }

    #[test]
    fn test_validate_self_dependency() {
        let plan = Plan::new(vec![
            PlanStep::new("step 0"),
            PlanStep::new("step 1").with_dependencies(vec!["step_1".to_string()]),
        ]);
        let issues = plan.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("depends on itself"))
        );
    }

    #[test]
    fn test_validate_invalid_index() {
        let plan = Plan::new(vec![
            PlanStep::new("step 0").with_dependencies(vec!["step_99".to_string()]),
        ]);
        let issues = plan.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("non-existent step index"))
        );
    }

    #[test]
    fn test_auto_fix_removes_self_dependency() {
        let mut plan = Plan::new(vec![
            PlanStep::new("step 0"),
            PlanStep::new("step 1")
                .with_dependencies(vec!["step_1".to_string(), "step_0".to_string()]),
        ]);
        let fixes = plan.auto_fix();
        assert!(fixes.iter().any(|f| f.contains("self-dependency")));
        assert_eq!(plan.steps[1].dependencies, vec!["step_0"]);
    }

    #[test]
    fn test_auto_fix_empty_description() {
        let mut plan = Plan::new(vec![PlanStep::new("")]);
        let fixes = plan.auto_fix();
        assert!(fixes.iter().any(|f| f.contains("empty description")));
        assert!(!plan.steps[0].description.is_empty());
    }

    #[test]
    fn test_downstream_steps() {
        let plan = Plan::new(vec![
            PlanStep::new("A"),
            PlanStep::new("B").with_dependencies(vec!["step_0".to_string()]),
            PlanStep::new("C").with_dependencies(vec!["step_0".to_string()]),
            PlanStep::new("D").with_dependencies(vec!["step_1".to_string()]),
        ]);
        let downstream = plan.downstream_steps(0);
        assert_eq!(downstream, vec![1, 2]);

        let recursive = plan.downstream_steps_recursive(0);
        assert!(recursive.contains(&1));
        assert!(recursive.contains(&2));
        assert!(recursive.contains(&3));
    }

    #[test]
    fn test_plan_touch() {
        let mut plan = Plan::new(vec![PlanStep::new("test")]);
        let before = plan.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        plan.touch();
        assert!(plan.updated_at >= before);
    }

    #[test]
    fn test_dependency_resolution_improved_matching() {
        let plan = Plan::new(vec![
            PlanStep::new("database migration"),
            PlanStep::new("setup environment"),
            PlanStep::new("group setup"),
        ]);

        let _result = plan.resolve_dependency("data");
        let _result = plan.resolve_dependency("setup");
        let result = plan.resolve_dependency("database migration");
        assert_eq!(result, Some(0));
        let result = plan.resolve_dependency("step_1");
        assert_eq!(result, Some(1));
        let result = plan.resolve_dependency("step_10");
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_static_planner() {
        let planner = StaticPlanner::new(vec!["A", "B", "C"]);
        let plan = planner.plan("test").await.unwrap();
        assert_eq!(plan.steps.len(), 3);
    }
}
