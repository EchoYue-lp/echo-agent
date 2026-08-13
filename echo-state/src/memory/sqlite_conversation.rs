//! SQLite conversation persistence implementation
//!
//! Production-grade conversation storage backed by SQLite, with cascading deletes and efficient queries.

use crate::util::{expand_tilde, memory_io_error};
use echo_core::error::Result;
pub use echo_core::memory::conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
use futures::future::BoxFuture;
use rusqlite::{Connection, TransactionBehavior, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

// ── SqliteConversationStore ─────────────────────────────────────────────────

/// SQLite conversation persistence Store
pub struct SqliteConversationStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteConversationStore {
    /// Open or create the SQLite database, auto-creating tables
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| memory_io_error("failed to create directory", e))?;
        }

        let conn = Connection::open(&path)
            .map_err(|e| memory_io_error("failed to open SQLite database", e))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=5000;
             PRAGMA temp_store=MEMORY;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(|e| memory_io_error("SQLite PRAGMA configuration failed", e))?;

        Self::init_tables(&conn)?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM conversation", [], |row| row.get(0))
            .map_err(|e| memory_io_error("failed to count conversations", e))?;

        info!(
            path = %path.display(),
            conversations = count,
            "SqliteConversationStore initialized"
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn run_db<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|error| {
                echo_core::error::MemoryError::IoError(format!(
                    "SqliteConversationStore lock poisoned: {error}"
                ))
            })?;
            operation(&mut guard)
        })
        .await
        .map_err(|error| {
            echo_core::error::MemoryError::IoError(format!(
                "SQLite conversation operation task failed: {error}"
            ))
        })?
    }

    fn init_tables(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversation (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id     TEXT NOT NULL UNIQUE,
                user_id             TEXT NOT NULL DEFAULT 'default',
                agent_type          TEXT,
                title               TEXT,
                summary             TEXT,
                compressed_before_id INTEGER,
                created_at          TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                updated_at          TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );
            CREATE INDEX IF NOT EXISTS idx_conv_user ON conversation(user_id);
            CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversation(updated_at DESC);

            CREATE TABLE IF NOT EXISTS message (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id     TEXT NOT NULL REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                role                TEXT NOT NULL,
                content             TEXT,
                attachments_json    TEXT,
                tool_calls_json     TEXT,
                tool_result_json    TEXT,
                created_at          TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );
            CREATE INDEX IF NOT EXISTS idx_msg_conv ON message(conversation_id);",
        )
        .map_err(|e| memory_io_error("failed to create tables", e))?;
        Ok(())
    }
}

// ── Trait implementation ───────────────────────────────────────────────────

