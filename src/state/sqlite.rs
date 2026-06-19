//! SQLite-backed RuntimeStateStore implementation.

use super::{AgentCheckpoint, RuntimeStateStore, TaskNode, TaskNodeStatus};
use crate::error::{Result, RuntimeStateError};
use chrono::Utc;
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use std::path::Path;

/// SQLite-backed runtime state store.
pub struct SqliteRuntimeStateStore {
    path: std::path::PathBuf,
}

impl SqliteRuntimeStateStore {
    /// Create a new SQLite state store.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| RuntimeStateError::Io(format!("failed to create directory: {}", e)))?;
        }
        let store = Self { path };
        store.init_tables()?;
        Ok(store)
    }

    fn open_conn(&self) -> Result<Connection> {
        Connection::open(&self.path).map_err(|e| {
            RuntimeStateError::Io(format!("failed to open SQLite connection: {}", e)).into()
        })
    }

    /// Initialize the SQLite tables.
    pub fn init_tables(&self) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute_batch(
            r#"
            -- Task nodes table
            CREATE TABLE IF NOT EXISTS task_nodes (
                id              TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                name            TEXT NOT NULL,
                status          TEXT NOT NULL,
                dependencies    TEXT,
                outputs         TEXT,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL,
                PRIMARY KEY (id, conversation_id)
            );

            -- Agent checkpoints table (working_dir added in migration below)
            CREATE TABLE IF NOT EXISTS agent_checkpoints (
                conversation_id TEXT PRIMARY KEY,
                messages_json   TEXT NOT NULL,
                current_plan    TEXT,
                active_skills   TEXT NOT NULL,
                blocked_reason  TEXT,
                working_dir     TEXT,
                timestamp       TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_task_nodes_conversation
                ON task_nodes(conversation_id);
            "#,
        )
        .map_err(|e| RuntimeStateError::Io(format!("failed to init tables: {}", e)))?;

        // Migration: add working_dir column for databases created before this
        // column was added (P1-6).  ALTER TABLE ADD COLUMN does not support
        // IF NOT EXISTS; we catch "duplicate column name" for databases that
        // already have it (e.g. created fresh above).
        if let Err(e) = conn.execute(
            "ALTER TABLE agent_checkpoints ADD COLUMN working_dir TEXT",
            [],
        ) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(RuntimeStateError::Io(format!(
                    "failed to add working_dir column: {msg}"
                ))
                .into());
            }
        }
        Ok(())
    }

    /// Delete all state for a conversation (sync version).
    pub fn clear_conversation_sync(&self, conversation_id: &str) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            "DELETE FROM task_nodes WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| RuntimeStateError::Io(format!("failed to clear nodes: {}", e)))?;
        conn.execute(
            "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| RuntimeStateError::Io(format!("failed to clear checkpoint: {}", e)))?;
        Ok(())
    }
}

