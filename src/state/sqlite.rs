//! SQLite-backed [`RuntimeStateStore`] implementation.

use super::{
    AgentCheckpoint, RuntimeStateClearReceipt, RuntimeStateScopeClearReceipt, RuntimeStateStore,
};
use crate::error::{Result, RuntimeStateError};
use echo_core::utils::blocking::{
    BlockingFileOperationKey, BlockingFileOperationScope, run_keyed_file_operation,
};
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use std::path::Path;

/// SQLite-backed runtime checkpoint store.
#[derive(Clone)]
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
        let mut store = Self { path };
        store.init_tables()?;
        store.path = std::fs::canonicalize(&store.path).map_err(|error| {
            RuntimeStateError::Io(format!("failed to canonicalize SQLite store: {error}"))
        })?;
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
            CREATE TABLE IF NOT EXISTS runtime_state_scopes (
                scope_id         TEXT NOT NULL,
                runtime_state_id TEXT NOT NULL,
                PRIMARY KEY (scope_id, runtime_state_id)
            );
            CREATE INDEX IF NOT EXISTS idx_runtime_state_scopes_runtime
                ON runtime_state_scopes(runtime_state_id);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_runtime_state_scope_owner
                ON runtime_state_scopes(runtime_state_id);
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
        let mut conn = self.open_conn()?;
        let transaction = conn.transaction().map_err(|error| {
            RuntimeStateError::Io(format!("failed to begin checkpoint clear: {error}"))
        })?;
        let owner = transaction
            .query_row(
                "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to inspect checkpoint owner: {error}"))
            })?;
        if let Some(owner) = owner.as_deref()
            && owner != conversation_id
        {
            return Err(RuntimeStateError::Io(format!(
                "runtime state {conversation_id} belongs to scope {owner}, not {conversation_id}"
            ))
            .into());
        }
        transaction
            .execute(
                "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to clear checkpoint: {error}"))
            })?;
        transaction
            .execute(
                "DELETE FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                params![conversation_id],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to clear checkpoint scope binding: {error}"))
            })?;
        transaction.commit().map_err(|error| {
            RuntimeStateError::Io(format!("failed to commit checkpoint clear: {error}"))
        })?;
        Ok(())
    }

    fn save_checkpoint_on_connection(
        conn: &Connection,
        checkpoint: &AgentCheckpoint,
    ) -> Result<()> {
        let active_skills = serde_json::to_string(&checkpoint.active_skills).map_err(|error| {
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
    }

    fn run_blocking<'a, T, F>(
        &'a self,
        scope: BlockingFileOperationScope,
        operation: F,
    ) -> BoxFuture<'a, Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self) -> Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            let key =
                BlockingFileOperationKey::new("runtime-state-sqlite", store.path.clone(), scope);
            run_keyed_file_operation(key, move || operation(store))
                .await
                .map_err(|error| RuntimeStateError::Io(error.to_string()))?
        })
    }

    fn entity_scope(value: &str) -> BlockingFileOperationScope {
        BlockingFileOperationScope::Entity(echo_core::utils::fs::encode_utf8_path_identity(value))
    }

    fn collection_scope(value: &str) -> BlockingFileOperationScope {
        BlockingFileOperationScope::Collection(echo_core::utils::fs::encode_utf8_path_identity(
            value,
        ))
    }
}

