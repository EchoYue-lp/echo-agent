//! Create plan tool — allows LLM to output complete plans
//!
//! This tool allows the LLM to output a complete `PlanSpec` in JSON format,
//! which is then validated by the framework before creating the task DAG.

use futures::future::BoxFuture;
use std::sync::Arc;

use crate::error::ToolError;
use crate::tasks::TaskManager;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_orchestration::planning::{PlanSpec, PlanValidator};
use serde_json::{Value, json};

/// Tool for creating structured task plans
#[allow(dead_code)]
pub struct CreatePlanTool {
    task_manager: Arc<TaskManager>,
    validator: PlanValidator,
}

#[allow(dead_code)]
impl CreatePlanTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self {
            task_manager,
            validator: PlanValidator::new(),
        }
    }

    /// Create with custom validator
    pub fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }
}

impl Tool for CreatePlanTool {
    fn name(&self) -> &str {
        "create_plan"
    }

    fn description(&self) -> &str {
        r#"Create a complete task plan with dependencies, milestones, and verification.

This tool allows you to output a complete plan specification that will be
validated by the framework before creating the task DAG. The plan should
include:

1. **Goal**: Overall objective of the plan
2. **Assumptions**: Assumptions made during planning
3. **Tasks**: List of tasks with:
   - Unique ID
   - Type (discovery/implementation/verification/background/delegation)
   - Description
   - Acceptance criteria (required)
   - Inputs/outputs
   - Verification method (required)
   - Risk level, parallelization, write access
4. **Dependencies**: Edges between tasks (from -> to)
5. **Milestones**: Progress tracking checkpoints
6. **Verification strategy**: per_task, per_milestone, final_only, or continuous

Example:
```json
{
  "goal": "Implement user authentication",
  "assumptions": ["Using JWT tokens", "PostgreSQL database"],
  "tasks": [
    {
      "id": "task1",
      "task_type": "discovery",
      "description": "Research authentication best practices",
      "acceptance_criteria": ["Document found", "Best practices listed"],
      "verification": {"verification_type": "llm_review"},
      "risk_level": "low",
      "can_parallelize": true,
      "requires_write_access": false
    },
    {
      "id": "task2",
      "task_type": "implementation",
      "description": "Implement JWT authentication",
      "acceptance_criteria": ["Login endpoint works", "Token validation works"],
      "verification": {"verification_type": "test", "command": "cargo test auth"},
      "risk_level": "high",
      "can_parallelize": false,
      "requires_write_access": true,
      "dependencies": ["task1"]
    }
  ],
  "edges": [
    {"from": "task2", "to": "task1", "dependency_type": "required"}
  ],
  "milestones": [
    {
      "id": "m1",
      "name": "Research Complete",
      "description": "All research tasks completed",
      "success_criteria": ["Research documented"],
      "task_ids": ["task1"]
    }
  ],
  "verification_strategy": "per_task"
}
```"#
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "object",
                    "description": "Complete plan specification in JSON format",
                    "properties": {
                        "goal": {
                            "type": "string",
                            "description": "Overall goal of the plan"
                        },
                        "assumptions": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Assumptions made during planning"
                        },
                        "tasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string"},
                                    "task_type": {
                                        "type": "string",
                                        "enum": ["discovery", "implementation", "verification", "background", "delegation"]
                                    },
                                    "description": {"type": "string"},
                                    "acceptance_criteria": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "inputs": {"type": "array"},
                                    "expected_outputs": {"type": "array"},
                                    "allowed_tools": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "context_scope": {
                                        "type": "string",
                                        "enum": ["minimal", "relevant", "full", "isolated"]
                                    },
                                    "risk_level": {
                                        "type": "string",
                                        "enum": ["low", "medium", "high"]
                                    },
                                    "can_parallelize": {"type": "boolean"},
                                    "requires_write_access": {"type": "boolean"},
                                    "verification": {
                                        "type": "object",
                                        "properties": {
                                            "verification_type": {
                                                "type": "string",
                                                "enum": ["command", "file_exists", "diff_check", "test", "human_review", "llm_review", "none"]
                                            },
                                            "command": {"type": "string"},
                                            "expected": {"type": "string"},
                                            "timeout_secs": {"type": "integer"},
                                            "retry_count": {"type": "integer"},
                                            "fallback_on_failure": {
                                                "type": "string",
                                                "enum": ["retry", "replan", "ask_user", "abort"]
                                            }
                                        }
                                    },
                                    "checkpoint_policy": {
                                        "type": "string",
                                        "enum": ["after_each", "on_milestone", "on_failure", "never"]
                                    },
                                    "priority": {"type": "integer", "minimum": 0, "maximum": 10},
                                    "assigned_agent": {"type": "string"},
                                    "tags": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "estimated_duration_secs": {"type": "integer"}
                                },
                                "required": ["id", "task_type", "description", "acceptance_criteria", "verification"]
                            }
                        },
                        "edges": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "from": {"type": "string"},
                                    "to": {"type": "string"},
                                    "dependency_type": {
                                        "type": "string",
                                        "enum": ["required", "preferred", "optional"]
                                    }
                                },
                                "required": ["from", "to", "dependency_type"]
                            }
                        },
                        "milestones": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string"},
                                    "name": {"type": "string"},
                                    "description": {"type": "string"},
                                    "success_criteria": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "task_ids": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "checkpoint_policy": {
                                        "type": "string",
                                        "enum": ["after_each", "on_milestone", "on_failure", "never"]
                                    }
                                },
                                "required": ["id", "name", "description", "success_criteria", "task_ids"]
                            }
                        },
                        "verification_strategy": {
                            "type": "string",
                            "enum": ["per_task", "per_milestone", "final_only", "continuous"]
                        },
                        "estimated_complexity": {
                            "type": "string",
                            "enum": ["low", "medium", "high"]
                        },
                        "context_budget": {"type": "integer"},
                        "fallback_strategy": {
                            "type": "string",
                            "enum": ["replan", "ask_user", "abort"]
                        }
                    },
                    "required": ["goal", "tasks"]
                }
            },
            "required": ["plan"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let plan_value = parameters
                .get("plan")
                .ok_or_else(|| ToolError::MissingParameter("plan".to_string()))?;

            // Parse PlanSpec from JSON
            let plan: PlanSpec = serde_json::from_value(plan_value.clone()).map_err(|e| {
                ToolError::InvalidParameter {
                    name: "plan".to_string(),
                    message: format!("Invalid plan specification: {}", e),
                }
            })?;

            // Validate the plan
            let report = self.validator.validate(&plan);

            if !report.is_valid() {
                let error_msg = format!("Plan validation failed:\n{}", report.errors.join("\n"));
                return Err(ToolError::InvalidParameter {
                    name: "plan".to_string(),
                    message: error_msg,
                }
                .into());
            }

            // Log warnings
            if report.has_warnings() {
                tracing::warn!("Plan validation warnings:\n{}", report.warnings.join("\n"));
            }

            // Convert PlanSpec to Tasks
            let tasks = plan.to_tasks();

            // Add tasks to TaskManager
            let mut created_tasks = Vec::new();
            for task in tasks {
                self.task_manager.add_task(task.clone());
                created_tasks.push(task.id.clone());
            }

            // Return success with created task IDs
            let result = json!({
                "success": true,
                "goal": plan.goal,
                "tasks_created": created_tasks.len(),
                "task_ids": created_tasks,
                "milestones": plan.milestones.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                "warnings": report.warnings,
                "message": format!(
                    "Plan created successfully with {} tasks and {} milestones",
                    plan.tasks.len(),
                    plan.milestones.len()
                )
            });

            Ok(ToolResult::success(
                serde_json::to_string_pretty(&result).unwrap(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_plan_tool_valid() {
        let task_manager = Arc::new(TaskManager::new());
        let tool = CreatePlanTool::new(task_manager.clone());

        let plan = json!({
            "goal": "Test goal",
            "tasks": [
                {
                    "id": "task1",
                    "task_type": "discovery",
                    "description": "First task",
                    "acceptance_criteria": ["Criterion 1"],
                    "verification": {"verification_type": "llm_review"},
                    "risk_level": "low",
                    "can_parallelize": true,
                    "requires_write_access": false
                },
                {
                    "id": "task2",
                    "task_type": "implementation",
                    "description": "Second task",
                    "acceptance_criteria": ["Criterion 2"],
                    "verification": {"verification_type": "test"},
                    "risk_level": "medium",
                    "can_parallelize": false,
                    "requires_write_access": true
                }
            ],
            "edges": [
                {"from": "task2", "to": "task1", "dependency_type": "required"}
            ]
        });

        let params: ToolParameters = serde_json::from_value(json!({"plan": plan})).unwrap();
        let result = tool.execute(params).await;

        if let Err(ref e) = result {
            eprintln!("Error: {:?}", e);
        }

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        // Check tasks were created
        let tasks = task_manager.get_all_tasks();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.id == "task1"));
        assert!(tasks.iter().any(|t| t.id == "task2"));

        // Check dependency was set
        let task2 = tasks.iter().find(|t| t.id == "task2").unwrap();
        assert!(task2.dependencies.contains(&"task1".to_string()));
    }

    #[tokio::test]
    async fn test_create_plan_tool_invalid() {
        let task_manager = Arc::new(TaskManager::new());
        let tool = CreatePlanTool::new(task_manager.clone());

        // Missing acceptance criteria
        let plan = json!({
            "goal": "Test goal",
            "tasks": [
                {
                    "id": "task1",
                    "task_type": "discovery",
                    "description": "First task",
                    "acceptance_criteria": [], // Empty!
                    "verification": {"verification_type": "llm_review"}
                }
            ]
        });

        let params: ToolParameters = serde_json::from_value(json!({"plan": plan})).unwrap();
        let result = tool.execute(params).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("acceptance criteria"));
    }

    #[tokio::test]
    async fn test_create_plan_tool_circular_dependency() {
        let task_manager = Arc::new(TaskManager::new());
        let tool = CreatePlanTool::new(task_manager.clone());

        let plan = json!({
            "goal": "Test goal",
            "tasks": [
                {
                    "id": "task1",
                    "task_type": "discovery",
                    "description": "First task",
                    "acceptance_criteria": ["Criterion 1"],
                    "verification": {"verification_type": "llm_review"}
                },
                {
                    "id": "task2",
                    "task_type": "discovery",
                    "description": "Second task",
                    "acceptance_criteria": ["Criterion 2"],
                    "verification": {"verification_type": "llm_review"}
                }
            ],
            "edges": [
                {"from": "task1", "to": "task2", "dependency_type": "required"},
                {"from": "task2", "to": "task1", "dependency_type": "required"}
            ]
        });

        let params: ToolParameters = serde_json::from_value(json!({"plan": plan})).unwrap();
        let result = tool.execute(params).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("circular"));
    }
}