impl RuntimeStateStore for SqliteRuntimeStateStore {
    fn save_node<'a>(
        &'a self,
        conversation_id: &'a str,
        node: &'a TaskNode,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let deps = serde_json::to_string(&node.dependencies).map_err(|e| {
                crate::error::ReactError::Other(format!("serialize dependencies: {e}"))
            })?;
            let outputs = serde_json::to_string(&node.outputs)
                .map_err(|e| crate::error::ReactError::Other(format!("serialize outputs: {e}")))?;
            conn.execute(
                r#"
                INSERT INTO task_nodes (id, conversation_id, name, status, dependencies, outputs, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(id, conversation_id) DO UPDATE SET
                    name = excluded.name,
                    status = excluded.status,
                    dependencies = excluded.dependencies,
                    outputs = excluded.outputs,
                    updated_at = excluded.updated_at
                "#,
                params![
                    &node.id,
                    conversation_id,
                    &node.name,
                serde_json::to_string(&node.status)
                    .map_err(|e| crate::error::ReactError::Other(format!("serialize status: {e}")))?,
                    deps,
                    outputs,
                    node.created_at.to_rfc3339(),
                    node.updated_at.to_rfc3339(),
                ],
            )
            .map_err(|e| RuntimeStateError::Io(format!("failed to save node: {}", e)))?;
            Ok(())
        })
    }

    fn load_nodes<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<Vec<TaskNode>>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, status, dependencies, outputs, created_at, updated_at
                 FROM task_nodes WHERE conversation_id = ?1",
                )
                .map_err(|e| RuntimeStateError::Io(format!("failed to prepare query: {}", e)))?;

            let rows = stmt
                .query_map(params![conversation_id], |row| {
                    let id: String = row.get(0)?;
                    let name: String = row.get(1)?;
                    let status_str: String = row.get(2)?;
                    let deps_str: String = row.get(3)?;
                    let outputs_str: String = row.get(4)?;
                    let created_at_str: String = row.get(5)?;
                    let updated_at_str: String = row.get(6)?;

                    let status: TaskNodeStatus =
                        serde_json::from_str(&status_str).unwrap_or(TaskNodeStatus::Pending);
                    let dependencies: Vec<String> =
                        serde_json::from_str(&deps_str).unwrap_or_default();
                    let outputs: serde_json::Value =
                        serde_json::from_str(&outputs_str).unwrap_or(serde_json::Value::Null);
                    let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| Utc::now());
                    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| Utc::now());

                    Ok(TaskNode {
                        id,
                        name,
                        status,
                        dependencies,
                        outputs,
                        created_at,
                        updated_at,
                    })
                })
                .map_err(|e| RuntimeStateError::Io(format!("failed to query nodes: {}", e)))?;

            let mut nodes = Vec::new();
            for row in rows {
                nodes.push(
                    row.map_err(|e| RuntimeStateError::Io(format!("failed to read row: {}", e)))?,
                );
            }
            Ok(nodes)
        })
    }

    fn update_status<'a>(
        &'a self,
        conversation_id: &'a str,
        node_id: &'a str,
        status: TaskNodeStatus,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let status_str = serde_json::to_string(&status).unwrap_or_default();
            conn.execute(
                "UPDATE task_nodes SET status = ?1, updated_at = ?2 WHERE id = ?3 AND conversation_id = ?4",
                params![status_str, Utc::now().to_rfc3339(), node_id, conversation_id],
            )
            .map_err(|e| RuntimeStateError::Io(format!("failed to update status: {}", e)))?;
            Ok(())
        })
    }

    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<AgentCheckpoint>>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let mut stmt = conn
                .prepare(
                    "SELECT messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp
                 FROM agent_checkpoints WHERE conversation_id = ?1",
                )
                .map_err(|e| RuntimeStateError::Io(format!("failed to prepare query: {}", e)))?;

            let result = stmt.query_row(params![conversation_id], |row| {
                let messages_json: String = row.get(0)?;
                let current_plan: Option<String> = row.get(1)?;
                let active_skills_str: String = row.get(2)?;
                let blocked_reason: Option<String> = row.get(3)?;
                let working_dir: Option<String> = row.get(4).ok(); // nullable; may not exist in old rows
                let timestamp_str: String = row.get(5)?;

                let active_skills: Vec<String> =
                    serde_json::from_str(&active_skills_str).unwrap_or_default();
                let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(AgentCheckpoint {
                    conversation_id: conversation_id.to_string(),
                    messages_json,
                    current_plan,
                    active_skills,
                    blocked_reason,
                    working_dir: working_dir.map(std::path::PathBuf::from),
                    timestamp,
                })
            });

            match result {
                Ok(cp) => Ok(Some(cp)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => {
                    Err(RuntimeStateError::Io(format!("failed to query checkpoint: {}", e)).into())
                }
            }
        })
    }

    fn save_checkpoint<'a>(&'a self, checkpoint: &'a AgentCheckpoint) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let active_skills_str =
                serde_json::to_string(&checkpoint.active_skills).unwrap_or_default();
            conn.execute(
                r#"
                INSERT INTO agent_checkpoints (conversation_id, messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(conversation_id) DO UPDATE SET
                    messages_json = excluded.messages_json,
                    current_plan = excluded.current_plan,
                    active_skills = excluded.active_skills,
                    blocked_reason = excluded.blocked_reason,
                    working_dir = excluded.working_dir,
                    timestamp = excluded.timestamp
                "#,
                params![
                    &checkpoint.conversation_id,
                    &checkpoint.messages_json,
                    checkpoint.current_plan.as_deref(),
                    active_skills_str,
                    checkpoint.blocked_reason.as_deref(),
                    checkpoint.working_dir.as_ref().and_then(|p| p.to_str()),
                    checkpoint.timestamp.to_rfc3339(),
                ],
            )
            .map_err(|e| RuntimeStateError::Io(format!("failed to save checkpoint: {}", e)))?;
            Ok(())
        })
    }

    fn clear_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.clear_conversation_sync(conversation_id) })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_runtime_state_store() {
        let tmp = std::env::temp_dir().join(format!("echo-state-test-{}", std::process::id()));
        let store = SqliteRuntimeStateStore::new(&tmp).unwrap();

        // Save a node
        let node = TaskNode::new("node-1", "Plan task")
            .with_status(TaskNodeStatus::Running)
            .with_dependencies(vec!["dep-1".to_string()]);
        store.save_node("conv-1", &node).await.unwrap();

        // Load nodes
        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "node-1");
        assert!(matches!(nodes[0].status, TaskNodeStatus::Running));

        // Update status
        store
            .update_status("conv-1", "node-1", TaskNodeStatus::Success)
            .await
            .unwrap();
        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert!(matches!(nodes[0].status, TaskNodeStatus::Success));

        // Save checkpoint
        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await.unwrap();

        let cp = store.get_checkpoint("conv-1").await.unwrap();
        assert!(cp.is_some());
        let cp = cp.unwrap();
        assert_eq!(cp.active_skills, vec!["coding"]);

        // Clear conversation
        store.clear_conversation("conv-1").await.unwrap();
        let nodes = store.load_nodes("conv-1").await.unwrap();
        assert!(nodes.is_empty());
        let cp = store.get_checkpoint("conv-1").await.unwrap();
        assert!(cp.is_none());

        let _ = std::fs::remove_file(&tmp);
    }

    #[tokio::test]
    async fn test_blocked_to_hydrated_state_transition() {
        let tmp =
            std::env::temp_dir().join(format!("echo-state-blocked-test-{}", std::process::id()));
        let store = SqliteRuntimeStateStore::new(&tmp).unwrap();
        let conv_id = "conv-blocked-test";

        // 1. Create a node and start it
        let node = TaskNode::new("task-1", "Data processing");
        store.save_node(conv_id, &node).await.unwrap();
        store
            .update_status(conv_id, "task-1", TaskNodeStatus::Running)
            .await
            .unwrap();

        // 2. Node hits a blocker (e.g., waiting for human approval)
        store
            .update_status(
                conv_id,
                "task-1",
                TaskNodeStatus::Blocked {
                    reason: "Waiting for API key approval".to_string(),
                },
            )
            .await
            .unwrap();

        // Verify blocked state
        let nodes = store.load_nodes(conv_id).await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].status.is_blocked());
        if let TaskNodeStatus::Blocked { reason } = &nodes[0].status {
            assert_eq!(reason, "Waiting for API key approval");
        } else {
            panic!("Expected Blocked status");
        }

        // 3. Save checkpoint with blocked reason
        let checkpoint = AgentCheckpoint {
            conversation_id: conv_id.to_string(),
            messages_json: r#"[{"role":"user","content":"process data"}]"#.to_string(),
            current_plan: Some("Step 1: Get API key, Step 2: Process data".to_string()),
            active_skills: vec!["data-wrangling".to_string()],
            blocked_reason: Some("Waiting for API key approval".to_string()),
            working_dir: None,
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await.unwrap();

        // 4. Resume: load checkpoint and transition to Hydrated
        let loaded_cp = store
            .get_checkpoint(conv_id)
            .await
            .unwrap()
            .expect("checkpoint should exist");
        assert_eq!(
            loaded_cp.blocked_reason.as_deref(),
            Some("Waiting for API key approval")
        );

        // Transition from Blocked to Hydrated
        store
            .update_status(conv_id, "task-1", TaskNodeStatus::Hydrated)
            .await
            .unwrap();

        // Verify hydrated state
        let nodes = store.load_nodes(conv_id).await.unwrap();
        assert!(
            matches!(nodes[0].status, TaskNodeStatus::Hydrated),
            "Expected Hydrated status, got {:?}",
            nodes[0].status
        );
        assert!(!nodes[0].status.is_blocked());
        assert!(!nodes[0].status.is_terminal());

        // 5. Continue execution: Hydrated → Running → Success
        store
            .update_status(conv_id, "task-1", TaskNodeStatus::Running)
            .await
            .unwrap();
        store
            .update_status(conv_id, "task-1", TaskNodeStatus::Success)
            .await
            .unwrap();

        let nodes = store.load_nodes(conv_id).await.unwrap();
        assert!(nodes[0].status.is_terminal());
        assert!(matches!(nodes[0].status, TaskNodeStatus::Success));

        // Verify the full state machine path was valid:
        // Pending → Running → Blocked → Hydrated → Running → Success
        // (each transition was accepted by the store without error)

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_task_node_status_properties() {
        // is_terminal
        assert!(TaskNodeStatus::Success.is_terminal());
        assert!(TaskNodeStatus::Failed.is_terminal());
        assert!(!TaskNodeStatus::Pending.is_terminal());
        assert!(!TaskNodeStatus::Running.is_terminal());
        assert!(!TaskNodeStatus::Hydrated.is_terminal());
        assert!(
            !TaskNodeStatus::Blocked {
                reason: "test".into()
            }
            .is_terminal()
        );

        // is_blocked
        assert!(
            TaskNodeStatus::Blocked {
                reason: "test".into()
            }
            .is_blocked()
        );
        assert!(!TaskNodeStatus::Running.is_blocked());
        assert!(!TaskNodeStatus::Hydrated.is_blocked());
    }
}
