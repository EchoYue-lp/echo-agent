//! SQLite-backed [`RuntimeStateStore`] implementation.

use super::{AgentCheckpoint, RuntimeStateStore};
use crate::error::{Result, RuntimeStateError};
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use std::path::Path;

/// SQLite-backed runtime checkpoint store.
pub struct SqliteRuntimeStateStore {
    path: std::path::PathBuf,
}

impl SqliteRuntimeStateStore {
    /// Create a SQLite state store at `path`.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                RuntimeStateError::Io(format!("failed to create directory: {error}"))
            })?;
        }
        let store = Self { path };
        store.init_tables()?;
        Ok(store)
    }

    fn open_conn(&self) -> Result<Connection> {
        Connection::open(&self.path).map_err(|error| {
            RuntimeStateError::Io(format!("failed to open SQLite connection: {error}")).into()
        })
    }

    /// Initialize the checkpoint table.
    pub fn init_tables(&self) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS agent_checkpoints (
                conversation_id TEXT PRIMARY KEY,
                messages_json   TEXT NOT NULL,
                current_plan    TEXT,
                active_skills   TEXT NOT NULL,
                blocked_reason  TEXT,
                working_dir     TEXT,
                timestamp       TEXT NOT NULL
            );
            "#,
        )
        .map_err(|error| RuntimeStateError::Io(format!("failed to init tables: {error}")))?;

        // Databases created before working-directory restoration lack this
        // column. SQLite has no `ADD COLUMN IF NOT EXISTS`, so only the exact
        // duplicate-column outcome is accepted.
        if let Err(error) = conn.execute(
            "ALTER TABLE agent_checkpoints ADD COLUMN working_dir TEXT",
            [],
        ) {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(RuntimeStateError::Io(format!(
                    "failed to add working_dir column: {message}"
                ))
                .into());
            }
        }
        Ok(())
    }

    /// Delete a conversation checkpoint synchronously.
    pub fn clear_conversation_sync(&self, conversation_id: &str) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute(
            "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|error| RuntimeStateError::Io(format!("failed to clear checkpoint: {error}")))?;
        Ok(())
    }
}

impl RuntimeStateStore for SqliteRuntimeStateStore {
    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<AgentCheckpoint>>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let mut statement = conn
                .prepare(
                    "SELECT messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp
                     FROM agent_checkpoints WHERE conversation_id = ?1",
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to prepare query: {error}"))
                })?;

            let row = statement.query_row(params![conversation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            });
            let (
                messages_json,
                current_plan,
                active_skills_json,
                blocked_reason,
                working_dir,
                timestamp,
            ) = match row {
                Ok(row) => row,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(error) => {
                    return Err(RuntimeStateError::Io(format!(
                        "failed to query checkpoint: {error}"
                    ))
                    .into());
                }
            };
            let active_skills = serde_json::from_str(&active_skills_json).map_err(|error| {
                RuntimeStateError::SerializationError(format!(
                    "invalid checkpoint active_skills: {error}"
                ))
            })?;
            let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
                .map_err(|error| {
                    RuntimeStateError::SerializationError(format!(
                        "invalid checkpoint timestamp: {error}"
                    ))
                })?
                .with_timezone(&chrono::Utc);

            Ok(Some(AgentCheckpoint {
                conversation_id: conversation_id.to_string(),
                messages_json,
                current_plan,
                active_skills,
                blocked_reason,
                working_dir: working_dir.map(std::path::PathBuf::from),
                timestamp,
            }))
        })
    }

    fn save_checkpoint<'a>(&'a self, checkpoint: &'a AgentCheckpoint) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.open_conn()?;
            let active_skills =
                serde_json::to_string(&checkpoint.active_skills).map_err(|error| {
                    RuntimeStateError::SerializationError(format!(
                        "failed to serialize checkpoint active_skills: {error}"
                    ))
                })?;
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
                    active_skills,
                    checkpoint.blocked_reason.as_deref(),
                    checkpoint.working_dir.as_ref().and_then(|path| path.to_str()),
                    crate::utils::time::to_local(checkpoint.timestamp).to_rfc3339(),
                ],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to save checkpoint: {error}"))
            })?;
            Ok(())
        })
    }

    fn clear_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.clear_conversation_sync(conversation_id) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn sqlite_runtime_checkpoint_lifecycle() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-test-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: Some(std::path::PathBuf::from("/tmp/work")),
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await?;

        let loaded = store
            .get_checkpoint("conv-1")
            .await?
            .ok_or_else(|| RuntimeStateError::Io("checkpoint missing after save".to_string()))?;
        assert_eq!(loaded.active_skills, vec!["coding"]);
        assert_eq!(loaded.working_dir, checkpoint.working_dir);

        store.clear_conversation("conv-1").await?;
        assert!(store.get_checkpoint("conv-1").await?.is_none());
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_checkpoint_is_rejected() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-corrupt-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let connection = store.open_conn()?;
        connection
            .execute(
                "INSERT INTO agent_checkpoints
                 (conversation_id, messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp)
                 VALUES (?1, ?2, NULL, ?3, NULL, NULL, ?4)",
                params!["corrupt", "[]", "not-json", "not-a-timestamp"],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        drop(connection);

        let error =
            store.get_checkpoint("corrupt").await.err().ok_or_else(|| {
                RuntimeStateError::Io("corrupt checkpoint was accepted".to_string())
            })?;
        assert!(error.to_string().contains("active_skills"));
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
