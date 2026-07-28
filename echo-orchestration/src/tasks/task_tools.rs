//! Framework task relation tools backed by [`TaskRevisionService`].

use std::str::FromStr;
use std::sync::Arc;

use echo_core::error::Result;
use echo_core::tools::{Tool, ToolContext, ToolParameters, ToolResult};
use futures::future::BoxFuture;

use super::{
    TaskCreateInput, TaskDraft, TaskGraphExecutionMode, TaskKind, TaskPlanPatchInputOp,
    TaskRevisionError, TaskRevisionService, TaskStatus, TaskUpdateInput,
};

pub struct TaskCreateTool {
    service: Arc<TaskRevisionService>,
}

impl TaskCreateTool {
    pub fn new(service: Arc<TaskRevisionService>) -> Self {
        Self { service }
    }
}

impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        "task_create"
    }

    fn description(&self) -> &str {
        "Create one task or atomically create a related task graph in the current TaskRun. Dependencies are optional; use base_revision when adding tasks to an existing graph."
    }

    fn parameters(&self) -> serde_json::Value {
        task_create_schema(&self.service)
    }

    fn execute_with_context<'a>(
        &'a self,
        params: ToolParameters,
        context: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let input = match parse_task_create_input(
                &params,
                &self.service.task_input_schema_extensions(),
            ) {
                Ok(input) => input,
                Err(message) => return Ok(ToolResult::error(message)),
            };
            let base_revision = input.base_revision;
            match self.service.create_from_tool(input, context).await {
                Ok(outcome) if outcome.appended => Ok(ToolResult::success(format!(
                    "Created task graph revision {} with {} total task(s)",
                    outcome.graph.snapshot.revision,
                    outcome.graph.snapshot.tasks.len()
                ))),
                Ok(outcome) => Ok(ToolResult::success(format!(
                    "Created task graph revision 1 with {} task(s). Call task_execute with revision=1.",
                    outcome.created_count
                ))),
                Err(TaskRevisionError::InvalidInput { message })
                | Err(TaskRevisionError::PolicyRejected { message }) => {
                    Ok(ToolResult::error(message))
                }
                Err(error) if base_revision.is_some() => Ok(ToolResult::error(format!(
                    "Failed to add tasks to revision {}: {error}",
                    base_revision.unwrap_or_default()
                ))),
                Err(error) => Ok(ToolResult::error(format!(
                    "Failed to create task graph: {error}"
                ))),
            }
        })
    }
}

pub struct TaskUpdateTool {
    service: Arc<TaskRevisionService>,
}

impl TaskUpdateTool {
    pub fn new(service: Arc<TaskRevisionService>) -> Self {
        Self { service }
    }
}

impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        "task_update"
    }

    fn description(&self) -> &str {
        "Atomically update task specifications, dependency relations, ordering, or skip state using optimistic concurrency. Only pending or blocked task specifications may change while a run is active."
    }

    fn parameters(&self) -> serde_json::Value {
        task_update_schema(&self.service)
    }

    fn execute_with_context<'a>(
        &'a self,
        params: ToolParameters,
        context: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let input = match parse_task_update_input(
                &params,
                &self.service.task_input_schema_extensions(),
                self.service.allow_manual_progress_updates(),
            ) {
                Ok(input) => input,
                Err(message) => return Ok(ToolResult::error(message)),
            };
            match self.service.update_from_tool(input, context).await {
                Ok(graph) => Ok(ToolResult::success(format!(
                    "Committed task graph revision {} with {} task(s)",
                    graph.snapshot.revision,
                    graph.snapshot.tasks.len()
                ))),
                Err(TaskRevisionError::GraphNotFound { .. }) => Ok(ToolResult::error(
                    "task_update requires existing tasks; call task_create first",
                )),
                Err(TaskRevisionError::PolicyRejected { message }) => {
                    Ok(ToolResult::error(message))
                }
                Err(error) => Ok(ToolResult::error(format!(
                    "Failed to update tasks: {error}"
                ))),
            }
        })
    }
}

pub struct TaskListTool {
    service: Arc<TaskRevisionService>,
}

impl TaskListTool {
    pub fn new(service: Arc<TaskRevisionService>) -> Self {
        Self { service }
    }
}

impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        "List the current TaskRun's tasks, dependency-aware graph revision, and runtime status."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    fn execute_with_context<'a>(
        &'a self,
        _params: ToolParameters,
        context: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let scope_id = match self.service.resolve_scope(context).await {
                Ok(scope_id) => scope_id,
                Err(error) => return Ok(ToolResult::error(error.to_string())),
            };
            match self.service.load(&scope_id).await {
                Ok(Some(graph)) => {
                    let lines = graph
                        .snapshot
                        .tasks
                        .iter()
                        .map(|task| {
                            format!(
                                "[{}] {} — {}",
                                status_name(&task.execution.status),
                                task.spec.id,
                                task.spec.title
                            )
                        })
                        .collect::<Vec<_>>();
                    Ok(ToolResult::success(format!(
                        "Task graph revision {} — Tasks ({}):\n{}",
                        graph.snapshot.revision,
                        graph.snapshot.tasks.len(),
                        lines.join("\n")
                    )))
                }
                Ok(None) => Ok(ToolResult::error("No tasks; call task_create first")),
                Err(error) => Ok(ToolResult::error(format!("Failed: {error}"))),
            }
        })
    }
}

pub fn build_task_tools(service: Arc<TaskRevisionService>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(TaskCreateTool::new(service.clone())),
        Box::new(TaskUpdateTool::new(service.clone())),
        Box::new(TaskListTool::new(service)),
    ]
}

pub fn build_task_create_tool(service: Arc<TaskRevisionService>) -> Box<dyn Tool> {
    Box::new(TaskCreateTool::new(service))
}

pub fn build_task_update_tool(service: Arc<TaskRevisionService>) -> Box<dyn Tool> {
    Box::new(TaskUpdateTool::new(service))
}

pub fn build_task_list_tool(service: Arc<TaskRevisionService>) -> Box<dyn Tool> {
    Box::new(TaskListTool::new(service))
}

fn task_create_schema(service: &TaskRevisionService) -> serde_json::Value {
    let task_schema = task_input_schema(service);
    serde_json::json!({
        "type": "object",
        "properties": {
            "task": task_schema.clone(),
            "tasks": {
                "type": "array",
                "minItems": 1,
                "description": "One atomic task batch. Dependency ids may refer to this batch or existing tasks.",
                "items": task_schema
            },
            "base_revision": {
                "type": "integer",
                "minimum": 1,
                "description": "Required when the current TaskRun already has tasks."
            },
            "reason": { "type": "string", "description": "Why these tasks are being added" },
            "assumptions": { "type": "array", "items": { "type": "string" } },
            "risks": { "type": "array", "items": { "type": "string" } },
            "execution_mode": { "type": "string", "enum": ["parallel", "sequential"] }
        },
        "oneOf": [
            { "required": ["task"] },
            { "required": ["tasks"] }
        ]
    })
}

fn task_update_schema(service: &TaskRevisionService) -> serde_json::Value {
    let task_schema = task_input_schema(service);
    let mut operation_schemas = vec![
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "op": { "const": "insert" },
                "after_task_id": { "type": ["string", "null"] },
                "task": task_schema
            },
            "required": ["op", "task"]
        }),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "op": { "const": "update" },
                "task_id": { "type": "string" },
                "patch": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "title": { "type": "string", "description": "User-visible task title in the user's current language" },
                        "description": { "type": "string", "description": "Subagent brief in the user's current language" },
                        "kind": task_kind_schema(),
                        "agent_role": { "type": "string" },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "files": { "type": "array", "items": { "type": "string" } },
                        "allowed_tools": { "type": "array", "items": { "type": "string" } },
                        "required_artifacts": { "type": "array", "items": { "type": "string" } },
                        "execution_checks": { "type": "array", "items": { "type": "string" } },
                        "acceptance_criteria": { "type": "array", "items": { "type": "string" } },
                        "max_retries": { "type": "integer", "minimum": 0, "maximum": 10 }
                    }
                }
            },
            "required": ["op", "task_id", "patch"]
        }),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "op": { "const": "skip" },
                "task_id": { "type": "string" }
            },
            "required": ["op", "task_id"]
        }),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "op": { "const": "reorder" },
                "task_ids": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["op", "task_ids"]
        }),
    ];
    if service.allow_manual_progress_updates() {
        operation_schemas.push(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "op": { "const": "set_status" },
                "task_id": { "type": "string" },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] }
            },
            "required": ["op", "task_id", "status"]
        }));
    }
    serde_json::json!({
        "type": "object",
        "properties": {
            "base_revision": { "type": "integer", "minimum": 1 },
            "reason": { "type": "string", "description": "Why runtime evidence requires this revision" },
            "operations": {
                "type": "array",
                "minItems": 1,
                "items": { "oneOf": operation_schemas }
            }
        },
        "required": ["base_revision", "reason", "operations"]
    })
}