impl ConversationStore for SqliteConversationStore {
    fn create_conversation<'a>(
        &'a self,
        conv: NewConversation,
    ) -> BoxFuture<'a, Result<Conversation>> {
        Box::pin(async move {
            self.run_db(move |conn| {
                conn.execute(
                    "INSERT INTO conversation (conversation_id, user_id, agent_type, title)
                 VALUES (?1, ?2, ?3, ?4)",
                    params![
                        conv.conversation_id,
                        conv.user_id,
                        conv.agent_type,
                        conv.title
                    ],
                )
                .map_err(|e| memory_io_error("failed to insert conversation", e))?;

                let id = conn.last_insert_rowid();
                let row = conn
                    .query_row(
                        "SELECT id, conversation_id, user_id, agent_type, title, summary,
                        compressed_before_id, created_at, updated_at
                 FROM conversation WHERE id = ?1",
                        params![id],
                        |row| {
                            Ok(Conversation {
                                id: row.get(0)?,
                                conversation_id: row.get(1)?,
                                user_id: row.get(2)?,
                                agent_type: row.get(3)?,
                                title: row.get(4)?,
                                summary: row.get(5)?,
                                compressed_before_id: row.get(6)?,
                                created_at: row.get(7)?,
                                updated_at: row.get(8)?,
                            })
                        },
                    )
                    .map_err(|e| memory_io_error("failed to query new conversation", e))?;

                Ok(row)
            })
            .await
        })
    }

    fn get_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Conversation>>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                let result = conn.query_row(
                    "SELECT id, conversation_id, user_id, agent_type, title, summary,
                        compressed_before_id, created_at, updated_at
                 FROM conversation WHERE conversation_id = ?1",
                    params![conversation_id],
                    |row| {
                        Ok(Conversation {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            user_id: row.get(2)?,
                            agent_type: row.get(3)?,
                            title: row.get(4)?,
                            summary: row.get(5)?,
                            compressed_before_id: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    },
                );

                match result {
                    Ok(conv) => Ok(Some(conv)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(memory_io_error("failed to query conversation", e).into()),
                }
            })
            .await
        })
    }

    fn list_conversations<'a>(
        &'a self,
        filter: ConversationFilter,
    ) -> BoxFuture<'a, Result<Vec<ConversationMeta>>> {
        Box::pin(async move {
            self.run_db(move |conn| {
            let mut sql = String::from(
                "SELECT c.id, c.conversation_id, c.user_id, c.title, c.created_at, c.updated_at,
                        (SELECT COUNT(*) FROM message m WHERE m.conversation_id = c.conversation_id) AS msg_count
                 FROM conversation c WHERE 1=1",
            );
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            let mut param_idx = 1;

            if let Some(user_id) = filter.user_id {
                sql.push_str(&format!(" AND c.user_id = ?{param_idx}"));
                param_values.push(Box::new(user_id));
                param_idx += 1;
            }
            if let Some(agent_type) = filter.agent_type {
                sql.push_str(&format!(" AND c.agent_type = ?{param_idx}"));
                param_values.push(Box::new(agent_type));
                param_idx += 1;
            }

            sql.push_str(" ORDER BY c.updated_at DESC");

            if let Some(limit) = filter.limit {
                sql.push_str(&format!(" LIMIT ?{param_idx}"));
                param_values.push(Box::new(i64::try_from(limit).map_err(|_| {
                    echo_core::error::MemoryError::SerializationError(
                        "conversation limit exceeds SQLite range".to_string(),
                    )
                })?));
                param_idx += 1;
            }
            if let Some(offset) = filter.offset {
                sql.push_str(&format!(" OFFSET ?{param_idx}"));
                param_values.push(Box::new(i64::try_from(offset).map_err(|_| {
                    echo_core::error::MemoryError::SerializationError(
                        "conversation offset exceeds SQLite range".to_string(),
                    )
                })?));
            }

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| memory_io_error("failed to prepare query", e))?;

            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(ConversationMeta {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        user_id: row.get(2)?,
                        title: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                        message_count: usize::try_from(row.get::<_, i64>(6)?)
                            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(6, i64::MIN))?,
                    })
                })
                .map_err(|e| memory_io_error("failed to query conversation list", e))?;

            let mut result = Vec::new();
            for row in rows {
                let meta = row.map_err(|e| memory_io_error("failed to read row", e))?;
                result.push(meta);
            }
            Ok(result)
            }).await
        })
    }

    fn update_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
        title: Option<&'a str>,
        summary: Option<&'a str>,
        compressed_before_id: Option<i64>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            let title = title.map(str::to_string);
            let summary = summary.map(str::to_string);
            self.run_db(move |conn| {
                // Build dynamic UPDATE — only update fields that are Some
                let mut sets = Vec::new();
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

                if let Some(t) = title {
                    sets.push(format!("title = ?{}", params.len() + 1));
                    params.push(Box::new(t));
                }
                if let Some(s) = summary {
                    sets.push(format!("summary = ?{}", params.len() + 1));
                    params.push(Box::new(s));
                }
                if let Some(cbid) = compressed_before_id {
                    sets.push(format!("compressed_before_id = ?{}", params.len() + 1));
                    params.push(Box::new(cbid));
                }

                if sets.is_empty() {
                    return Ok(());
                }

                sets.push("updated_at = datetime('now', 'localtime')".to_string());
                params.push(Box::new(conversation_id));

                let sql = format!(
                    "UPDATE conversation SET {} WHERE conversation_id = ?{}",
                    sets.join(", "),
                    params.len()
                );

                let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();

                conn.execute(&sql, params_refs.as_slice())
                    .map_err(|e| memory_io_error("failed to update conversation", e))?;

                Ok(())
            })
            .await
        })
    }

    fn delete_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                conn.execute(
                    "DELETE FROM conversation WHERE conversation_id = ?1",
                    params![conversation_id],
                )
                .map_err(|e| memory_io_error("failed to delete conversation", e))?;
                Ok(())
            })
            .await
        })
    }

    fn save_messages<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [StoredMessage],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            let messages = messages.to_vec();
            self.run_db(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| memory_io_error("failed to begin transaction", e))?;
            tx.execute(
                "DELETE FROM message WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|e| memory_io_error("failed to clear old messages", e))?;

            for msg in messages {
                if let Some(id) = msg.id {
                    tx.execute(
                        "INSERT INTO message (id, conversation_id, role, content, attachments_json, tool_calls_json, tool_result_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            id,
                            conversation_id,
                            msg.role,
                            msg.content,
                            msg.attachments_json,
                            msg.tool_calls_json,
                            msg.tool_result_json,
                            msg.created_at,
                        ],
                    )
                    .map_err(|e| memory_io_error("failed to insert identified message", e))?;
                } else {
                    tx.execute(
                        "INSERT INTO message (conversation_id, role, content, attachments_json, tool_calls_json, tool_result_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            conversation_id,
                            msg.role,
                            msg.content,
                            msg.attachments_json,
                            msg.tool_calls_json,
                            msg.tool_result_json,
                            msg.created_at,
                        ],
                    )
                    .map_err(|e| memory_io_error("failed to insert message", e))?;
                }
            }

            tx.execute(
                "UPDATE conversation
                 SET updated_at = datetime('now', 'localtime'),
                     compressed_before_id = CASE
                         WHEN compressed_before_id IS NULL THEN NULL
                         WHEN EXISTS (
                             SELECT 1 FROM message
                             WHERE conversation_id = ?1 AND id = compressed_before_id
                         ) THEN compressed_before_id
                         ELSE NULL
                     END
                 WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|e| memory_io_error("failed to update conversation timestamp", e))?;

            tx.commit()
                .map_err(|e| memory_io_error("failed to commit transaction", e))?;

            Ok(())
            }).await
        })
    }

    fn get_messages<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<StoredMessage>>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, conversation_id, role, content, attachments_json,
                            tool_calls_json, tool_result_json, created_at
                     FROM message WHERE conversation_id = ?1 ORDER BY id ASC",
                    )
                    .map_err(|e| memory_io_error("failed to prepare query", e))?;

                let rows = stmt
                    .query_map(params![conversation_id], |row| {
                        Ok(StoredMessage {
                            id: Some(row.get(0)?),
                            conversation_id: row.get(1)?,
                            role: row.get(2)?,
                            content: row.get(3)?,
                            attachments_json: row.get(4)?,
                            tool_calls_json: row.get(5)?,
                            tool_result_json: row.get(6)?,
                            created_at: row.get(7)?,
                        })
                    })
                    .map_err(|e| memory_io_error("failed to query messages", e))?;

                let mut result = Vec::new();
                for row in rows {
                    result.push(row.map_err(|e| memory_io_error("failed to read message", e))?);
                }
                Ok(result)
            })
            .await
        })
    }

    fn count_messages<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM message WHERE conversation_id = ?1",
                        params![conversation_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| memory_io_error("failed to count messages", e))?;
                usize::try_from(count).map_err(|_| {
                    echo_core::error::MemoryError::SerializationError(
                        "negative message count".to_string(),
                    )
                    .into()
                })
            })
            .await
        })
    }

    fn search_conversations<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ConversationMeta>>> {
        Box::pin(async move {
            let pattern = format!("%{}%", query);
            self.run_db(move |conn| {

            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT c.id, c.conversation_id, c.user_id, c.title,
                            c.created_at, c.updated_at,
                            (SELECT COUNT(*) FROM message WHERE conversation_id = c.conversation_id) AS msg_count
                     FROM conversation c
                     LEFT JOIN message m ON c.conversation_id = m.conversation_id
                     WHERE c.title LIKE ?1 OR m.content LIKE ?1
                     ORDER BY c.updated_at DESC
                     LIMIT ?2",
                )
                .map_err(|e| memory_io_error("failed to prepare search query", e))?;

            let limit = i64::try_from(limit).map_err(|_| {
                echo_core::error::MemoryError::SerializationError(
                    "conversation search limit exceeds SQLite range".to_string(),
                )
            })?;
            let rows = stmt
                .query_map(params![pattern, limit], |row| {
                    Ok(ConversationMeta {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        user_id: row.get(2)?,
                        title: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                        message_count: usize::try_from(row.get::<_, i64>(6)?)
                            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(6, i64::MIN))?,
                    })
                })
                .map_err(|e| memory_io_error("failed to search conversations", e))?;

            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| memory_io_error("failed to read search row", e))?);
            }
            Ok(result)
            }).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_message(conversation_id: &str, id: Option<i64>, content: &str) -> StoredMessage {
        StoredMessage {
            id,
            conversation_id: conversation_id.to_string(),
            role: "user".to_string(),
            content: Some(content.to_string()),
            attachments_json: None,
            tool_calls_json: None,
            tool_result_json: None,
            created_at: "2026-08-13T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn replace_preserves_explicit_ids_and_clears_stale_boundary() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .map_err(|error| memory_io_error("failed to create test directory", error))?;
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        let conversation_id = "conversation-1";
        store
            .create_conversation(NewConversation {
                conversation_id: conversation_id.to_string(),
                user_id: "local".to_string(),
                agent_type: None,
                title: None,
            })
            .await?;
        store
            .save_messages(
                conversation_id,
                &[stored_message(conversation_id, Some(41), "one")],
            )
            .await?;
        store
            .update_conversation(conversation_id, None, Some("summary"), Some(41))
            .await?;

        store
            .save_messages(
                conversation_id,
                &[stored_message(conversation_id, Some(42), "two")],
            )
            .await?;

        let messages = store.get_messages(conversation_id).await?;
        assert_eq!(messages.first().and_then(|message| message.id), Some(42));
        let conversation = store.get_conversation(conversation_id).await?;
        assert!(conversation.is_some_and(|value| value.compressed_before_id.is_none()));
        Ok(())
    }
}
