//! SQLite conversation persistence implementation
//!
//! Production-grade conversation storage backed by SQLite, with cascading deletes and efficient queries.

use crate::util::{expand_tilde, memory_io_error};
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationProjectionAuthority,
    ConversationProjectionCapability, ConversationProjectionEpochReceipt,
    ConversationProjectionEpochStatus, ConversationProjectionLifecycle, ConversationStore,
    EnsureConversationProjectionRequest, ManagedConversationDelete,
    ManagedConversationDeleteReceipt, ManagedConversationDeleteStatus, ManagedConversationImport,
    ManagedConversationImportLocator, ManagedConversationMetadataUpdate,
    ManagedConversationMetadataUpdateReceipt, ManagedConversationMetadataUpdateStatus,
    NewConversation, PersistenceCallCapability, PersistenceCallContext, StoredMessage,
    TranscriptProjectionApplyReceipt, TranscriptProjectionApplyStatus, TranscriptProjectionBatch,
    TranscriptProjectionConflictKind,
};
use futures::future::BoxFuture;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::info;

const MAX_PROJECTION_EPOCH: u64 = i64::MAX as u64;
const DELETE_RECEIPT_RETENTION: i64 = 32;

#[derive(Debug, Clone)]
struct SqliteProjectionState {
    epoch: u64,
    revision: u64,
    lifecycle: ConversationProjectionLifecycle,
    retention_floor_epoch: u64,
}

impl SqliteProjectionState {
    fn authority(&self, conversation_id: &str) -> ConversationProjectionAuthority {
        ConversationProjectionAuthority {
            conversation_id: conversation_id.to_string(),
            epoch: self.epoch,
            revision: self.revision,
            lifecycle: self.lifecycle,
            delete_receipt_retention_floor_epoch: self.retention_floor_epoch,
        }
    }

    fn next_revision(&self, conversation_id: &str) -> Result<u64> {
        self.revision.checked_add(1).ok_or_else(|| {
            MemoryError::Unsupported(format!(
                "conversation projection revision exhausted: {conversation_id}"
            ))
            .into()
        })
    }
}

// ── SqliteConversationStore ─────────────────────────────────────────────────

/// SQLite conversation persistence Store
pub struct SqliteConversationStore {
    conn: Arc<Mutex<Connection>>,
    persistence_call_context: Option<PersistenceCallContext>,
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
            persistence_call_context: None,
        })
    }

    fn with_persistence_call_context(&self, context: PersistenceCallContext) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
            persistence_call_context: Some(context),
        }
    }

    async fn run_db<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        let context = self.persistence_call_context;
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|error| {
                echo_core::error::MemoryError::IoError(format!(
                    "SqliteConversationStore lock poisoned: {error}"
                ))
            })?;
            if let Some(context) = context {
                context.ensure_not_expired()?;
            }
            operation(&mut guard)
        })
        .await
        .map_err(|error| {
            echo_core::error::MemoryError::IoError(format!(
                "SQLite conversation operation task failed: {error}"
            ))
        })?
    }

    fn projection_state(
        conn: &Connection,
        conversation_id: &str,
    ) -> Result<Option<SqliteProjectionState>> {
        let result = conn.query_row(
            "SELECT epoch, revision, lifecycle, retention_floor_epoch
             FROM conversation_projection WHERE conversation_id = ?1",
            params![conversation_id],
            |row| {
                let epoch = row.get::<_, i64>(0)?;
                let revision = row.get::<_, i64>(1)?;
                let lifecycle = row.get::<_, String>(2)?;
                let retention_floor_epoch = row.get::<_, i64>(3)?;
                Ok((epoch, revision, lifecycle, retention_floor_epoch))
            },
        );
        let (epoch, revision, lifecycle, retention_floor_epoch) = match result {
            Ok(value) => value,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => {
                return Err(
                    memory_io_error("failed to query conversation projection", error).into(),
                );
            }
        };
        let lifecycle = match lifecycle.as_str() {
            "live" => ConversationProjectionLifecycle::Live,
            "deleted" => ConversationProjectionLifecycle::Deleted,
            other => {
                return Err(MemoryError::SerializationError(format!(
                    "invalid conversation projection lifecycle: {other}"
                ))
                .into());
            }
        };
        let state = SqliteProjectionState {
            epoch: u64::try_from(epoch).map_err(|_| {
                MemoryError::SerializationError(
                    "conversation projection epoch is negative".to_string(),
                )
            })?,
            revision: u64::try_from(revision).map_err(|_| {
                MemoryError::SerializationError(
                    "conversation projection revision is negative".to_string(),
                )
            })?,
            lifecycle,
            retention_floor_epoch: u64::try_from(retention_floor_epoch).map_err(|_| {
                MemoryError::SerializationError(
                    "conversation projection retention floor is negative".to_string(),
                )
            })?,
        };
        if state.epoch == 0
            || state.epoch > MAX_PROJECTION_EPOCH
            || state.revision == 0
            || state.retention_floor_epoch > state.epoch
        {
            return Err(MemoryError::SerializationError(
                "conversation projection authority is outside its valid range".to_string(),
            )
            .into());
        }
        Ok(Some(state))
    }

    fn import_locator(
        conn: &Connection,
        conversation_id: &str,
    ) -> Result<Option<ManagedConversationImportLocator>> {
        let raw = conn
            .query_row(
                "SELECT locator_json FROM managed_import_locator WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| memory_io_error("failed to query managed import locator", error))?;
        raw.map(|raw| {
            let locator: ManagedConversationImportLocator = serde_json::from_str(&raw)?;
            if locator.conversation_id != conversation_id {
                return Err(MemoryError::SerializationError(
                    "managed import locator belongs to another conversation".to_string(),
                )
                .into());
            }
            Ok(locator)
        })
        .transpose()
    }

    fn sqlite_u64(value: u64, field: &str) -> Result<i64> {
        i64::try_from(value).map_err(|_| {
            MemoryError::Unsupported(format!(
                "conversation projection {field} exceeds SQLite range"
            ))
            .into()
        })
    }

    fn managed_conversation_error(conversation_id: &str) -> echo_core::error::ReactError {
        MemoryError::ManagedConversationRequiresProjection(conversation_id.to_string()).into()
    }

    fn replace_messages(
        tx: &Transaction<'_>,
        conversation_id: &str,
        messages: &[StoredMessage],
    ) -> Result<()> {
        tx.execute(
            "DELETE FROM message WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|error| memory_io_error("failed to clear managed messages", error))?;
        for message in messages {
            if let Some(id) = message.id {
                tx.execute(
                    "INSERT INTO message (id, conversation_id, role, content, attachments_json,
                        tool_calls_json, tool_result_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id,
                        conversation_id,
                        message.role,
                        message.content,
                        message.attachments_json,
                        message.tool_calls_json,
                        message.tool_result_json,
                        message.created_at,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to insert identified managed message", error)
                })?;
            } else {
                tx.execute(
                    "INSERT INTO message (conversation_id, role, content, attachments_json,
                        tool_calls_json, tool_result_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        conversation_id,
                        message.role,
                        message.content,
                        message.attachments_json,
                        message.tool_calls_json,
                        message.tool_result_json,
                        message.created_at,
                    ],
                )
                .map_err(|error| memory_io_error("failed to insert managed message", error))?;
            }
        }
        Ok(())
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
            CREATE INDEX IF NOT EXISTS idx_msg_conv ON message(conversation_id);

            CREATE TABLE IF NOT EXISTS conversation_projection (
                conversation_id       TEXT PRIMARY KEY REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                epoch                 INTEGER NOT NULL CHECK(epoch > 0),
                revision              INTEGER NOT NULL CHECK(revision > 0),
                lifecycle             TEXT NOT NULL CHECK(lifecycle IN ('live', 'deleted')),
                retention_floor_epoch INTEGER NOT NULL DEFAULT 0 CHECK(retention_floor_epoch >= 0)
            );

            CREATE TABLE IF NOT EXISTS transcript_projection_receipt (
                conversation_id TEXT NOT NULL REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                operation_id    TEXT NOT NULL,
                payload_digest  TEXT NOT NULL,
                PRIMARY KEY (conversation_id, operation_id)
            );

            CREATE TABLE IF NOT EXISTS transcript_projection_ordinal (
                conversation_id TEXT NOT NULL REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                generation_id   TEXT NOT NULL,
                ordinal         INTEGER NOT NULL CHECK(ordinal >= 0),
                digest          TEXT NOT NULL,
                PRIMARY KEY (conversation_id, generation_id, ordinal)
            );

            CREATE TABLE IF NOT EXISTS managed_import_locator (
                conversation_id TEXT PRIMARY KEY REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                locator_json    TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS managed_conversation_delete_receipt (
                conversation_id TEXT NOT NULL REFERENCES conversation(conversation_id) ON DELETE CASCADE,
                operation_id    TEXT NOT NULL,
                payload_digest  TEXT NOT NULL,
                deleted_epoch   INTEGER NOT NULL CHECK(deleted_epoch > 0),
                PRIMARY KEY (conversation_id, operation_id)
            );
            CREATE INDEX IF NOT EXISTS idx_managed_delete_epoch
                ON managed_conversation_delete_receipt(conversation_id, deleted_epoch);",
        )
        .map_err(|e| memory_io_error("failed to create tables", e))?;
        Ok(())
    }
}

// ── Trait implementation ───────────────────────────────────────────────────

impl ConversationStore for SqliteConversationStore {
    fn projection_capability(&self) -> ConversationProjectionCapability {
        ConversationProjectionCapability::AtomicV1
    }

    fn persistence_call_capability(&self) -> PersistenceCallCapability {
        PersistenceCallCapability::AbsoluteDeadlineV1
    }