fn task_input_schema(service: &TaskRevisionService) -> serde_json::Value {
    let mut properties = serde_json::Map::from_iter([
        (
            "id".to_string(),
            serde_json::json!({ "type": "string", "description": "Stable task id unique within this run" }),
        ),
        (
            "title".to_string(),
            serde_json::json!({ "type": "string", "description": "User-visible task title in the user's current language; keep technical identifiers unchanged" }),
        ),
        (
            "description".to_string(),
            serde_json::json!({ "type": "string", "description": "Subagent brief in the user's current language; keep code, paths, commands, and technical identifiers unchanged" }),
        ),
        ("kind".to_string(), task_kind_schema()),
        (
            "subagent".to_string(),
            serde_json::json!({ "type": "string", "description": "Registered Subagent role; omit for the domain default" }),
        ),
        (
            "depends_on".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "files".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "allowed_tools".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "required_artifacts".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "execution_checks".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "acceptance_criteria".to_string(),
            serde_json::json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "max_retries".to_string(),
            serde_json::json!({ "type": "integer", "minimum": 0, "maximum": 10 }),
        ),
    ]);
    for (key, schema) in service.task_input_schema_extensions() {
        properties.insert(key, schema);
    }
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": ["id", "title", "description", "kind"]
    })
}

fn task_kind_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "enum": ["implementation", "debugging", "verification", "review", "investigation", "test_plan", "summary", "read_only_review"]
    })
}

fn parse_task_create_input(
    params: &ToolParameters,
    extension_schemas: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<TaskCreateInput, String> {
    let single_task = params.get("task");
    let task_batch = params.get("tasks").and_then(serde_json::Value::as_array);
    if single_task.is_some() == task_batch.is_some() {
        return Err("task_create requires exactly one of task or tasks".to_string());
    }
    let raw_tasks = match (single_task, task_batch) {
        (Some(task), None) if task.is_object() => vec![task],
        (None, Some(tasks)) => tasks.iter().collect::<Vec<_>>(),
        (Some(_), None) => return Err("task_create task must be an object".to_string()),
        _ => Vec::new(),
    };
    if raw_tasks.is_empty() {
        return Err("task_create requires at least one task".to_string());
    }
    let tasks = raw_tasks
        .into_iter()
        .enumerate()
        .map(|(index, value)| parse_task_draft(value, index, extension_schemas))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let execution_mode = match params
        .get("execution_mode")
        .and_then(serde_json::Value::as_str)
    {
        Some("sequential") => TaskGraphExecutionMode::Sequential,
        _ => TaskGraphExecutionMode::Parallel,
    };
    Ok(TaskCreateInput {
        tasks,
        base_revision: params
            .get("base_revision")
            .and_then(serde_json::Value::as_u64),
        reason: params
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        assumptions: string_array_in_parameters(params, "assumptions"),
        risks: string_array_in_parameters(params, "risks"),
        execution_mode,
    })
}

fn parse_task_update_input(
    params: &ToolParameters,
    extension_schemas: &serde_json::Map<String, serde_json::Value>,
    allow_manual_progress_updates: bool,
) -> std::result::Result<TaskUpdateInput, String> {
    let base_revision = params
        .get("base_revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "task_update requires base_revision".to_string())?;
    let reason = params
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let raw_operations = params
        .get("operations")
        .ok_or_else(|| "task_update requires operations".to_string())?
        .as_array()
        .ok_or_else(|| "task_update operations must be an array".to_string())?;
    let mut operations = Vec::with_capacity(raw_operations.len());
    for (index, operation) in raw_operations.iter().enumerate() {
        let op = operation
            .get("op")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("operations[{index}].op is required"))?;
        let parsed = match op {
            "insert" => {
                let task = operation
                    .get("task")
                    .ok_or_else(|| format!("operations[{index}].task is required"))?;
                TaskPlanPatchInputOp::Insert {
                    after_task_id: operation
                        .get("after_task_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    task: parse_task_draft(task, index, extension_schemas)?,
                }
            }
            "update" => TaskPlanPatchInputOp::Update {
                task_id: required_task_id(operation, index)?,
                patch: operation
                    .get("patch")
                    .cloned()
                    .ok_or_else(|| format!("operations[{index}].patch is required"))
                    .and_then(|patch| {
                        serde_json::from_value(patch)
                            .map_err(|error| format!("operations[{index}].patch: {error}"))
                    })?,
            },
            "skip" => TaskPlanPatchInputOp::Skip {
                task_id: required_task_id(operation, index)?,
            },
            "reorder" => TaskPlanPatchInputOp::Reorder {
                task_ids: string_array_in(operation, "task_ids"),
            },
            "set_status" if allow_manual_progress_updates => {
                let status = operation
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("operations[{index}].status is required"))?;
                let status = match status {
                    "pending" => TaskStatus::Pending,
                    "in_progress" => TaskStatus::Running,
                    "completed" => TaskStatus::Completed,
                    "cancelled" => TaskStatus::Cancelled,
                    other => {
                        return Err(format!("operations[{index}] has unknown status '{other}'"));
                    }
                };
                TaskPlanPatchInputOp::SetStatus {
                    task_id: required_task_id(operation, index)?,
                    status,
                }
            }
            other => return Err(format!("operations[{index}] has unknown op '{other}'")),
        };
        operations.push(parsed);
    }
    Ok(TaskUpdateInput {
        base_revision,
        reason,
        operations,
    })
}

fn parse_task_draft(
    value: &serde_json::Value,
    index: usize,
    extension_schemas: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<TaskDraft, String> {
    let field = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("tasks[{index}].{key} is required"))
    };
    let kind_name = field("kind")?;
    let kind =
        TaskKind::from_str(&kind_name).map_err(|_| format!("unknown task kind '{kind_name}'"))?;
    let max_retries = value
        .get("max_retries")
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(3);
    let mut extensions = serde_json::Map::new();
    for key in extension_schemas.keys() {
        if let Some(extension) = value.get(key) {
            extensions.insert(key.clone(), extension.clone());
        }
    }
    Ok(TaskDraft {
        id: field("id")?,
        title: field("title")?,
        description: field("description")?,
        kind,
        subagent: value
            .get("subagent")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string),
        depends_on: string_array_in(value, "depends_on"),
        files: string_array_in(value, "files"),
        allowed_tools: string_array_in(value, "allowed_tools"),
        required_artifacts: string_array_in(value, "required_artifacts"),
        execution_checks: string_array_in(value, "execution_checks"),
        acceptance_criteria: string_array_in(value, "acceptance_criteria"),
        max_retries,
        extensions: serde_json::Value::Object(extensions),
    })
}

