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
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;
use tracing::info;

// ── SqliteConversationStore ─────────────────────────────────────────────────

/// SQLite conversation persistence Store
pub struct SqliteConversationStore {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    path: PathBuf,
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
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| memory_io_error("SQLite PRAGMA configuration failed", e))?;

        Self::init_tables(&conn)?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM conversation", [], |row| row.get(0))
            .unwrap_or(0);

        info!(
            path = %path.display(),
            conversations = count,
            "SqliteConversationStore initialized"
        );

        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
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
                created_at          TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
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
                created_at          TEXT NOT NULL DEFAULT (datetime('now'))
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
            let conn = self.conn.lock().await;
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
    }

    fn get_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Conversation>>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
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
    }

    fn list_conversations<'a>(
        &'a self,
        filter: ConversationFilter,
    ) -> BoxFuture<'a, Result<Vec<ConversationMeta>>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;

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
                param_values.push(Box::new(limit as i64));
                param_idx += 1;
            }
            if let Some(offset) = filter.offset {
                sql.push_str(&format!(" OFFSET ?{param_idx}"));
                param_values.push(Box::new(offset as i64));
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
                        message_count: row.get::<_, i64>(6).unwrap_or(0) as usize,
                    })
                })
                .map_err(|e| memory_io_error("failed to query conversation list", e))?;

            let mut result = Vec::new();
            for row in rows {
                let meta = row.map_err(|e| memory_io_error("failed to read row", e))?;
                result.push(meta);
            }
            Ok(result)
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
            let conn = self.conn.lock().await;

            // Build dynamic UPDATE — only update fields that are Some
            let mut sets = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(t) = title {
                sets.push(format!("title = ?{}", params.len() + 1));
                params.push(Box::new(t.to_string()));
            }
            if let Some(s) = summary {
                sets.push(format!("summary = ?{}", params.len() + 1));
                params.push(Box::new(s.to_string()));
            }
            if let Some(cbid) = compressed_before_id {
                sets.push(format!("compressed_before_id = ?{}", params.len() + 1));
                params.push(Box::new(cbid));
            }

            if sets.is_empty() {
                return Ok(());
            }

            sets.push("updated_at = datetime('now')".to_string());
            params.push(Box::new(conversation_id.to_string()));

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
    }

    fn delete_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            conn.execute(
                "DELETE FROM conversation WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|e| memory_io_error("failed to delete conversation", e))?;
            Ok(())
        })
    }

    fn save_messages<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [StoredMessage],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;

            // Delete existing messages (upsert pattern: replace all)
            conn.execute(
                "DELETE FROM message WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|e| memory_io_error("failed to clear old messages", e))?;

            // Insert new messages
            for msg in messages {
                conn.execute(
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

            // Update conversation timestamp
            conn.execute(
                "UPDATE conversation SET updated_at = datetime('now') WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|e| memory_io_error("failed to update conversation timestamp", e))?;

            Ok(())
        })
    }

    fn get_messages<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<StoredMessage>>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
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
    }

    fn count_messages<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM message WHERE conversation_id = ?1",
                    params![conversation_id],
                    |row| row.get(0),
                )
                .map_err(|e| memory_io_error("failed to count messages", e))?;
            Ok(count as usize)
        })
    }
}