impl RuntimeStateStore for SqliteRuntimeStateStore {
    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<AgentCheckpoint>>> {
        let conversation_id = conversation_id.to_string();
        self.run_blocking(Self::entity_scope(&conversation_id), move |store| {
            let conn = store.open_conn()?;
            let mut statement = conn
                .prepare(
                    "SELECT messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp
                     FROM agent_checkpoints WHERE conversation_id = ?1",
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to prepare query: {error}"))
                })?;

            let row = statement.query_row(params![&conversation_id], |row| {
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
                conversation_id,
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
        self.save_checkpoint_for_scope(&checkpoint.conversation_id, checkpoint)
    }

    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope_id: &'a str,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, Result<()>> {
        let scope_id = scope_id.to_string();
        let checkpoint = checkpoint.clone();
        self.run_blocking(
            Self::entity_scope(&checkpoint.conversation_id),
            move |store| {
                let mut conn = store.open_conn()?;
                let transaction = conn.transaction().map_err(|error| {
                    RuntimeStateError::Io(format!(
                        "failed to begin checkpoint transaction: {error}"
                    ))
                })?;
                transaction
                .execute(
                    "INSERT INTO runtime_state_scopes (scope_id, runtime_state_id) VALUES (?1, ?2)
                     ON CONFLICT(scope_id, runtime_state_id) DO NOTHING",
                    params![&scope_id, &checkpoint.conversation_id],
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to bind checkpoint scope: {error}"))
                })?;
                Self::save_checkpoint_on_connection(&transaction, &checkpoint)?;
                transaction.commit().map_err(|error| {
                    RuntimeStateError::Io(format!(
                        "failed to commit checkpoint transaction: {error}"
                    ))
                })?;
                Ok(())
            },
        )
    }

    fn runtime_state_ids<'a>(&'a self, scope_id: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        let scope_id = scope_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let conn = store.open_conn()?;
            let mut statement = conn
                .prepare(
                    "SELECT runtime_state_id FROM runtime_state_scopes WHERE scope_id = ?1 ORDER BY runtime_state_id",
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to prepare scope query: {error}"))
                })?;
            let rows = statement
                .query_map(params![&scope_id], |row| row.get::<_, String>(0))
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to query checkpoint scope: {error}"))
                })?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(|error| {
                    RuntimeStateError::Io(format!("failed to read checkpoint scope: {error}"))
                })?);
            }
            Ok(ids)
        })
    }

    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, Result<RuntimeStateClearReceipt>> {
        let scope_id = scope_id.to_string();
        let runtime_state_id = runtime_state_id.to_string();
        self.run_blocking(Self::entity_scope(&runtime_state_id), move |store| {
            let mut conn = store.open_conn()?;
            let transaction = conn.transaction().map_err(|error| {
                RuntimeStateError::Io(format!("failed to begin runtime clear: {error}"))
            })?;
            let owner = transaction
                .query_row(
                    "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                    params![&runtime_state_id],
                    |row| row.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect runtime scope: {error}"))
                })?;
            if let Some(owner) = owner.as_deref()
                && owner != scope_id
            {
                return Err(RuntimeStateError::Io(format!(
                    "runtime state {runtime_state_id} belongs to scope {owner}, not {scope_id}"
                ))
                .into());
            }
            let indexed = owner.as_deref() == Some(scope_id.as_str());
            let legacy = owner.is_none() && scope_id == runtime_state_id;
            let checkpoint_removed = if indexed || legacy {
                transaction
                    .execute(
                        "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                        params![&runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete checkpoint: {error}"))
                    })?
                    != 0
            } else {
                false
            };
            if indexed {
                transaction
                    .execute(
                        "DELETE FROM runtime_state_scopes WHERE scope_id = ?1 AND runtime_state_id = ?2",
                        params![&scope_id, &runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete scope binding: {error}"))
                    })?;
            }
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime clear: {error}"))
            })?;
            Ok(RuntimeStateClearReceipt {
                scope_id,
                runtime_state_id,
                checkpoint_removed,
            })
        })
    }

    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, Result<RuntimeStateScopeClearReceipt>> {
        let scope_id = scope_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let mut conn = store.open_conn()?;
            let transaction = conn.transaction().map_err(|error| {
                RuntimeStateError::Io(format!("failed to begin scope clear: {error}"))
            })?;
            let mut runtime_state_ids = {
                let mut statement = transaction
                    .prepare(
                        "SELECT runtime_state_id FROM runtime_state_scopes WHERE scope_id = ?1 ORDER BY runtime_state_id",
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to prepare scope clear: {error}"))
                    })?;
                let rows = statement
                    .query_map(params![&scope_id], |row| row.get::<_, String>(0))
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to query scope clear: {error}"))
                    })?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(|error| {
                        RuntimeStateError::Io(format!("failed to read scope clear: {error}"))
                    })?);
                }
                ids
            };
            let legacy_owner = transaction
                .query_row(
                    "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                    params![&scope_id],
                    |row| row.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect legacy owner: {error}"))
                })?;
            // A foreign scope may legitimately own a runtime ID equal to this
            // scope's name. That only disables the legacy same-ID fallback; it
            // must not block deletion of rows actually owned by `scope_id`.
            let legacy_exists = legacy_owner.is_none()
                && transaction
                .query_row(
                    "SELECT 1 FROM agent_checkpoints WHERE conversation_id = ?1",
                    params![&scope_id],
                    |_row| Ok(()),
                )
                .map(|()| true)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(false),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect legacy checkpoint: {error}"))
                })?;
            if legacy_exists
                && !runtime_state_ids
                    .iter()
                    .any(|runtime_id| runtime_id == &scope_id)
            {
                runtime_state_ids.push(scope_id.to_string());
                runtime_state_ids.sort();
            }
            for runtime_state_id in &runtime_state_ids {
                transaction
                    .execute(
                        "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                        params![runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete scope checkpoint: {error}"))
                    })?;
            }
            transaction
                .execute(
                    "DELETE FROM runtime_state_scopes WHERE scope_id = ?1",
                    params![&scope_id],
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to delete runtime scope: {error}"))
                })?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit scope clear: {error}"))
            })?;
            Ok(RuntimeStateScopeClearReceipt {
                scope_id,
                runtime_state_ids,
            })
        })
    }

    fn clear_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.clear_runtime_state(conversation_id, conversation_id)
                .await
                .map(|_receipt| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::time::Duration;

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
    async fn sqlite_scope_index_survives_restart_and_clears_exactly() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-scope-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let checkpoint = |runtime_state_id: &str| AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: "[]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        let store = SqliteRuntimeStateStore::new(&path)?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1"))
            .await?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2"))
            .await?;
        store
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1"))
            .await?;
        assert!(
            store
                .save_checkpoint_for_scope("bob", &checkpoint("alice-2"))
                .await
                .is_err()
        );
        drop(store);

        let restarted = SqliteRuntimeStateStore::new(&path)?;
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-1".to_string(), "alice-2".to_string()]
        );
        assert!(
            restarted
                .clear_runtime_state("alice-2", "alice-2")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state_scope("alice-2")
                .await?
                .runtime_state_ids
                .is_empty()
        );
        assert!(restarted.clear_conversation("alice-2").await.is_err());
        assert!(restarted.clear_conversation_sync("alice-2").is_err());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert!(
            restarted
                .clear_runtime_state("alice", "alice-1")
                .await?
                .checkpoint_removed
        );
        assert!(restarted.get_checkpoint("alice-1").await?.is_none());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("scope-a-1"))
            .await?;
        restarted
            .save_checkpoint_for_scope("scope-b", &checkpoint("scope-a"))
            .await?;
        let same_name = restarted.clear_runtime_state_scope("scope-a").await?;
        assert_eq!(same_name.runtime_state_ids, vec!["scope-a-1".to_string()]);
        assert!(restarted.get_checkpoint("scope-a-1").await?.is_none());
        assert!(restarted.get_checkpoint("scope-a").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("scope-b").await?,
            vec!["scope-a".to_string()]
        );
        let cleared = restarted.clear_runtime_state_scope("alice").await?;
        assert_eq!(cleared.runtime_state_ids, vec!["alice-2".to_string()]);
        assert!(restarted.get_checkpoint("alice-2").await?.is_none());
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
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

    #[tokio::test(flavor = "current_thread")]
    async fn sqlite_blocking_owner_preserves_runtime_heartbeat() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-blocking-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let operation_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = tokio::spawn(async move {
            operation_store
                .run_blocking(
                    SqliteRuntimeStateStore::entity_scope("heartbeat"),
                    move |_store| {
                        let _ignored = entered_tx.send(());
                        release_rx
                            .recv_timeout(Duration::from_secs(2))
                            .map_err(|error| {
                                RuntimeStateError::Io(format!(
                                    "blocking test release failed: {error}"
                                ))
                                .into()
                            })
                    },
                )
                .await
        });
        entered_rx.await.map_err(|error| {
            RuntimeStateError::Io(format!("blocking test did not start: {error}"))
        })?;
        tokio::time::timeout(Duration::from_millis(250), async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| RuntimeStateError::Io("SQLite stalled the Tokio runtime".to_string()))?;
        release_tx.send(()).map_err(|error| {
            RuntimeStateError::Io(format!("blocking test release failed: {error}"))
        })?;
        operation.await.map_err(|error| {
            RuntimeStateError::Io(format!("blocking test join failed: {error}"))
        })??;
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