fn required_task_id(
    operation: &serde_json::Value,
    index: usize,
) -> std::result::Result<String, String> {
    operation
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|task_id| !task_id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("operations[{index}].task_id is required"))
}

fn string_array_in(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn string_array_in_parameters(value: &ToolParameters, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn status_name(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Blocked(_) => "blocked",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed(_) => "failed",
        TaskStatus::Skipped => "skipped",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::TimedOut { .. } => "timed_out",
        TaskStatus::Retrying { .. } => "retrying",
        TaskStatus::Paused(_) => "paused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{DefaultTaskToolPolicy, InMemoryRevisionedTaskStore};

    fn service() -> Arc<TaskRevisionService> {
        Arc::new(TaskRevisionService::new(
            Arc::new(InMemoryRevisionedTaskStore::new()),
            Arc::new(DefaultTaskToolPolicy::new("tool-test")),
        ))
    }

    fn parameters(value: serde_json::Value) -> std::result::Result<ToolParameters, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    #[tokio::test]
    async fn tools_create_update_and_list_one_graph() -> std::result::Result<(), String> {
        let service = service();
        let create = TaskCreateTool::new(service.clone());
        let created = create
            .execute(parameters(serde_json::json!({
                "task": {
                    "id": "分析",
                    "title": "分析问题",
                    "description": "检查输入",
                    "kind": "investigation",
                    "acceptance_criteria": ["完成"]
                }
            }))?)
            .await
            .map_err(|error| error.to_string())?;
        assert!(created.success);

        let update = TaskUpdateTool::new(service.clone());
        let updated = update
            .execute(parameters(serde_json::json!({
                "base_revision": 1,
                "reason": "开始",
                "operations": [{"op": "set_status", "task_id": "分析", "status": "in_progress"}]
            }))?)
            .await
            .map_err(|error| error.to_string())?;
        assert!(updated.success);

        let list = TaskListTool::new(service);
        let listed = list
            .execute(parameters(serde_json::json!({}))?)
            .await
            .map_err(|error| error.to_string())?;
        assert!(listed.success);
        assert!(listed.output.contains("revision 2"));
        assert!(listed.output.contains("分析问题"));
        Ok(())
    }

    #[test]
    fn default_schema_exposes_manual_status_updates() {
        let schema = task_update_schema(&service());
        assert!(schema.to_string().contains("set_status"));
    }
}
