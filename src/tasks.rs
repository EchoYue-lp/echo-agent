//! Tasks facade
//!
//! This module is the public SDK re-export of `echo_orchestration::tasks`.

/// Direct re-exports from `echo_orchestration::tasks`.
pub mod orchestration {
    pub use echo_orchestration::tasks::*;
}

pub use echo_orchestration::tasks::*;

/// Replace the Agent's task relation tools with tools backed by `service`.
/// Tool registration is name-based, so this atomically selects the supplied
/// store/policy service without exposing a second task API.
pub fn register_task_tools(
    agent: &mut crate::agent::ReactAgent,
    service: std::sync::Arc<TaskRevisionService>,
) {
    for tool in build_task_tools(service) {
        agent.replace_tool(tool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_task_tools_replaces_default_service_used_by_tool_manager()
    -> Result<(), String> {
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::minimal(
            "test-model",
            "task-tool-replacement",
        ));
        if agent.tool_manager().get_tool("task_create").is_none() {
            return Err("ReactAgent default task_create tool is missing".to_string());
        }

        let replacement_service = std::sync::Arc::new(TaskRevisionService::new(
            std::sync::Arc::new(InMemoryRevisionedTaskStore::new()),
            std::sync::Arc::new(DefaultTaskToolPolicy::new("replacement-scope")),
        ));
        register_task_tools(&mut agent, replacement_service.clone());

        let parameters = serde_json::from_value(serde_json::json!({
            "tasks": [{
                "id": "replacement-task",
                "title": "Use the replacement task store",
                "description": "Prove ToolManager dispatches to the supplied service",
                "extension": {
                    "kind": "investigation",
                    "acceptance_criteria": ["replacement graph is persisted"]
                }
            }]
        }))
        .map_err(|error| error.to_string())?;
        let result = agent
            .tool_manager()
            .execute_tool("task_create", parameters)
            .await
            .map_err(|error| error.to_string())?;
        if !result.success {
            return Err(format!("replacement task_create failed: {}", result.output));
        }

        let graph = replacement_service
            .load("replacement-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "replacement service did not persist the task graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "replacement graph contains no tasks".to_string())?;
        assert_eq!(task.spec.id, "replacement-task");
        assert_eq!(
            task.spec
                .extension
                .get("kind")
                .and_then(serde_json::Value::as_str),
            Some("investigation")
        );
        Ok(())
    }
}