    fn ensure_projection_epoch_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: EnsureConversationProjectionRequest,
    ) -> BoxFuture<'a, Result<ConversationProjectionEpochReceipt>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.ensure_projection_epoch(request).await
        })
    }

    fn get_projection_authority_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ConversationProjectionAuthority>>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.get_projection_authority(conversation_id).await
        })
    }

    fn apply_transcript_projection_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        batch: TranscriptProjectionBatch,
    ) -> BoxFuture<'a, Result<TranscriptProjectionApplyReceipt>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.apply_transcript_projection(batch).await
        })
    }

    fn import_managed_messages_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: ManagedConversationImport,
    ) -> BoxFuture<'a, Result<TranscriptProjectionApplyReceipt>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.import_managed_messages(request).await
        })
    }

    fn get_latest_managed_import_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ManagedConversationImportLocator>>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.get_latest_managed_import(conversation_id).await
        })
    }

    fn update_managed_conversation_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: ManagedConversationMetadataUpdate,
    ) -> BoxFuture<'a, Result<ManagedConversationMetadataUpdateReceipt>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.update_managed_conversation(request).await
        })
    }

    fn delete_managed_conversation_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: ManagedConversationDelete,
    ) -> BoxFuture<'a, Result<ManagedConversationDeleteReceipt>> {
        let store = self.with_persistence_call_context(context);
        Box::pin(async move {
            context.ensure_not_expired()?;
            store.delete_managed_conversation(request).await
        })
    }

    fn create_conversation<'a>(
        &'a self,
        conv: NewConversation,
    ) -> BoxFuture<'a, Result<Conversation>> {
        Box::pin(async move {
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin create conversation transaction", error)
                    })?;
                if Self::projection_state(&tx, &conv.conversation_id)?.is_some() {
                    return Err(Self::managed_conversation_error(&conv.conversation_id));
                }
                tx.execute(
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

                let id = tx.last_insert_rowid();
                let row = tx
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
                tx.commit()
                    .map_err(|error| memory_io_error("failed to commit conversation", error))?;
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
                    "SELECT c.id, c.conversation_id, c.user_id, c.agent_type, c.title, c.summary,
                        c.compressed_before_id, c.created_at, c.updated_at
                 FROM conversation c
                 LEFT JOIN conversation_projection p ON p.conversation_id = c.conversation_id
                 WHERE c.conversation_id = ?1 AND (p.lifecycle IS NULL OR p.lifecycle = 'live')",
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
                 FROM conversation c
                 LEFT JOIN conversation_projection p ON p.conversation_id = c.conversation_id
                 WHERE (p.lifecycle IS NULL OR p.lifecycle = 'live')",
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
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin metadata update transaction", error)
                    })?;
                if Self::projection_state(&tx, &conversation_id)?.is_some() {
                    return Err(Self::managed_conversation_error(&conversation_id));
                }
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

                tx.execute(&sql, params_refs.as_slice())
                    .map_err(|e| memory_io_error("failed to update conversation", e))?;
                tx.commit()
                    .map_err(|error| memory_io_error("failed to commit metadata update", error))?;
                Ok(())
            })
            .await
        })
    }

    fn delete_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin delete conversation transaction", error)
                    })?;
                if Self::projection_state(&tx, &conversation_id)?.is_some() {
                    return Err(Self::managed_conversation_error(&conversation_id));
                }
                tx.execute(
                    "DELETE FROM conversation WHERE conversation_id = ?1",
                    params![conversation_id],
                )
                .map_err(|e| memory_io_error("failed to delete conversation", e))?;
                tx.commit().map_err(|error| {
                    memory_io_error("failed to commit conversation deletion", error)
                })?;
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
            if Self::projection_state(&tx, &conversation_id)?.is_some() {
                return Err(Self::managed_conversation_error(&conversation_id));
            }
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
                     FROM message
                     WHERE conversation_id = ?1
                       AND EXISTS (
                           SELECT 1 FROM conversation c
                           LEFT JOIN conversation_projection p
                             ON p.conversation_id = c.conversation_id
                           WHERE c.conversation_id = message.conversation_id
                             AND (p.lifecycle IS NULL OR p.lifecycle = 'live')
                       )
                     ORDER BY id ASC",
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
                        "SELECT COUNT(*) FROM message
                         WHERE conversation_id = ?1
                           AND EXISTS (
                               SELECT 1 FROM conversation c
                               LEFT JOIN conversation_projection p
                                 ON p.conversation_id = c.conversation_id
                               WHERE c.conversation_id = message.conversation_id
                                 AND (p.lifecycle IS NULL OR p.lifecycle = 'live')
                           )",
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

    fn ensure_projection_epoch<'a>(
        &'a self,
        request: EnsureConversationProjectionRequest,
    ) -> BoxFuture<'a, Result<ConversationProjectionEpochReceipt>> {
        Box::pin(async move {
            let conversation_id = request.conversation.conversation_id.clone();
            if conversation_id.trim().is_empty() {
                return Err(MemoryError::SerializationError(
                    "managed conversation id must not be empty".to_string(),
                )
                .into());
            }
            let context = self.persistence_call_context;
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin projection epoch transaction", error)
                    })?;
                if let Some(context) = context {
                    context.ensure_not_expired()?;
                }
                if let Some(mut state) = Self::projection_state(&tx, &conversation_id)? {
                    if state.lifecycle == ConversationProjectionLifecycle::Live {
                        return Ok(ConversationProjectionEpochReceipt {
                            authority: state.authority(&conversation_id),
                            status: if request.expected_tombstone_epoch.is_some() {
                                ConversationProjectionEpochStatus::EpochConflict
                            } else {
                                ConversationProjectionEpochStatus::Existing
                            },
                        });
                    }
                    if request.expected_tombstone_epoch != Some(state.epoch) {
                        return Ok(ConversationProjectionEpochReceipt {
                            authority: state.authority(&conversation_id),
                            status: if request.expected_tombstone_epoch.is_none() {
                                ConversationProjectionEpochStatus::Tombstoned
                            } else {
                                ConversationProjectionEpochStatus::EpochConflict
                            },
                        });
                    }
                    let next_epoch = state
                        .epoch
                        .checked_add(1)
                        .filter(|epoch| *epoch <= MAX_PROJECTION_EPOCH)
                        .ok_or_else(|| {
                            MemoryError::ProjectionEpochExhausted(conversation_id.clone())
                        })?;
                    let next_revision = state.next_revision(&conversation_id)?;
                    tx.execute(
                        "UPDATE conversation
                         SET user_id = ?2, agent_type = ?3, title = ?4, summary = NULL,
                             compressed_before_id = NULL,
                             created_at = datetime('now', 'localtime'),
                             updated_at = datetime('now', 'localtime')
                         WHERE conversation_id = ?1",
                        params![
                            conversation_id,
                            request.conversation.user_id,
                            request.conversation.agent_type,
                            request.conversation.title,
                        ],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to recreate managed conversation", error)
                    })?;
                    tx.execute(
                        "DELETE FROM message WHERE conversation_id = ?1",
                        params![conversation_id],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to clear recreated conversation", error)
                    })?;
                    tx.execute(
                        "DELETE FROM transcript_projection_receipt WHERE conversation_id = ?1",
                        params![conversation_id],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to clear old projection receipts", error)
                    })?;
                    tx.execute(
                        "DELETE FROM transcript_projection_ordinal WHERE conversation_id = ?1",
                        params![conversation_id],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to clear old projection ordinals", error)
                    })?;
                    tx.execute(
                        "UPDATE conversation_projection
                         SET epoch = ?2, revision = ?3, lifecycle = 'live'
                         WHERE conversation_id = ?1",
                        params![
                            conversation_id,
                            Self::sqlite_u64(next_epoch, "epoch")?,
                            Self::sqlite_u64(next_revision, "revision")?,
                        ],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to publish recreated projection epoch", error)
                    })?;
                    state.epoch = next_epoch;
                    state.revision = next_revision;
                    state.lifecycle = ConversationProjectionLifecycle::Live;
                    let authority = state.authority(&conversation_id);
                    tx.commit().map_err(|error| {
                        memory_io_error("failed to commit recreated projection epoch", error)
                    })?;
                    return Ok(ConversationProjectionEpochReceipt {
                        authority,
                        status: ConversationProjectionEpochStatus::Recreated,
                    });
                }

                let exists = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM conversation WHERE conversation_id = ?1)",
                        params![conversation_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|error| {
                        memory_io_error("failed to inspect legacy conversation", error)
                    })?;
                if request.expected_tombstone_epoch.is_some() {
                    let authority = SqliteProjectionState {
                        epoch: 1,
                        revision: 1,
                        lifecycle: ConversationProjectionLifecycle::Live,
                        retention_floor_epoch: 0,
                    }
                    .authority(&conversation_id);
                    return Ok(ConversationProjectionEpochReceipt {
                        authority,
                        status: ConversationProjectionEpochStatus::EpochConflict,
                    });
                }
                let status = if exists {
                    ConversationProjectionEpochStatus::AdoptedLegacy
                } else {
                    tx.execute(
                        "INSERT INTO conversation (conversation_id, user_id, agent_type, title)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            conversation_id,
                            request.conversation.user_id,
                            request.conversation.agent_type,
                            request.conversation.title,
                        ],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to create managed conversation", error)
                    })?;
                    ConversationProjectionEpochStatus::Created
                };
                tx.execute(
                    "INSERT INTO conversation_projection
                     (conversation_id, epoch, revision, lifecycle, retention_floor_epoch)
                     VALUES (?1, 1, 1, 'live', 0)",
                    params![conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to establish conversation projection", error)
                })?;
                let authority = SqliteProjectionState {
                    epoch: 1,
                    revision: 1,
                    lifecycle: ConversationProjectionLifecycle::Live,
                    retention_floor_epoch: 0,
                }
                .authority(&conversation_id);
                tx.commit()
                    .map_err(|error| memory_io_error("failed to commit projection epoch", error))?;
                Ok(ConversationProjectionEpochReceipt { authority, status })
            })
            .await
        })
    }

    fn get_projection_authority<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ConversationProjectionAuthority>>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            if conversation_id.trim().is_empty() {
                return Err(MemoryError::SerializationError(
                    "managed conversation id must not be empty".to_string(),
                )
                .into());
            }
            self.run_db(move |conn| {
                Ok(Self::projection_state(conn, &conversation_id)?
                    .map(|state| state.authority(&conversation_id)))
            })
            .await
        })
    }

    fn get_latest_managed_import<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ManagedConversationImportLocator>>> {
        Box::pin(async move {
            let conversation_id = conversation_id.to_string();
            self.run_db(move |conn| {
                let state = Self::projection_state(conn, &conversation_id)?;
                let locator = Self::import_locator(conn, &conversation_id)?;
                Ok(match (state, locator) {
                    (Some(state), Some(locator))
                        if state.lifecycle == ConversationProjectionLifecycle::Live
                            && state.epoch == locator.applied_epoch =>
                    {
                        Some(locator)
                    }
                    _ => None,
                })
            })
            .await
        })
    }

    fn apply_transcript_projection<'a>(
        &'a self,
        batch: TranscriptProjectionBatch,
    ) -> BoxFuture<'a, Result<TranscriptProjectionApplyReceipt>> {
        Box::pin(async move {
            batch.validate()?;
            let context = self.persistence_call_context;
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin transcript projection transaction", error)
                    })?;
                if let Some(context) = context {
                    context.ensure_not_expired()?;
                }
                let state = Self::projection_state(&tx, &batch.conversation_id)?
                    .ok_or_else(|| Self::managed_conversation_error(&batch.conversation_id))?;
                let existing_operation = tx.query_row(
                    "SELECT payload_digest FROM transcript_projection_receipt
                     WHERE conversation_id = ?1 AND operation_id = ?2",
                    params![batch.conversation_id, batch.operation_id],
                    |row| row.get::<_, String>(0),
                );
                match existing_operation {
                    Ok(existing_digest) => {
                        return Ok(TranscriptProjectionApplyReceipt {
                            operation_id: batch.operation_id,
                            payload_digest: batch.payload_digest.clone(),
                            authority: state.authority(&batch.conversation_id),
                            status: if existing_digest == batch.payload_digest {
                                TranscriptProjectionApplyStatus::AlreadyApplied
                            } else {
                                TranscriptProjectionApplyStatus::Conflict {
                                    current_epoch: state.epoch,
                                    current_revision: state.revision,
                                    kind: TranscriptProjectionConflictKind::OperationIdentity,
                                }
                            },
                        });
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(error) => {
                        return Err(memory_io_error(
                            "failed to query transcript projection receipt",
                            error,
                        )
                        .into());
                    }
                }
                if state.epoch != batch.conversation_epoch
                    || state.lifecycle != ConversationProjectionLifecycle::Live
                {
                    return Ok(TranscriptProjectionApplyReceipt {
                        operation_id: batch.operation_id,
                        payload_digest: batch.payload_digest,
                        authority: state.authority(&batch.conversation_id),
                        status: TranscriptProjectionApplyStatus::Fenced {
                            current_epoch: state.epoch,
                            lifecycle: state.lifecycle,
                        },
                    });
                }

                let (ordinal_count, first_ordinal, last_ordinal) = tx
                    .query_row(
                        "SELECT COUNT(*), MIN(ordinal), MAX(ordinal)
                         FROM transcript_projection_ordinal
                         WHERE conversation_id = ?1 AND generation_id = ?2",
                        params![batch.conversation_id, batch.generation_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                            ))
                        },
                    )
                    .map_err(|error| {
                        memory_io_error("failed to query transcript projection frontier", error)
                    })?;
                let generation_frontier = u64::try_from(ordinal_count).map_err(|_| {
                    MemoryError::SerializationError(
                        "transcript projection ordinal count is negative".to_string(),
                    )
                })?;
                if ordinal_count == 0 {
                    if first_ordinal.is_some() || last_ordinal.is_some() {
                        return Err(MemoryError::SerializationError(
                            "empty transcript projection generation has ordinal bounds".to_string(),
                        )
                        .into());
                    }
                } else {
                    let expected_last = ordinal_count.checked_sub(1).ok_or_else(|| {
                        MemoryError::SerializationError(
                            "transcript projection ordinal frontier underflow".to_string(),
                        )
                    })?;
                    if first_ordinal != Some(0) || last_ordinal != Some(expected_last) {
                        return Err(MemoryError::SerializationError(format!(
                            "transcript projection generation is not contiguous: {}",
                            batch.generation_id
                        ))
                        .into());
                    }
                }
                let batch_first_ordinal =
                    batch
                        .items
                        .first()
                        .map(|item| item.ordinal)
                        .ok_or_else(|| {
                            MemoryError::SerializationError(
                                "transcript projection batch must not be empty".to_string(),
                            )
                        })?;
                if batch_first_ordinal != generation_frontier {
                    return Ok(TranscriptProjectionApplyReceipt {
                        operation_id: batch.operation_id,
                        payload_digest: batch.payload_digest,
                        authority: state.authority(&batch.conversation_id),
                        status: TranscriptProjectionApplyStatus::Conflict {
                            current_epoch: state.epoch,
                            current_revision: state.revision,
                            kind: TranscriptProjectionConflictKind::OrdinalDigest,
                        },
                    });
                }

                let mut new_items = Vec::new();
                for item in &batch.items {
                    let ordinal = Self::sqlite_u64(item.ordinal, "ordinal")?;
                    let existing = tx.query_row(
                        "SELECT digest FROM transcript_projection_ordinal
                         WHERE conversation_id = ?1 AND generation_id = ?2 AND ordinal = ?3",
                        params![batch.conversation_id, batch.generation_id, ordinal],
                        |row| row.get::<_, String>(0),
                    );
                    match existing {
                        Ok(digest) if digest != item.digest => {
                            return Ok(TranscriptProjectionApplyReceipt {
                                operation_id: batch.operation_id,
                                payload_digest: batch.payload_digest,
                                authority: state.authority(&batch.conversation_id),
                                status: TranscriptProjectionApplyStatus::Conflict {
                                    current_epoch: state.epoch,
                                    current_revision: state.revision,
                                    kind: TranscriptProjectionConflictKind::OrdinalDigest,
                                },
                            });
                        }
                        Ok(_) => {}
                        Err(rusqlite::Error::QueryReturnedNoRows) => {
                            new_items.push(item.clone());
                        }
                        Err(error) => {
                            return Err(memory_io_error(
                                "failed to query transcript projection ordinal",
                                error,
                            )
                            .into());
                        }
                    }
                }

                for item in &new_items {
                    let message = &item.message;
                    tx.execute(
                        "INSERT INTO message (conversation_id, role, content, attachments_json,
                            tool_calls_json, tool_result_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            batch.conversation_id,
                            message.role,
                            message.content,
                            message.attachments_json,
                            message.tool_calls_json,
                            message.tool_result_json,
                            message.created_at,
                        ],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to append transcript projection message", error)
                    })?;
                    tx.execute(
                        "INSERT INTO transcript_projection_ordinal
                         (conversation_id, generation_id, ordinal, digest)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            batch.conversation_id,
                            batch.generation_id,
                            Self::sqlite_u64(item.ordinal, "ordinal")?,
                            item.digest,
                        ],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to record transcript projection ordinal", error)
                    })?;
                }
                tx.execute(
                    "INSERT INTO transcript_projection_receipt
                     (conversation_id, operation_id, payload_digest) VALUES (?1, ?2, ?3)",
                    params![
                        batch.conversation_id,
                        batch.operation_id,
                        batch.payload_digest,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to record transcript projection receipt", error)
                })?;
                let next_revision = state.next_revision(&batch.conversation_id)?;
                tx.execute(
                    "UPDATE conversation_projection SET revision = ?2 WHERE conversation_id = ?1",
                    params![
                        batch.conversation_id,
                        Self::sqlite_u64(next_revision, "revision")?
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to advance transcript projection revision", error)
                })?;
                tx.execute(
                    "UPDATE conversation SET updated_at = datetime('now', 'localtime')
                     WHERE conversation_id = ?1",
                    params![batch.conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to touch projected conversation", error)
                })?;
                let receipt = TranscriptProjectionApplyReceipt {
                    operation_id: batch.operation_id,
                    payload_digest: batch.payload_digest,
                    authority: SqliteProjectionState {
                        revision: next_revision,
                        ..state
                    }
                    .authority(&batch.conversation_id),
                    status: TranscriptProjectionApplyStatus::Applied,
                };
                tx.commit().map_err(|error| {
                    memory_io_error("failed to commit transcript projection", error)
                })?;
                Ok(receipt)
            })
            .await
        })
    }

    fn import_managed_messages<'a>(
        &'a self,
        request: ManagedConversationImport,
    ) -> BoxFuture<'a, Result<TranscriptProjectionApplyReceipt>> {
        Box::pin(async move {
            request.validate()?;
            let import_digests = request
                .generation_id
                .as_deref()
                .map(|_| {
                    crate::memory::conversation::managed_import_projection_digests(
                        &request.conversation_id,
                        &request.messages,
                    )?;
                    request.ordinal_digests()
                })
                .transpose()?;
            let context = self.persistence_call_context;
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin managed import transaction", error)
                    })?;
                if let Some(context) = context {
                    context.ensure_not_expired()?;
                }
                let state = Self::projection_state(&tx, &request.conversation_id)?
                    .ok_or_else(|| Self::managed_conversation_error(&request.conversation_id))?;
                let existing_operation = tx.query_row(
                    "SELECT payload_digest FROM transcript_projection_receipt
                     WHERE conversation_id = ?1 AND operation_id = ?2",
                    params![request.conversation_id, request.operation_id],
                    |row| row.get::<_, String>(0),
                );
                match existing_operation {
                    Ok(existing_digest) => {
                        let mut authority = state.authority(&request.conversation_id);
                        if existing_digest == request.payload_digest
                            && let Some(locator) =
                                Self::import_locator(&tx, &request.conversation_id)?
                                    .filter(|locator| locator.matches(&request))
                        {
                            authority.epoch = locator.applied_epoch;
                            authority.revision = locator.applied_revision;
                        }
                        return Ok(TranscriptProjectionApplyReceipt {
                            operation_id: request.operation_id,
                            payload_digest: request.payload_digest.clone(),
                            authority,
                            status: if existing_digest == request.payload_digest {
                                TranscriptProjectionApplyStatus::AlreadyApplied
                            } else {
                                TranscriptProjectionApplyStatus::Conflict {
                                    current_epoch: state.epoch,
                                    current_revision: state.revision,
                                    kind: TranscriptProjectionConflictKind::OperationIdentity,
                                }
                            },
                        });
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(error) => {
                        return Err(memory_io_error(
                            "failed to query managed import receipt",
                            error,
                        )
                        .into());
                    }
                }
                if state.epoch != request.expected_epoch
                    || state.lifecycle != ConversationProjectionLifecycle::Live
                {
                    return Ok(TranscriptProjectionApplyReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        authority: state.authority(&request.conversation_id),
                        status: TranscriptProjectionApplyStatus::Fenced {
                            current_epoch: state.epoch,
                            lifecycle: state.lifecycle,
                        },
                    });
                }
                if state.revision != request.expected_revision {
                    return Ok(TranscriptProjectionApplyReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        authority: state.authority(&request.conversation_id),
                        status: TranscriptProjectionApplyStatus::Conflict {
                            current_epoch: state.epoch,
                            current_revision: state.revision,
                            kind: TranscriptProjectionConflictKind::Revision,
                        },
                    });
                }

                let next_epoch = state
                    .epoch
                    .checked_add(1)
                    .filter(|epoch| *epoch <= MAX_PROJECTION_EPOCH)
                    .ok_or_else(|| {
                        MemoryError::ProjectionEpochExhausted(request.conversation_id.clone())
                    })?;
                let next_revision = state.next_revision(&request.conversation_id)?;
                let applied_authority = SqliteProjectionState {
                    epoch: next_epoch,
                    revision: next_revision,
                    ..state
                }
                .authority(&request.conversation_id);
                let locator =
                    ManagedConversationImportLocator::from_applied(&request, &applied_authority)?;
                Self::replace_messages(&tx, &request.conversation_id, &request.messages)?;
                tx.execute(
                    "DELETE FROM transcript_projection_receipt WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to clear pre-import projection receipts", error)
                })?;
                tx.execute(
                    "DELETE FROM transcript_projection_ordinal WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to clear pre-import projection ordinals", error)
                })?;
                tx.execute(
                    "DELETE FROM managed_import_locator WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| memory_io_error("failed to clear pre-import locator", error))?;
                if let Some(locator) = locator {
                    tx.execute(
                        "INSERT INTO managed_import_locator (conversation_id, locator_json)
                         VALUES (?1, ?2)",
                        params![request.conversation_id, serde_json::to_string(&locator)?,],
                    )
                    .map_err(|error| {
                        memory_io_error("failed to record managed import locator", error)
                    })?;
                }
                if let (Some(generation_id), Some(digests)) =
                    (&request.generation_id, import_digests)
                {
                    for (ordinal, digest) in digests.into_iter().enumerate() {
                        let ordinal = u64::try_from(ordinal).map_err(|_| {
                            MemoryError::SerializationError(
                                "managed import ordinal capacity exhausted".to_string(),
                            )
                        })?;
                        tx.execute(
                            "INSERT INTO transcript_projection_ordinal
                             (conversation_id, generation_id, ordinal, digest)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![
                                request.conversation_id,
                                generation_id,
                                Self::sqlite_u64(ordinal, "ordinal")?,
                                digest,
                            ],
                        )
                        .map_err(|error| {
                            memory_io_error("failed to seed managed import ordinal", error)
                        })?;
                    }
                }
                tx.execute(
                    "INSERT INTO transcript_projection_receipt
                     (conversation_id, operation_id, payload_digest) VALUES (?1, ?2, ?3)",
                    params![
                        request.conversation_id,
                        request.operation_id,
                        request.payload_digest,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to record managed import receipt", error)
                })?;
                tx.execute(
                    "UPDATE conversation_projection SET epoch = ?2, revision = ?3
                     WHERE conversation_id = ?1",
                    params![
                        request.conversation_id,
                        Self::sqlite_u64(next_epoch, "epoch")?,
                        Self::sqlite_u64(next_revision, "revision")?,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to advance managed import revision", error)
                })?;
                tx.execute(
                    "UPDATE conversation
                     SET updated_at = datetime('now', 'localtime'), summary = NULL,
                         compressed_before_id = NULL
                     WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| memory_io_error("failed to touch managed import", error))?;
                let receipt = TranscriptProjectionApplyReceipt {
                    operation_id: request.operation_id,
                    payload_digest: request.payload_digest,
                    authority: applied_authority,
                    status: TranscriptProjectionApplyStatus::Applied,
                };
                tx.commit()
                    .map_err(|error| memory_io_error("failed to commit managed import", error))?;
                Ok(receipt)
            })
            .await
        })
    }

    fn update_managed_conversation<'a>(
        &'a self,
        request: ManagedConversationMetadataUpdate,
    ) -> BoxFuture<'a, Result<ManagedConversationMetadataUpdateReceipt>> {
        Box::pin(async move {
            request.validate()?;
            let context = self.persistence_call_context;
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin managed metadata transaction", error)
                    })?;
                if let Some(context) = context {
                    context.ensure_not_expired()?;
                }
                let state = Self::projection_state(&tx, &request.conversation_id)?
                    .ok_or_else(|| Self::managed_conversation_error(&request.conversation_id))?;
                let existing_operation = tx.query_row(
                    "SELECT payload_digest FROM transcript_projection_receipt
                     WHERE conversation_id = ?1 AND operation_id = ?2",
                    params![request.conversation_id, request.operation_id],
                    |row| row.get::<_, String>(0),
                );
                match existing_operation {
                    Ok(existing_digest) => {
                        return Ok(ManagedConversationMetadataUpdateReceipt {
                            operation_id: request.operation_id,
                            payload_digest: request.payload_digest.clone(),
                            authority: state.authority(&request.conversation_id),
                            status: if existing_digest == request.payload_digest {
                                ManagedConversationMetadataUpdateStatus::AlreadyUpdated
                            } else {
                                ManagedConversationMetadataUpdateStatus::IdentityConflict
                            },
                        });
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(error) => {
                        return Err(memory_io_error(
                            "failed to query managed metadata receipt",
                            error,
                        )
                        .into());
                    }
                }
                if state.epoch != request.expected_epoch
                    || state.lifecycle != ConversationProjectionLifecycle::Live
                {
                    return Ok(ManagedConversationMetadataUpdateReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        authority: state.authority(&request.conversation_id),
                        status: ManagedConversationMetadataUpdateStatus::EpochConflict,
                    });
                }
                if state.revision != request.expected_revision {
                    return Ok(ManagedConversationMetadataUpdateReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        authority: state.authority(&request.conversation_id),
                        status: ManagedConversationMetadataUpdateStatus::RevisionConflict,
                    });
                }
                if let Some(compressed_before_id) = request.compressed_before_id {
                    let boundary_exists = tx
                        .query_row(
                            "SELECT EXISTS(
                                SELECT 1 FROM message
                                WHERE conversation_id = ?1 AND id = ?2
                            )",
                            params![request.conversation_id, compressed_before_id],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(|error| {
                            memory_io_error("failed to validate compression boundary", error)
                        })?;
                    if !boundary_exists {
                        return Err(MemoryError::NotFound(format!(
                            "conversation compression boundary: {compressed_before_id}"
                        ))
                        .into());
                    }
                }

                let mut sets = Vec::new();
                let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
                if let Some(title) = request.title.as_ref() {
                    sets.push(format!("title = ?{}", values.len() + 1));
                    values.push(Box::new(title.clone()));
                }
                if let Some(summary) = request.summary.as_ref() {
                    sets.push(format!("summary = ?{}", values.len() + 1));
                    values.push(Box::new(summary.clone()));
                }
                if let Some(compressed_before_id) = request.compressed_before_id {
                    sets.push(format!("compressed_before_id = ?{}", values.len() + 1));
                    values.push(Box::new(compressed_before_id));
                }
                sets.push("updated_at = datetime('now', 'localtime')".to_string());
                values.push(Box::new(request.conversation_id.clone()));
                let sql = format!(
                    "UPDATE conversation SET {} WHERE conversation_id = ?{}",
                    sets.join(", "),
                    values.len()
                );
                let value_refs = values
                    .iter()
                    .map(|value| value.as_ref())
                    .collect::<Vec<&dyn rusqlite::types::ToSql>>();
                tx.execute(&sql, value_refs.as_slice()).map_err(|error| {
                    memory_io_error("failed to update managed conversation metadata", error)
                })?;
                tx.execute(
                    "INSERT INTO transcript_projection_receipt
                     (conversation_id, operation_id, payload_digest) VALUES (?1, ?2, ?3)",
                    params![
                        request.conversation_id,
                        request.operation_id,
                        request.payload_digest,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to record managed metadata receipt", error)
                })?;
                let next_revision = state.next_revision(&request.conversation_id)?;
                tx.execute(
                    "UPDATE conversation_projection SET revision = ?2 WHERE conversation_id = ?1",
                    params![
                        request.conversation_id,
                        Self::sqlite_u64(next_revision, "revision")?
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to advance managed metadata revision", error)
                })?;
                let receipt = ManagedConversationMetadataUpdateReceipt {
                    operation_id: request.operation_id,
                    payload_digest: request.payload_digest,
                    authority: SqliteProjectionState {
                        revision: next_revision,
                        ..state
                    }
                    .authority(&request.conversation_id),
                    status: ManagedConversationMetadataUpdateStatus::Updated,
                };
                tx.commit().map_err(|error| {
                    memory_io_error("failed to commit managed metadata update", error)
                })?;
                Ok(receipt)
            })
            .await
        })
    }

    fn delete_managed_conversation<'a>(
        &'a self,
        request: ManagedConversationDelete,
    ) -> BoxFuture<'a, Result<ManagedConversationDeleteReceipt>> {
        Box::pin(async move {
            request.validate()?;
            let context = self.persistence_call_context;
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin managed delete transaction", error)
                    })?;
                if let Some(context) = context {
                    context.ensure_not_expired()?;
                }
                let state = Self::projection_state(&tx, &request.conversation_id)?
                    .ok_or_else(|| Self::managed_conversation_error(&request.conversation_id))?;
                let existing_receipt = tx.query_row(
                    "SELECT payload_digest, deleted_epoch
                     FROM managed_conversation_delete_receipt
                     WHERE conversation_id = ?1 AND operation_id = ?2",
                    params![request.conversation_id, request.operation_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                );
                match existing_receipt {
                    Ok((payload_digest, deleted_epoch)) => {
                        return Ok(ManagedConversationDeleteReceipt {
                            operation_id: request.operation_id,
                            payload_digest: payload_digest.clone(),
                            deleted_epoch: u64::try_from(deleted_epoch).map_err(|_| {
                                MemoryError::SerializationError(
                                    "managed delete receipt epoch is negative".to_string(),
                                )
                            })?,
                            retention_floor_epoch: state.retention_floor_epoch,
                            status: if payload_digest == request.payload_digest {
                                ManagedConversationDeleteStatus::AlreadyDeleted
                            } else {
                                ManagedConversationDeleteStatus::IdentityConflict
                            },
                        });
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(error) => {
                        return Err(memory_io_error(
                            "failed to query managed delete receipt",
                            error,
                        )
                        .into());
                    }
                }
                if request.expected_epoch <= state.retention_floor_epoch {
                    return Ok(ManagedConversationDeleteReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        deleted_epoch: request.expected_epoch,
                        retention_floor_epoch: state.retention_floor_epoch,
                        status: ManagedConversationDeleteStatus::ReceiptExpired,
                    });
                }
                if state.epoch != request.expected_epoch
                    || state.lifecycle != ConversationProjectionLifecycle::Live
                {
                    return Ok(ManagedConversationDeleteReceipt {
                        operation_id: request.operation_id,
                        payload_digest: request.payload_digest,
                        deleted_epoch: request.expected_epoch,
                        retention_floor_epoch: state.retention_floor_epoch,
                        status: ManagedConversationDeleteStatus::EpochConflict,
                    });
                }

                let next_revision = state.next_revision(&request.conversation_id)?;
                tx.execute(
                    "DELETE FROM message WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| memory_io_error("failed to clear deleted transcript", error))?;
                tx.execute(
                    "DELETE FROM transcript_projection_receipt WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to clear deleted projection receipts", error)
                })?;
                tx.execute(
                    "DELETE FROM transcript_projection_ordinal WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| {
                    memory_io_error("failed to clear deleted projection ordinals", error)
                })?;
                tx.execute(
                    "INSERT INTO managed_conversation_delete_receipt
                     (conversation_id, operation_id, payload_digest, deleted_epoch)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        request.conversation_id,
                        request.operation_id,
                        request.payload_digest,
                        Self::sqlite_u64(request.expected_epoch, "delete epoch")?,
                    ],
                )
                .map_err(|error| memory_io_error("failed to record managed delete", error))?;

                let receipt_count = tx
                    .query_row(
                        "SELECT COUNT(*) FROM managed_conversation_delete_receipt
                         WHERE conversation_id = ?1",
                        params![request.conversation_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| {
                        memory_io_error("failed to count managed delete receipts", error)
                    })?;
                let mut retention_floor_epoch = state.retention_floor_epoch;
                if receipt_count > DELETE_RECEIPT_RETENTION {
                    let excess = receipt_count.saturating_sub(DELETE_RECEIPT_RETENTION);
                    let expired = {
                        let mut statement = tx
                            .prepare(
                                "SELECT operation_id, deleted_epoch
                                 FROM managed_conversation_delete_receipt
                                 WHERE conversation_id = ?1
                                 ORDER BY deleted_epoch ASC
                                 LIMIT ?2",
                            )
                            .map_err(|error| {
                                memory_io_error("failed to prepare delete receipt retention", error)
                            })?;
                        let rows = statement
                            .query_map(params![request.conversation_id, excess], |row| {
                                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                            })
                            .map_err(|error| {
                                memory_io_error("failed to query expired delete receipts", error)
                            })?;
                        let mut expired = Vec::new();
                        for row in rows {
                            expired.push(row.map_err(|error| {
                                memory_io_error("failed to read expired delete receipt", error)
                            })?);
                        }
                        expired
                    };
                    for (operation_id, deleted_epoch) in expired {
                        tx.execute(
                            "DELETE FROM managed_conversation_delete_receipt
                             WHERE conversation_id = ?1 AND operation_id = ?2",
                            params![request.conversation_id, operation_id],
                        )
                        .map_err(|error| {
                            memory_io_error("failed to expire managed delete receipt", error)
                        })?;
                        let deleted_epoch = u64::try_from(deleted_epoch).map_err(|_| {
                            MemoryError::SerializationError(
                                "expired delete receipt epoch is negative".to_string(),
                            )
                        })?;
                        retention_floor_epoch = retention_floor_epoch.max(deleted_epoch);
                    }
                }
                tx.execute(
                    "UPDATE conversation_projection
                     SET revision = ?2, lifecycle = 'deleted', retention_floor_epoch = ?3
                     WHERE conversation_id = ?1",
                    params![
                        request.conversation_id,
                        Self::sqlite_u64(next_revision, "revision")?,
                        Self::sqlite_u64(retention_floor_epoch, "retention floor")?,
                    ],
                )
                .map_err(|error| {
                    memory_io_error("failed to publish managed delete fence", error)
                })?;
                tx.execute(
                    "UPDATE conversation
                     SET summary = NULL, compressed_before_id = NULL,
                         updated_at = datetime('now', 'localtime')
                     WHERE conversation_id = ?1",
                    params![request.conversation_id],
                )
                .map_err(|error| memory_io_error("failed to touch deleted conversation", error))?;
                let receipt = ManagedConversationDeleteReceipt {
                    operation_id: request.operation_id,
                    payload_digest: request.payload_digest,
                    deleted_epoch: request.expected_epoch,
                    retention_floor_epoch,
                    status: ManagedConversationDeleteStatus::Deleted,
                };
                tx.commit()
                    .map_err(|error| memory_io_error("failed to commit managed delete", error))?;
                Ok(receipt)
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
                     LEFT JOIN conversation_projection p ON p.conversation_id = c.conversation_id
                     LEFT JOIN message m ON c.conversation_id = m.conversation_id
                     WHERE (p.lifecycle IS NULL OR p.lifecycle = 'live')
                       AND (c.title LIKE ?1 OR m.content LIKE ?1)
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

    fn ensure_conversation<'a>(
        &'a self,
        conv: NewConversation,
    ) -> BoxFuture<'a, Result<Conversation>> {
        Box::pin(async move {
            self.run_db(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| {
                        memory_io_error("failed to begin ensure conversation transaction", error)
                    })?;
                if Self::projection_state(&tx, &conv.conversation_id)?.is_some() {
                    return Err(Self::managed_conversation_error(&conv.conversation_id));
                }
                let existing = tx.query_row(
                    "SELECT id, conversation_id, user_id, agent_type, title, summary,
                        compressed_before_id, created_at, updated_at
                     FROM conversation WHERE conversation_id = ?1",
                    params![conv.conversation_id],
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
                match existing {
                    Ok(existing) => return Ok(existing),
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(error) => {
                        return Err(
                            memory_io_error("failed to query ensured conversation", error).into(),
                        );
                    }
                }
                tx.execute(
                    "INSERT INTO conversation (conversation_id, user_id, agent_type, title)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        conv.conversation_id,
                        conv.user_id,
                        conv.agent_type,
                        conv.title
                    ],
                )
                .map_err(|error| memory_io_error("failed to ensure conversation", error))?;
                let id = tx.last_insert_rowid();
                let created = tx
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
                    .map_err(|error| {
                        memory_io_error("failed to query ensured conversation", error)
                    })?;
                tx.commit().map_err(|error| {
                    memory_io_error("failed to commit ensured conversation", error)
                })?;
                Ok(created)
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::memory::conversation::{
        ConversationProjectionCapability, ConversationProjectionEpochStatus,
        EnsureConversationProjectionRequest, ManagedConversationDelete,
        ManagedConversationDeleteStatus, ManagedConversationImport, PersistenceCallContext,
        TranscriptProjectionApplyStatus, TranscriptProjectionBatch,
        TranscriptProjectionConflictKind,
    };

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

    fn new_conversation(conversation_id: &str) -> NewConversation {
        NewConversation {
            conversation_id: conversation_id.to_string(),
            user_id: "local".to_string(),
            agent_type: None,
            title: None,
        }
    }

    fn projection_batch(
        conversation_id: &str,
        epoch: u64,
        generation_id: &str,
        first_ordinal: u64,
        contents: &[&str],
    ) -> Result<TranscriptProjectionBatch> {
        TranscriptProjectionBatch::prepare(
            conversation_id,
            epoch,
            generation_id,
            first_ordinal,
            contents
                .iter()
                .map(|content| stored_message(conversation_id, None, content))
                .collect(),
        )
    }

    fn ensure_projection_request(
        conversation_id: &str,
        expected_tombstone_epoch: Option<u64>,
    ) -> EnsureConversationProjectionRequest {
        EnsureConversationProjectionRequest {
            conversation: new_conversation(conversation_id),
            expected_tombstone_epoch,
        }
    }

    fn managed_import(
        conversation_id: &str,
        expected_epoch: u64,
        expected_revision: u64,
        contents: &[&str],
    ) -> Result<ManagedConversationImport> {
        ManagedConversationImport::prepare(
            conversation_id,
            expected_epoch,
            expected_revision,
            contents
                .iter()
                .map(|content| stored_message(conversation_id, None, content))
                .collect(),
        )
    }

    fn managed_delete(
        conversation_id: &str,
        expected_epoch: u64,
    ) -> Result<ManagedConversationDelete> {
        ManagedConversationDelete::prepare(conversation_id, expected_epoch)
    }

    fn managed_metadata_update(
        conversation_id: &str,
        expected_epoch: u64,
        expected_revision: u64,
        title: &str,
        summary: &str,
        compressed_before_id: i64,
    ) -> Result<ManagedConversationMetadataUpdate> {
        ManagedConversationMetadataUpdate::prepare(
            conversation_id,
            expected_epoch,
            expected_revision,
            Some(title.to_string()),
            Some(summary.to_string()),
            Some(compressed_before_id),
        )
    }

    fn is_managed_projection_error<T>(result: Result<T>) -> bool {
        matches!(
            result,
            Err(echo_core::error::ReactError::Memory(error))
                if matches!(
                    error.as_ref(),
                    MemoryError::ManagedConversationRequiresProjection(_)
                )
        )
    }

    fn is_deadline_exceeded<T>(result: Result<T>) -> bool {
        matches!(
            result,
            Err(echo_core::error::ReactError::Memory(error))
                if matches!(error.as_ref(), MemoryError::DeadlineExceeded(_))
        )
    }

    fn execute_test_sql(store: &SqliteConversationStore, sql: &str) -> Result<()> {
        let guard = store.conn.lock().map_err(|error| {
            MemoryError::IoError(format!("lock SQLite test connection: {error}"))
        })?;
        guard
            .execute_batch(sql)
            .map_err(|error| memory_io_error("execute SQLite test fault", error))?;
        Ok(())
    }

    #[tokio::test]
    async fn projection_epoch_recreation_requires_a_matching_tombstone() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;

        let absent = store
            .ensure_projection_epoch(ensure_projection_request("absent-epoch", Some(7)))
            .await?;
        assert_eq!(
            absent.status,
            ConversationProjectionEpochStatus::EpochConflict
        );
        assert!(store.get_conversation("absent-epoch").await?.is_none());
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("absent-epoch", None))
                .await?
                .status,
            ConversationProjectionEpochStatus::Created
        );
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("absent-epoch", Some(1)))
                .await?
                .status,
            ConversationProjectionEpochStatus::EpochConflict
        );
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("absent-epoch", None))
                .await?
                .status,
            ConversationProjectionEpochStatus::Existing
        );

        store
            .create_conversation(new_conversation("legacy-epoch"))
            .await?;
        store
            .save_messages(
                "legacy-epoch",
                &[stored_message("legacy-epoch", None, "before-conflict")],
            )
            .await?;
        let legacy = store
            .ensure_projection_epoch(ensure_projection_request("legacy-epoch", Some(1)))
            .await?;
        assert_eq!(
            legacy.status,
            ConversationProjectionEpochStatus::EpochConflict
        );
        assert_eq!(store.count_messages("legacy-epoch").await?, 1);
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("legacy-epoch", None))
                .await?
                .status,
            ConversationProjectionEpochStatus::AdoptedLegacy
        );

        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn projection_authority_query_is_read_only_and_fails_closed_on_corruption() -> Result<()>
    {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;

        assert!(
            store
                .get_projection_authority("authority-live")
                .await?
                .is_none()
        );
        assert!(store.get_conversation("authority-live").await?.is_none());
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("authority-live", None))
            .await?;
        assert_eq!(acquired.status, ConversationProjectionEpochStatus::Created);
        assert_eq!(
            store
                .get_projection_authority_with_context(
                    PersistenceCallContext {
                        absolute_deadline_unix_ms: i64::MAX,
                    },
                    "authority-live",
                )
                .await?,
            Some(acquired.authority.clone())
        );

        store
            .create_conversation(new_conversation("authority-legacy"))
            .await?;
        assert!(
            store
                .get_projection_authority("authority-legacy")
                .await?
                .is_none()
        );
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("authority-legacy", None))
                .await?
                .status,
            ConversationProjectionEpochStatus::AdoptedLegacy
        );

        let deleted = store
            .delete_managed_conversation(managed_delete(
                "authority-live",
                acquired.authority.epoch,
            )?)
            .await?;
        let deleted_authority = store
            .get_projection_authority("authority-live")
            .await?
            .ok_or_else(|| MemoryError::NotFound("deleted authority".to_string()))?;
        assert_eq!(deleted_authority.conversation_id, "authority-live");
        assert_eq!(deleted_authority.epoch, deleted.deleted_epoch);
        assert_eq!(
            deleted_authority.revision,
            acquired.authority.revision.checked_add(1).ok_or_else(|| {
                MemoryError::Unsupported("deleted authority revision overflow".to_string())
            })?
        );
        assert_eq!(
            deleted_authority.lifecycle,
            ConversationProjectionLifecycle::Deleted
        );
        assert_eq!(
            deleted_authority.delete_receipt_retention_floor_epoch,
            deleted.retention_floor_epoch
        );

        store
            .ensure_projection_epoch(ensure_projection_request("authority-corrupt", None))
            .await?;
        execute_test_sql(
            &store,
            "PRAGMA ignore_check_constraints = ON;
             UPDATE conversation_projection SET lifecycle = 'corrupt'
             WHERE conversation_id = 'authority-corrupt';
             PRAGMA ignore_check_constraints = OFF;",
        )?;
        let query = store.get_projection_authority("authority-corrupt").await;
        assert!(matches!(
            query,
            Err(echo_core::error::ReactError::Memory(error))
                if matches!(error.as_ref(), MemoryError::SerializationError(_))
        ));
        let lifecycle = {
            let guard = store.conn.lock().map_err(|error| {
                MemoryError::IoError(format!("lock SQLite test connection: {error}"))
            })?;
            guard
                .query_row(
                    "SELECT lifecycle FROM conversation_projection WHERE conversation_id = ?1",
                    params!["authority-corrupt"],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| memory_io_error("query corrupt projection lifecycle", error))?
        };
        assert_eq!(lifecycle, "corrupt");

        let invalid = store.get_projection_authority("  ").await;
        assert!(matches!(
            invalid,
            Err(echo_core::error::ReactError::Memory(error))
                if matches!(error.as_ref(), MemoryError::SerializationError(_))
        ));

        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn transcript_projection_rejects_non_contiguous_generation_batches() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        store
            .ensure_projection_epoch(ensure_projection_request("frontier", None))
            .await?;

        let ordinal_ten = store
            .apply_transcript_projection(projection_batch(
                "frontier",
                1,
                "generation-a",
                10,
                &["ordinal-10"],
            )?)
            .await?;
        assert!(matches!(
            ordinal_ten.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::OrdinalDigest,
                ..
            }
        ));
        assert_eq!(ordinal_ten.authority.revision, 1);
        assert_eq!(store.count_messages("frontier").await?, 0);

        let first = projection_batch("frontier", 1, "generation-a", 0, &["ordinal-0"])?;
        assert_eq!(
            store
                .apply_transcript_projection(first.clone())
                .await?
                .status,
            TranscriptProjectionApplyStatus::Applied
        );
        assert_eq!(
            store.apply_transcript_projection(first).await?.status,
            TranscriptProjectionApplyStatus::AlreadyApplied
        );

        let gap = store
            .apply_transcript_projection(projection_batch(
                "frontier",
                1,
                "generation-a",
                2,
                &["ordinal-2"],
            )?)
            .await?;
        assert!(matches!(
            gap.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::OrdinalDigest,
                ..
            }
        ));
        assert_eq!(gap.authority.revision, 2);
        assert_eq!(store.count_messages("frontier").await?, 1);

        assert_eq!(
            store
                .apply_transcript_projection(projection_batch(
                    "frontier",
                    1,
                    "generation-a",
                    1,
                    &["ordinal-1"],
                )?)
                .await?
                .status,
            TranscriptProjectionApplyStatus::Applied
        );
        let late_zero = store
            .apply_transcript_projection(projection_batch(
                "frontier",
                1,
                "generation-a",
                0,
                &["late-ordinal-0"],
            )?)
            .await?;
        assert!(matches!(
            late_zero.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::OrdinalDigest,
                ..
            }
        ));
        assert_eq!(late_zero.authority.revision, 3);
        let messages = store.get_messages("frontier").await?;
        assert_eq!(
            messages
                .iter()
                .filter_map(|message| message.content.as_deref())
                .collect::<Vec<_>>(),
            vec!["ordinal-0", "ordinal-1"]
        );

        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn atomic_projection_is_idempotent_and_merges_generations() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        assert_eq!(
            store.projection_capability(),
            ConversationProjectionCapability::AtomicV1
        );
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("atomic", None))
            .await?;
        assert_eq!(acquired.status, ConversationProjectionEpochStatus::Created);

        let first = projection_batch("atomic", 1, "generation-a", 0, &["a0", "a1"])?;
        assert_eq!(
            store
                .apply_transcript_projection(first.clone())
                .await?
                .status,
            TranscriptProjectionApplyStatus::Applied
        );
        assert_eq!(
            store.apply_transcript_projection(first).await?.status,
            TranscriptProjectionApplyStatus::AlreadyApplied
        );
        assert_eq!(
            store
                .apply_transcript_projection(projection_batch(
                    "atomic",
                    1,
                    "generation-b",
                    0,
                    &["b0"],
                )?)
                .await?
                .status,
            TranscriptProjectionApplyStatus::Applied
        );
        let messages = store.get_messages("atomic").await?;
        assert_eq!(
            messages
                .iter()
                .filter_map(|message| message.content.as_deref())
                .collect::<Vec<_>>(),
            vec!["a0", "a1", "b0"]
        );

        let conflict = store
            .apply_transcript_projection(projection_batch(
                "atomic",
                1,
                "generation-a",
                0,
                &["changed"],
            )?)
            .await?;
        assert!(matches!(
            conflict.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::OrdinalDigest,
                ..
            }
        ));
        assert_eq!(store.count_messages("atomic").await?, 3);
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_projection_context_rejects_expired_deadlines() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let db_path = dir.join("conversations.db");
        let store = SqliteConversationStore::new(&db_path)?;
        assert_eq!(
            store.persistence_call_capability(),
            PersistenceCallCapability::AbsoluteDeadlineV1
        );
        let expired = PersistenceCallContext {
            absolute_deadline_unix_ms: 0,
        };

        assert!(is_deadline_exceeded(
            store
                .ensure_projection_epoch_with_context(
                    expired,
                    ensure_projection_request("expired", None),
                )
                .await
        ));
        assert!(is_deadline_exceeded(
            store
                .get_projection_authority_with_context(expired, "expired")
                .await
        ));
        assert!(is_deadline_exceeded(
            store
                .apply_transcript_projection_with_context(
                    expired,
                    projection_batch("expired", 1, "generation", 0, &["message"])?
                )
                .await
        ));
        assert!(is_deadline_exceeded(
            store
                .import_managed_messages_with_context(
                    expired,
                    managed_import("expired", 1, 0, &["message"])?
                )
                .await
        ));
        assert!(is_deadline_exceeded(
            store
                .update_managed_conversation_with_context(
                    expired,
                    managed_metadata_update("expired", 1, 0, "title", "summary", 1)?
                )
                .await
        ));
        assert!(is_deadline_exceeded(
            store
                .delete_managed_conversation_with_context(expired, managed_delete("expired", 1)?)
                .await
        ));
        assert!(store.get_conversation("expired").await?.is_none());

        let conn = Arc::clone(&store.conn);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || -> Result<()> {
            let _guard = conn.lock().map_err(|error| {
                MemoryError::IoError(format!("lock SQLite deadline blocker: {error}"))
            })?;
            locked_tx.send(()).map_err(|error| {
                MemoryError::IoError(format!("publish SQLite deadline blocker: {error}"))
            })?;
            release_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|error| {
                    MemoryError::IoError(format!("release SQLite deadline blocker: {error}"))
                })?;
            Ok(())
        });
        locked_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|error| {
                MemoryError::IoError(format!("wait for SQLite deadline blocker: {error}"))
            })?;
        let locked_context =
            PersistenceCallContext::with_timeout(std::time::Duration::from_millis(25))?;
        let (locked_result, release_result) = tokio::join!(
            store.ensure_projection_epoch_with_context(
                locked_context,
                ensure_projection_request("locked-expiry", None),
            ),
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
                release_tx.send(()).map_err(|error| {
                    MemoryError::IoError(format!("release SQLite deadline blocker: {error}"))
                })
            }
        );
        release_result?;
        blocker
            .join()
            .map_err(|_| MemoryError::IoError("SQLite deadline blocker panicked".to_string()))??;
        assert!(is_deadline_exceeded(locked_result));
        assert!(store.get_conversation("locked-expiry").await?.is_none());

        let transaction_blocker = Connection::open(&db_path)
            .map_err(|error| memory_io_error("open SQLite transaction blocker", error))?;
        transaction_blocker
            .execute_batch("PRAGMA busy_timeout=2000; BEGIN IMMEDIATE")
            .map_err(|error| memory_io_error("begin SQLite transaction blocker", error))?;
        let transaction_context =
            PersistenceCallContext::with_timeout(std::time::Duration::from_millis(25))?;
        let (transaction_result, release_result) = tokio::join!(
            store.ensure_projection_epoch_with_context(
                transaction_context,
                ensure_projection_request("transaction-expiry", None),
            ),
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
                transaction_blocker
                    .execute_batch("ROLLBACK")
                    .map_err(|error| memory_io_error("release SQLite transaction blocker", error))
            }
        );
        release_result?;
        assert!(is_deadline_exceeded(transaction_result));
        assert!(
            store
                .get_conversation("transaction-expiry")
                .await?
                .is_none()
        );

        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_import_uses_revision_cas_and_fences_raw_mutators() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        store
            .create_conversation(new_conversation("managed-import"))
            .await?;
        store
            .save_messages(
                "managed-import",
                &[stored_message("managed-import", None, "legacy")],
            )
            .await?;
        let adopted = store
            .ensure_projection_epoch(ensure_projection_request("managed-import", None))
            .await?;
        assert_eq!(
            adopted.status,
            ConversationProjectionEpochStatus::AdoptedLegacy
        );
        assert!(is_managed_projection_error(
            store
                .save_messages(
                    "managed-import",
                    &[stored_message("managed-import", None, "raw-overwrite")],
                )
                .await
        ));
        assert!(is_managed_projection_error(
            store.delete_conversation("managed-import").await
        ));
        assert!(is_managed_projection_error(
            store
                .ensure_conversation(new_conversation("managed-import"))
                .await
        ));
        assert!(is_managed_projection_error(
            store
                .create_conversation(new_conversation("managed-import"))
                .await
        ));
        assert!(is_managed_projection_error(
            store
                .update_conversation("managed-import", Some("late"), Some("late"), Some(1))
                .await
        ));

        let old_batch = projection_batch(
            "managed-import",
            adopted.authority.epoch,
            "pre-import-generation",
            0,
            &["old-projection"],
        )?;
        let projected = store.apply_transcript_projection(old_batch.clone()).await?;
        assert_eq!(projected.status, TranscriptProjectionApplyStatus::Applied);
        let import = managed_import(
            "managed-import",
            adopted.authority.epoch,
            projected.authority.revision,
            &["restored"],
        )?;
        let imported = store.import_managed_messages(import.clone()).await?;
        assert_eq!(imported.status, TranscriptProjectionApplyStatus::Applied);
        let repeated = store.import_managed_messages(import.clone()).await?;
        assert_eq!(
            repeated.status,
            TranscriptProjectionApplyStatus::AlreadyApplied
        );
        let mut tampered_import = import;
        tampered_import.messages = vec![stored_message("managed-import", None, "tampered")];
        assert!(
            store
                .import_managed_messages(tampered_import)
                .await
                .is_err()
        );
        let late_old_projection = store.apply_transcript_projection(old_batch).await?;
        assert!(matches!(
            late_old_projection.status,
            TranscriptProjectionApplyStatus::Fenced {
                current_epoch: 2,
                ..
            }
        ));
        let stale = store
            .import_managed_messages(managed_import(
                "managed-import",
                imported.authority.epoch,
                adopted.authority.revision,
                &["stale"],
            )?)
            .await?;
        assert!(matches!(
            stale.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::Revision,
                ..
            }
        ));
        let messages = store.get_messages("managed-import").await?;
        assert_eq!(
            messages
                .first()
                .and_then(|message| message.content.as_deref()),
            Some("restored")
        );
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn generation_bound_import_seeds_projection_frontier() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("import-frontier", None))
            .await?;
        let imported_message = stored_message("import-frontier", None, "restored");
        let import = ManagedConversationImport::prepare_for_generation(
            "import-frontier",
            acquired.authority.epoch,
            acquired.authority.revision,
            "import-frontier",
            vec![imported_message.clone()],
        )?;
        let imported = store.import_managed_messages(import.clone()).await?;
        assert_eq!(imported.status, TranscriptProjectionApplyStatus::Applied);
        let locator = store
            .get_latest_managed_import("import-frontier")
            .await?
            .ok_or_else(|| MemoryError::NotFound("managed import locator".to_string()))?;
        assert!(locator.matches(&import));
        let renamed = store
            .update_managed_conversation(ManagedConversationMetadataUpdate::prepare(
                "import-frontier",
                imported.authority.epoch,
                imported.authority.revision,
                Some("Renamed".to_string()),
                None,
                None,
            )?)
            .await?;
        assert_eq!(
            renamed.status,
            ManagedConversationMetadataUpdateStatus::Updated
        );
        let repeated = store.import_managed_messages(import).await?;
        assert_eq!(
            repeated.status,
            TranscriptProjectionApplyStatus::AlreadyApplied
        );
        assert_eq!(repeated.authority.revision, imported.authority.revision);
        assert_eq!(
            store
                .get_latest_managed_import("import-frontier")
                .await?
                .map(|locator| locator.applied_revision),
            Some(imported.authority.revision)
        );
        let replay = TranscriptProjectionBatch::prepare(
            "import-frontier",
            imported.authority.epoch,
            "import-frontier",
            0,
            vec![imported_message],
        )?;
        assert!(matches!(
            store.apply_transcript_projection(replay).await?.status,
            TranscriptProjectionApplyStatus::Conflict {
                kind: TranscriptProjectionConflictKind::OrdinalDigest,
                ..
            }
        ));
        let appended = store
            .apply_transcript_projection(projection_batch(
                "import-frontier",
                imported.authority.epoch,
                "import-frontier",
                1,
                &["next"],
            )?)
            .await?;
        assert_eq!(appended.status, TranscriptProjectionApplyStatus::Applied);
        assert_eq!(store.get_messages("import-frontier").await?.len(), 2);
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_metadata_update_is_idempotent_and_epoch_fenced() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("metadata", None))
            .await?;
        let applied = store
            .apply_transcript_projection(projection_batch(
                "metadata",
                acquired.authority.epoch,
                "generation-a",
                0,
                &["message"],
            )?)
            .await?;
        let message_id = store
            .get_messages("metadata")
            .await?
            .first()
            .and_then(|message| message.id)
            .ok_or_else(|| MemoryError::NotFound("managed message id".to_string()))?;
        let update = managed_metadata_update(
            "metadata",
            applied.authority.epoch,
            applied.authority.revision,
            "title",
            "summary",
            message_id,
        )?;
        let updated = store.update_managed_conversation(update.clone()).await?;
        assert_eq!(
            updated.status,
            ManagedConversationMetadataUpdateStatus::Updated
        );
        assert_eq!(
            store
                .update_managed_conversation(update.clone())
                .await?
                .status,
            ManagedConversationMetadataUpdateStatus::AlreadyUpdated
        );
        let mut tampered = update.clone();
        tampered.summary = Some("tampered".to_string());
        assert!(store.update_managed_conversation(tampered).await.is_err());
        let stale = store
            .update_managed_conversation(managed_metadata_update(
                "metadata",
                updated.authority.epoch,
                applied.authority.revision,
                "stale",
                "stale",
                message_id,
            )?)
            .await?;
        assert_eq!(
            stale.status,
            ManagedConversationMetadataUpdateStatus::RevisionConflict
        );
        let conversation = store
            .get_conversation("metadata")
            .await?
            .ok_or_else(|| MemoryError::NotFound("managed conversation".to_string()))?;
        assert_eq!(conversation.title.as_deref(), Some("title"));
        assert_eq!(conversation.summary.as_deref(), Some("summary"));
        assert_eq!(conversation.compressed_before_id, Some(message_id));

        let imported = store
            .import_managed_messages(managed_import(
                "metadata",
                updated.authority.epoch,
                updated.authority.revision,
                &["replacement"],
            )?)
            .await?;
        let after_import = store
            .get_conversation("metadata")
            .await?
            .ok_or_else(|| MemoryError::NotFound("conversation after import".to_string()))?;
        assert!(after_import.summary.is_none());
        assert!(after_import.compressed_before_id.is_none());
        assert_eq!(
            store.update_managed_conversation(update).await?.status,
            ManagedConversationMetadataUpdateStatus::EpochConflict
        );
        let replacement_id = store
            .get_messages("metadata")
            .await?
            .first()
            .and_then(|message| message.id)
            .ok_or_else(|| MemoryError::NotFound("imported message id".to_string()))?;
        let after_import = managed_metadata_update(
            "metadata",
            imported.authority.epoch,
            imported.authority.revision,
            "new-title",
            "new-summary",
            replacement_id,
        )?;
        let updated_again = store
            .update_managed_conversation(after_import.clone())
            .await?;
        let deleted = store
            .delete_managed_conversation(managed_delete("metadata", updated_again.authority.epoch)?)
            .await?;
        assert_eq!(deleted.status, ManagedConversationDeleteStatus::Deleted);
        let recreated = store
            .ensure_projection_epoch(ensure_projection_request(
                "metadata",
                Some(updated_again.authority.epoch),
            ))
            .await?;
        assert_eq!(
            store
                .update_managed_conversation(after_import)
                .await?
                .status,
            ManagedConversationMetadataUpdateStatus::EpochConflict
        );
        assert!(is_managed_projection_error(
            store
                .update_conversation("metadata", Some("late"), Some("late"), Some(1))
                .await
        ));
        let conversation = store
            .get_conversation("metadata")
            .await?
            .ok_or_else(|| MemoryError::NotFound("recreated conversation".to_string()))?;
        assert_eq!(recreated.authority.epoch, 3);
        assert!(conversation.title.is_none());
        assert!(conversation.summary.is_none());
        assert!(conversation.compressed_before_id.is_none());
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_delete_receipt_fences_late_effects_and_survives_recreate() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("delete-fence", None))
            .await?;
        let batch = projection_batch("delete-fence", 1, "generation-a", 0, &["before"])?;
        assert_eq!(
            store
                .apply_transcript_projection(batch.clone())
                .await?
                .status,
            TranscriptProjectionApplyStatus::Applied
        );
        let delete = managed_delete("delete-fence", acquired.authority.epoch)?;
        assert_eq!(
            store
                .delete_managed_conversation(delete.clone())
                .await?
                .status,
            ManagedConversationDeleteStatus::Deleted
        );
        assert_eq!(
            store
                .delete_managed_conversation(delete.clone())
                .await?
                .status,
            ManagedConversationDeleteStatus::AlreadyDeleted
        );
        let mut tampered_delete = delete.clone();
        tampered_delete.payload_digest = "different-delete-payload".to_string();
        assert!(
            store
                .delete_managed_conversation(tampered_delete)
                .await
                .is_err()
        );
        assert!(store.get_conversation("delete-fence").await?.is_none());
        assert_eq!(
            store
                .ensure_projection_epoch(ensure_projection_request("delete-fence", None))
                .await?
                .status,
            ConversationProjectionEpochStatus::Tombstoned
        );

        let recreated = store
            .ensure_projection_epoch(ensure_projection_request("delete-fence", Some(1)))
            .await?;
        assert_eq!(
            recreated.status,
            ConversationProjectionEpochStatus::Recreated
        );
        assert_eq!(recreated.authority.epoch, 2);
        assert_eq!(
            store.delete_managed_conversation(delete).await?.status,
            ManagedConversationDeleteStatus::AlreadyDeleted
        );
        assert!(store.get_conversation("delete-fence").await?.is_some());
        assert_eq!(
            store
                .delete_managed_conversation(managed_delete("delete-fence", 3)?)
                .await?
                .status,
            ManagedConversationDeleteStatus::EpochConflict
        );
        assert!(matches!(
            store.apply_transcript_projection(batch).await?.status,
            TranscriptProjectionApplyStatus::Fenced {
                current_epoch: 2,
                ..
            }
        ));
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_generations_merge_without_lost_updates() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("conversations.db");
        let store = SqliteConversationStore::new(&path)?;
        store
            .ensure_projection_epoch(ensure_projection_request("concurrent", None))
            .await?;
        let first_store = SqliteConversationStore::new(&path)?;
        let second_store = SqliteConversationStore::new(&path)?;
        let first = projection_batch("concurrent", 1, "generation-a", 0, &["a"])?;
        let second = projection_batch("concurrent", 1, "generation-b", 0, &["b"])?;
        let (first, second) = tokio::join!(
            first_store.apply_transcript_projection(first),
            second_store.apply_transcript_projection(second),
        );
        assert_eq!(first?.status, TranscriptProjectionApplyStatus::Applied);
        assert_eq!(second?.status, TranscriptProjectionApplyStatus::Applied);
        let mut contents = store
            .get_messages("concurrent")
            .await?
            .into_iter()
            .filter_map(|message| message.content)
            .collect::<Vec<_>>();
        contents.sort();
        assert_eq!(contents, vec!["a", "b"]);
        drop(first_store);
        drop(second_store);
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_transactions_roll_back_before_receipt_publish() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        let acquired = store
            .ensure_projection_epoch(ensure_projection_request("fault", None))
            .await?;
        let batch = projection_batch("fault", acquired.authority.epoch, "generation-a", 0, &["a"])?;

        execute_test_sql(
            &store,
            "CREATE TRIGGER fail_projection_receipt
             BEFORE INSERT ON transcript_projection_receipt
             BEGIN SELECT RAISE(ABORT, 'projection receipt fault'); END;",
        )?;
        assert!(
            store
                .apply_transcript_projection(batch.clone())
                .await
                .is_err()
        );
        execute_test_sql(&store, "DROP TRIGGER fail_projection_receipt;")?;
        assert_eq!(store.count_messages("fault").await?, 0);
        let after_failed_apply = store
            .ensure_projection_epoch(ensure_projection_request("fault", None))
            .await?;
        assert_eq!(after_failed_apply.authority, acquired.authority);
        let applied = store.apply_transcript_projection(batch).await?;
        assert_eq!(applied.status, TranscriptProjectionApplyStatus::Applied);

        let pre_import_message_id = store
            .get_messages("fault")
            .await?
            .first()
            .and_then(|message| message.id)
            .ok_or_else(|| MemoryError::NotFound("pre-import message id".to_string()))?;
        let pre_import_metadata = store
            .update_managed_conversation(managed_metadata_update(
                "fault",
                applied.authority.epoch,
                applied.authority.revision,
                "pre-import-title",
                "pre-import-summary",
                pre_import_message_id,
            )?)
            .await?;
        let import = managed_import(
            "fault",
            pre_import_metadata.authority.epoch,
            pre_import_metadata.authority.revision,
            &["imported"],
        )?;
        execute_test_sql(
            &store,
            "CREATE TRIGGER fail_projection_receipt
             BEFORE INSERT ON transcript_projection_receipt
             BEGIN SELECT RAISE(ABORT, 'import receipt fault'); END;",
        )?;
        assert!(store.import_managed_messages(import.clone()).await.is_err());
        execute_test_sql(&store, "DROP TRIGGER fail_projection_receipt;")?;
        assert_eq!(
            store
                .get_messages("fault")
                .await?
                .first()
                .and_then(|message| message.content.as_deref()),
            Some("a")
        );
        let after_failed_import_metadata = store
            .get_conversation("fault")
            .await?
            .ok_or_else(|| MemoryError::NotFound("conversation after import fault".to_string()))?;
        assert_eq!(
            after_failed_import_metadata.summary.as_deref(),
            Some("pre-import-summary")
        );
        assert_eq!(
            after_failed_import_metadata.compressed_before_id,
            Some(pre_import_message_id)
        );
        let after_failed_import = store
            .ensure_projection_epoch(ensure_projection_request("fault", None))
            .await?;
        assert_eq!(after_failed_import.authority, pre_import_metadata.authority);
        let imported = store.import_managed_messages(import).await?;
        assert_eq!(imported.status, TranscriptProjectionApplyStatus::Applied);
        let after_import_metadata = store
            .get_conversation("fault")
            .await?
            .ok_or_else(|| MemoryError::NotFound("conversation after import".to_string()))?;
        assert!(after_import_metadata.summary.is_none());
        assert!(after_import_metadata.compressed_before_id.is_none());

        let imported_message_id = store
            .get_messages("fault")
            .await?
            .first()
            .and_then(|message| message.id)
            .ok_or_else(|| MemoryError::NotFound("imported message id".to_string()))?;
        let metadata = managed_metadata_update(
            "fault",
            imported.authority.epoch,
            imported.authority.revision,
            "metadata-title",
            "metadata-summary",
            imported_message_id,
        )?;
        execute_test_sql(
            &store,
            "CREATE TRIGGER fail_projection_receipt
             BEFORE INSERT ON transcript_projection_receipt
             BEGIN SELECT RAISE(ABORT, 'metadata receipt fault'); END;",
        )?;
        assert!(
            store
                .update_managed_conversation(metadata.clone())
                .await
                .is_err()
        );
        execute_test_sql(&store, "DROP TRIGGER fail_projection_receipt;")?;
        let after_failed_metadata = store.get_conversation("fault").await?.ok_or_else(|| {
            MemoryError::NotFound("conversation after metadata fault".to_string())
        })?;
        assert_eq!(
            after_failed_metadata.title.as_deref(),
            Some("pre-import-title")
        );
        assert!(after_failed_metadata.summary.is_none());
        assert!(after_failed_metadata.compressed_before_id.is_none());
        let after_failed_metadata_authority = store
            .ensure_projection_epoch(ensure_projection_request("fault", None))
            .await?;
        assert_eq!(
            after_failed_metadata_authority.authority,
            imported.authority
        );
        let metadata_updated = store.update_managed_conversation(metadata).await?;
        assert_eq!(
            metadata_updated.status,
            ManagedConversationMetadataUpdateStatus::Updated
        );

        let delete = managed_delete("fault", metadata_updated.authority.epoch)?;
        execute_test_sql(
            &store,
            "CREATE TRIGGER fail_delete_receipt
             BEFORE INSERT ON managed_conversation_delete_receipt
             BEGIN SELECT RAISE(ABORT, 'delete receipt fault'); END;",
        )?;
        assert!(
            store
                .delete_managed_conversation(delete.clone())
                .await
                .is_err()
        );
        execute_test_sql(&store, "DROP TRIGGER fail_delete_receipt;")?;
        assert!(store.get_conversation("fault").await?.is_some());
        assert_eq!(
            store
                .get_messages("fault")
                .await?
                .first()
                .and_then(|message| message.content.as_deref()),
            Some("imported")
        );
        let after_failed_delete = store
            .ensure_projection_epoch(ensure_projection_request("fault", None))
            .await?;
        assert_eq!(after_failed_delete.authority, metadata_updated.authority);
        assert_eq!(
            store.delete_managed_conversation(delete).await?.status,
            ManagedConversationDeleteStatus::Deleted
        );
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn delete_receipt_retention_floor_prevents_expired_replay() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        store
            .ensure_projection_epoch(ensure_projection_request("retention", None))
            .await?;
        let cycles = DELETE_RECEIPT_RETENTION.saturating_add(1);
        for offset in 0..cycles {
            let epoch = u64::try_from(offset)
                .map_err(|error| {
                    MemoryError::Unsupported(format!("test epoch conversion failed: {error}"))
                })?
                .saturating_add(1);
            let receipt = store
                .delete_managed_conversation(managed_delete("retention", epoch)?)
                .await?;
            assert_eq!(receipt.status, ManagedConversationDeleteStatus::Deleted);
            if offset.saturating_add(1) < cycles {
                let recreated = store
                    .ensure_projection_epoch(ensure_projection_request("retention", Some(epoch)))
                    .await?;
                assert_eq!(
                    recreated.status,
                    ConversationProjectionEpochStatus::Recreated
                );
            }
        }
        let expired = store
            .delete_managed_conversation(managed_delete("retention", 1)?)
            .await?;
        assert_eq!(
            expired.status,
            ManagedConversationDeleteStatus::ReceiptExpired
        );
        assert!(expired.retention_floor_epoch >= 1);
        let last_epoch = u64::try_from(cycles).map_err(|error| {
            MemoryError::Unsupported(format!("test epoch conversion failed: {error}"))
        })?;
        let live = store
            .ensure_projection_epoch(ensure_projection_request("retention", Some(last_epoch)))
            .await?;
        let mut tampered_expired = managed_delete("retention", 1)?;
        tampered_expired.expected_epoch = live.authority.epoch;
        assert!(
            store
                .delete_managed_conversation(tampered_expired)
                .await
                .is_err()
        );
        assert!(store.get_conversation("retention").await?.is_some());
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn maximum_projection_epoch_cannot_be_recreated() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let store = SqliteConversationStore::new(dir.join("conversations.db"))?;
        store
            .ensure_projection_epoch(ensure_projection_request("epoch-max", None))
            .await?;
        {
            let guard = store.conn.lock().map_err(|error| {
                MemoryError::IoError(format!("lock SQLite test connection: {error}"))
            })?;
            guard
                .execute(
                    "UPDATE conversation_projection SET epoch = ?2, lifecycle = 'deleted'
                     WHERE conversation_id = ?1",
                    params![
                        "epoch-max",
                        SqliteConversationStore::sqlite_u64(MAX_PROJECTION_EPOCH, "epoch")?
                    ],
                )
                .map_err(|error| memory_io_error("set maximum test epoch", error))?;
        }
        let result = store
            .ensure_projection_epoch(ensure_projection_request(
                "epoch-max",
                Some(MAX_PROJECTION_EPOCH),
            ))
            .await;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Memory(error))
                if matches!(error.as_ref(), MemoryError::ProjectionEpochExhausted(_))
        ));
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_receipts_survive_store_reopen() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("conversations.db");
        let batch = projection_batch("reopen", 1, "generation-a", 0, &["durable"])?;
        let delete = managed_delete("reopen", 1)?;
        {
            let store = SqliteConversationStore::new(&path)?;
            store
                .ensure_projection_epoch(ensure_projection_request("reopen", None))
                .await?;
            assert_eq!(
                store
                    .apply_transcript_projection(batch.clone())
                    .await?
                    .status,
                TranscriptProjectionApplyStatus::Applied
            );
        }
        {
            let reopened = SqliteConversationStore::new(&path)?;
            assert_eq!(
                reopened.apply_transcript_projection(batch).await?.status,
                TranscriptProjectionApplyStatus::AlreadyApplied
            );
            assert_eq!(
                reopened
                    .delete_managed_conversation(delete.clone())
                    .await?
                    .status,
                ManagedConversationDeleteStatus::Deleted
            );
        }
        let reopened = SqliteConversationStore::new(&path)?;
        assert_eq!(
            reopened.delete_managed_conversation(delete).await?.status,
            ManagedConversationDeleteStatus::AlreadyDeleted
        );
        assert_eq!(
            reopened
                .ensure_projection_epoch(ensure_projection_request("reopen", Some(1)))
                .await?
                .status,
            ConversationProjectionEpochStatus::Recreated
        );
        drop(reopened);
        std::fs::remove_dir_all(dir)?;
        Ok(())
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
