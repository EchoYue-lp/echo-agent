//! File-backed [`ConversationStore`] — a no-dependency JSON-file backend.
//!
//! One file per conversation under `<base>/conversations/<id>.json`, plus a
//! monotonic id counter in `<base>/conversations/_meta.json`. This is the
//! no-SQLite alternative to [`SqliteConversationStore`] (`sqlite` feature).
//!
//! ## Layout
//!
//! - `<base>/conversations/<safe_id>.json` — one conversation + its messages,
//!   written atomically (unique tmp file + fsync + rename + parent-dir sync).
//! - `<base>/conversations/_meta.json` — monotonic id counter, replacing the
//!   SQLite autoincrement. Bumped on every new conversation/message so ids stay
//!   unique across the store; self-heals across restarts.
//!
//! ## Robustness
//!
//! - **Path-safe ids.** Conversation ids are sanitized to a filesystem-safe
//!   segment (rejecting `/`, `\`, `..`) before joining into the path. This
//!   prevents path-traversal (`../foo`) and accidental directory escapes.
//! - **Corrupt JSON is an error.** A malformed record / meta file surfaces as
//!   `MemoryError::SerializationError` rather than silently returning `None` /
//!   an empty list (which previously looked indistinguishable from "no data").
//! - **Unique temp names + parent-dir sync.** Each atomic write uses a
//!   uuid-suffixed temp file (no cross-write collisions) and, on Unix, `fsync`s
//!   the parent directory after rename so the rename survives a crash.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use echo_core::error::{MemoryError, Result};
use echo_core::memory::conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

type BoxFut<'a, T> = BoxFuture<'a, Result<T>>;

/// One conversation record persisted to disk (its `Conversation` header + all
/// its messages). Serialized as `<safe_id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationRecord {
    conversation: Conversation,
    messages: Vec<StoredMessage>,
}

/// Monotonic id counter persisted as `_meta.json`, replacing the SQLite
/// autoincrement. `next_id` is bumped on every new conversation/message so ids
/// stay unique across the store.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoreMeta {
    next_id: i64,
}

impl StoreMeta {
    fn take_id(&mut self) -> i64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }
}

/// File-backed conversation store.
///
/// All operations are serialized by an in-process `Mutex`. Suitable for a
/// single-process local agent (the typical echo-agent consumer). For multi-
/// process concurrency, use the SQLite backend.
pub struct FileConversationStore {
    base: PathBuf,
    lock: Mutex<StoreMeta>,
}

impl FileConversationStore {
    /// Create a file-backed conversation store rooted at `base/conversations/`.
    pub fn new(base: impl AsRef<Path>) -> Result<Self> {
        let base = base.as_ref().join("conversations");
        std::fs::create_dir_all(&base)
            .map_err(|e| MemoryError::IoError(format!("create conversations dir: {e}")))?;
        let meta = Self::read_meta(&base)?;
        Ok(Self {
            base,
            lock: Mutex::new(meta),
        })
    }

    fn conv_path(&self, conversation_id: &str) -> Result<PathBuf> {
        let safe = safe_segment(conversation_id)?;
        Ok(self.base.join(format!("{safe}.json")))
    }

    fn meta_path(base: &Path) -> PathBuf {
        base.join("_meta.json")
    }

    /// Read `_meta.json` and reconcile it with persisted records.
    ///
    /// A record rename can become durable just before the corresponding meta
    /// write during a crash. Scanning the existing records prevents the next
    /// process from reusing an already-persisted conversation/message id.
    fn read_meta(base: &Path) -> Result<StoreMeta> {
        let mut meta = match std::fs::read_to_string(Self::meta_path(base)) {
            Ok(s) => {
                let meta: StoreMeta = serde_json::from_str(&s).map_err(|e| {
                    MemoryError::SerializationError(format!("parse _meta.json: {e}"))
                })?;
                meta
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreMeta::default(),
            Err(e) => return Err(MemoryError::IoError(format!("read _meta.json: {e}")).into()),
        };

        let entries =
            std::fs::read_dir(base).map_err(|e| MemoryError::IoError(format!("readdir: {e}")))?;
        for entry in entries {
            let entry = entry.map_err(|e| MemoryError::IoError(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || path.file_name().and_then(|value| value.to_str()) == Some("_meta.json")
            {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| MemoryError::IoError(format!("read {}: {e}", path.display())))?;
            let record: ConversationRecord = serde_json::from_str(&content).map_err(|e| {
                MemoryError::SerializationError(format!("parse {}: {e}", path.display()))
            })?;
            meta.next_id = meta.next_id.max(record.conversation.id);
            if let Some(message_id) = record
                .messages
                .iter()
                .filter_map(|message| message.id)
                .max()
            {
                meta.next_id = meta.next_id.max(message_id);
            }
        }
        Ok(meta)
    }

    fn persist_meta(&self, meta: &StoreMeta) -> Result<()> {
        let json = serde_json::to_string(meta)
            .map_err(|e| MemoryError::SerializationError(format!("serialize meta: {e}")))?;
        atomic_write(&Self::meta_path(&self.base), json.as_bytes())
            .map_err(|e| MemoryError::IoError(format!("write meta: {e}")))?;
        Ok(())
    }

    /// Read one conversation record. Missing file → `Ok(None)`; corrupt → `Err`.
    fn read_record(&self, conversation_id: &str) -> Result<Option<ConversationRecord>> {
        let path = self.conv_path(conversation_id)?;
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let rec: ConversationRecord = serde_json::from_str(&s).map_err(|e| {
                    MemoryError::SerializationError(format!(
                        "parse conversation {conversation_id}: {e}"
                    ))
                })?;
                Ok(Some(rec))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MemoryError::IoError(format!(
                "read conversation {conversation_id}: {e}"
            ))
            .into()),
        }
    }

    fn write_record(&self, record: &ConversationRecord) -> Result<()> {
        let json = serde_json::to_string_pretty(record)
            .map_err(|e| MemoryError::SerializationError(format!("serialize conversation: {e}")))?;
        let path = self.conv_path(&record.conversation.conversation_id)?;
        atomic_write(&path, json.as_bytes())
            .map_err(|e| MemoryError::IoError(format!("write conversation: {e}")))?;
        Ok(())
    }

    fn create_conversation_locked(
        &self,
        conv: NewConversation,
        meta: &mut StoreMeta,
    ) -> Result<Conversation> {
        if self.read_record(&conv.conversation_id)?.is_some() {
            return Err(MemoryError::IoError(format!(
                "conversation already exists: {}",
                conv.conversation_id
            ))
            .into());
        }
        let id = meta.take_id();
        let now = now_rfc3339();
        let conversation = Conversation {
            id,
            conversation_id: conv.conversation_id,
            user_id: conv.user_id,
            agent_type: conv.agent_type,
            title: conv.title,
            summary: None,
            compressed_before_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let record = ConversationRecord {
            conversation: conversation.clone(),
            messages: Vec::new(),
        };
        self.write_record(&record)?;
        self.persist_meta(meta)?;
        Ok(conversation)
    }

    /// Enumerate all conversation records on disk.
    ///
    /// A single corrupt record surfaces as an error (the previous behavior
    /// silently skipped it, masking data loss).
    fn read_all_records(&self) -> Result<Vec<ConversationRecord>> {
        let mut records = Vec::new();
        let entries = std::fs::read_dir(&self.base)
            .map_err(|e| MemoryError::IoError(format!("readdir: {e}")))?;
        for entry in entries {
            let entry = entry.map_err(|e| MemoryError::IoError(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Skip the meta file.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "_meta.json")
            {
                continue;
            }
            let s = std::fs::read_to_string(&path)
                .map_err(|e| MemoryError::IoError(format!("read {}: {e}", path.display())))?;
            let rec: ConversationRecord = serde_json::from_str(&s).map_err(|e| {
                MemoryError::SerializationError(format!("parse {}: {e}", path.display()))
            })?;
            records.push(rec);
        }
        Ok(records)
    }
}

fn poison<T>(_: std::sync::PoisonError<T>) -> MemoryError {
    MemoryError::IoError("store lock poisoned".into())
}

impl ConversationStore for FileConversationStore {
    fn create_conversation<'a>(&'a self, conv: NewConversation) -> BoxFut<'a, Conversation> {
        Box::pin(async move {
            let mut meta = self.lock.lock().map_err(poison)?;
            self.create_conversation_locked(conv, &mut meta)
        })
    }

    fn get_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFut<'a, Option<Conversation>> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            Ok(self
                .read_record(conversation_id)?
                .map(|record| record.conversation))
        })
    }

    fn list_conversations<'a>(
        &'a self,
        filter: ConversationFilter,
    ) -> BoxFut<'a, Vec<ConversationMeta>> {
        Box::pin(async move {
            let _g = self.lock.lock().map_err(poison)?;
            let mut metas: Vec<ConversationMeta> = self
                .read_all_records()?
                .into_iter()
                .filter(|r| {
                    filter
                        .user_id
                        .as_deref()
                        .is_none_or(|u| r.conversation.user_id == u)
                })
                .filter(|r| {
                    filter
                        .agent_type
                        .as_deref()
                        .is_none_or(|a| r.conversation.agent_type.as_deref() == Some(a))
                })
                .map(|r| ConversationMeta {
                    id: r.conversation.id,
                    conversation_id: r.conversation.conversation_id,
                    user_id: r.conversation.user_id,
                    title: r.conversation.title,
                    message_count: r.messages.len(),
                    created_at: r.conversation.created_at,
                    updated_at: r.conversation.updated_at,
                })
                .collect();
            // ORDER BY updated_at DESC.
            metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            // OFFSET then LIMIT without indexing into the collection.
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(usize::MAX);
            Ok(metas.into_iter().skip(offset).take(limit).collect())
        })
    }

    fn update_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
        title: Option<&'a str>,
        summary: Option<&'a str>,
        compressed_before_id: Option<i64>,
    ) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            let mut record = match self.read_record(conversation_id)? {
                Some(r) => r,
                None => return Ok(()), // matches SQL UPDATE on 0 rows.
            };
            if title.is_some() || summary.is_some() || compressed_before_id.is_some() {
                if let Some(t) = title {
                    record.conversation.title = Some(t.to_string());
                }
                if let Some(s) = summary {
                    record.conversation.summary = Some(s.to_string());
                }
                if let Some(cbid) = compressed_before_id {
                    record.conversation.compressed_before_id = Some(cbid);
                }
                record.conversation.updated_at = now_rfc3339();
                self.write_record(&record)?;
            }
            Ok(())
        })
    }

    fn delete_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            let path = self.conv_path(conversation_id)?;
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(MemoryError::IoError(format!("delete conversation: {e}")).into()),
            }
        })
    }

    fn save_messages<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [StoredMessage],
    ) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let mut meta = self.lock.lock().map_err(poison)?;
            let mut record = self
                .read_record(conversation_id)?
                .ok_or_else(|| MemoryError::NotFound(format!("conversation: {conversation_id}")))?;
            // Assign stable ids to messages that don't have one yet (matches
            // the SQLite autoincrement). Reuse existing ids when present.
            let mut assigned: Vec<StoredMessage> = messages.to_vec();
            for m in assigned.iter_mut() {
                m.conversation_id = conversation_id.to_string();
                match m.id {
                    Some(id) => meta.next_id = meta.next_id.max(id),
                    None => m.id = Some(meta.take_id()),
                }
            }
            record.messages = assigned;
            record.conversation.updated_at = now_rfc3339();
            self.write_record(&record)?;
            self.persist_meta(&meta)?;
            Ok(())
        })
    }

    fn get_messages<'a>(&'a self, conversation_id: &'a str) -> BoxFut<'a, Vec<StoredMessage>> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            Ok(self
                .read_record(conversation_id)?
                .map(|r| r.messages)
                .unwrap_or_default())
        })
    }

    fn count_messages<'a>(&'a self, conversation_id: &'a str) -> BoxFut<'a, usize> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            Ok(self
                .read_record(conversation_id)?
                .map(|r| r.messages.len())
                .unwrap_or(0))
        })
    }

    fn search_conversations<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFut<'a, Vec<ConversationMeta>> {
        Box::pin(async move {
            let _guard = self.lock.lock().map_err(poison)?;
            let needle = query.to_lowercase();
            let mut results: Vec<ConversationMeta> = self
                .read_all_records()?
                .into_iter()
                .filter(|r| {
                    // Match if title OR any message content contains the query
                    // (case-insensitive), mirroring SQL `title LIKE '%q%' OR
                    // m.content LIKE '%q%'`.
                    let title_hit = r
                        .conversation
                        .title
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase().contains(&needle));
                    let msg_hit = r.messages.iter().any(|m| {
                        m.content
                            .as_deref()
                            .is_some_and(|c| c.to_lowercase().contains(&needle))
                    });
                    title_hit || msg_hit
                })
                .map(|r| ConversationMeta {
                    id: r.conversation.id,
                    conversation_id: r.conversation.conversation_id,
                    user_id: r.conversation.user_id,
                    title: r.conversation.title,
                    message_count: r.messages.len(),
                    created_at: r.conversation.created_at,
                    updated_at: r.conversation.updated_at,
                })
                .collect();
            // ORDER BY updated_at DESC, then LIMIT.
            results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            results.truncate(limit);
            Ok(results)
        })
    }

    fn ensure_conversation<'a>(&'a self, conv: NewConversation) -> BoxFut<'a, Conversation> {
        Box::pin(async move {
            let mut meta = self.lock.lock().map_err(poison)?;
            if let Some(existing) = self.read_record(&conv.conversation_id)? {
                return Ok(existing.conversation);
            }
            self.create_conversation_locked(conv, &mut meta)
        })
    }
}

fn now_rfc3339() -> String {
    echo_core::utils::time::now_local().to_rfc3339()
}

/// Sanitize an arbitrary id string into a single safe filesystem segment.
///
/// Rejects empty, path separators (`/`, `\`), the traversal segment `..`, and
/// control characters. This prevents `<conversation_id>.json` from escaping the
/// `conversations/` directory via `../foo` or absolute paths.
///
/// Character-safe (no byte slicing); the allowed set is ASCII so the check is
/// char-boundary-correct by construction.
fn safe_segment(id: &str) -> Result<String> {
    if id.is_empty() {
        return Err(MemoryError::Unsupported("conversation id is empty".into()).into());
    }
    // Reject anything that is not a path-safe token. We allow alphanumerics,
    // `-`, `_`, `.` (but `..` alone is rejected below), and `:` (common in ids).
    for ch in id.chars() {
        let safe = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '~');
        if !safe {
            return Err(MemoryError::Unsupported(format!(
                "conversation id contains unsafe character {ch:?}"
            ))
            .into());
        }
    }
    if id == ".." || id == "." || id.contains('/') || id.contains('\\') {
        return Err(
            MemoryError::Unsupported(format!("conversation id is a path segment: {id:?}")).into(),
        );
    }
    Ok(id.to_string())
}

/// Write `bytes` to `path` atomically: write to a unique temp file, fsync it,
/// then rename into place.
///
/// The temp file is fsynced so the *content* is durable before the rename. On
/// Unix, the parent directory is fsynced as well so the renamed directory entry
/// survives a crash.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("path has no parent: {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    // Unique temp name: avoids collisions across concurrent writes (even though
    // the in-process Mutex serializes this store, multi-process safety is a
    // belt-and-suspenders property).
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("data"),
        uuid::Uuid::new_v4()
    ));
    let write_result = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_base() -> PathBuf {
        std::env::temp_dir().join(format!(
            "echo-file-conv-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn new_conv(id: &str, title: Option<&str>) -> NewConversation {
        NewConversation {
            conversation_id: id.to_string(),
            user_id: "default".to_string(),
            agent_type: None,
            title: title.map(String::from),
        }
    }

    fn stored(role: &str, content: &str) -> StoredMessage {
        StoredMessage {
            id: None,
            conversation_id: "c1".into(),
            role: role.into(),
            content: Some(content.into()),
            attachments_json: None,
            tool_calls_json: None,
            tool_result_json: None,
            created_at: String::new(),
        }
    }

    #[tokio::test]
    async fn conversation_crud_and_search() {
        let base = tmp_base();
        let store = FileConversationStore::new(&base).unwrap();

        store
            .create_conversation(new_conv("c1", Some("rust tokio help")))
            .await
            .unwrap();
        store
            .create_conversation(new_conv("c2", Some("python asyncio")))
            .await
            .unwrap();

        let list = store
            .list_conversations(ConversationFilter {
                limit: Some(10),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(list.len(), 2);

        store
            .save_messages("c1", &[stored("user", "how do I use tokio")])
            .await
            .unwrap();
        let msgs = store.get_messages("c1").await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].id.is_some());

        assert_eq!(store.count_messages("c1").await.unwrap(), 1);

        let found = store.search_conversations("tokio", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].conversation_id, "c1");

        let found = store.search_conversations("python", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].conversation_id, "c2");

        store
            .update_conversation("c1", Some("renamed"), None, None)
            .await
            .unwrap();
        let conv = store.get_conversation("c1").await.unwrap().unwrap();
        assert_eq!(conv.title.as_deref(), Some("renamed"));

        store.delete_conversation("c2").await.unwrap();
        assert!(store.get_conversation("c2").await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn corrupt_record_surfaces_as_error_not_empty() {
        let base = tmp_base();
        let store = FileConversationStore::new(&base).unwrap();
        store
            .create_conversation(new_conv("c1", Some("title")))
            .await
            .unwrap();
        // Corrupt the record file on disk.
        let path = base.join("conversations").join("c1.json");
        std::fs::write(&path, b"{ not valid json").unwrap();

        let err = store.get_conversation("c1").await.unwrap_err();
        assert!(
            matches!(err, echo_core::error::ReactError::Memory(_)),
            "expected a Memory error, got {err:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn path_traversal_id_is_rejected() {
        let base = tmp_base();
        let store = FileConversationStore::new(&base).unwrap();
        let err = store.get_conversation("../escape").await.unwrap_err();
        assert!(matches!(err, echo_core::error::ReactError::Memory(_)));
        // No file was created outside the base dir.
        assert!(!base.parent().unwrap().join("escape.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn round_trip_messages_via_project_and_restore() {
        use crate::memory::{project_messages, restore_messages};
        use echo_core::llm::types::{
            ContentPart, FunctionCall, ImageUrl, Message, MessageContent, Role, ToolCall,
        };

        let base = tmp_base();
        let store = FileConversationStore::new(&base).unwrap();
        store
            .create_conversation(new_conv("c1", Some("rt")))
            .await
            .unwrap();

        let mut multimodal = Message::user("placeholder".to_string());
        multimodal.content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "inspect ".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AA==".to_string(),
                    detail: Some("low".to_string()),
                },
            },
        ]);
        multimodal.reasoning_content = Some("reasoning trace".to_string());
        let original = vec![
            Message::system("be helpful".into()),
            multimodal,
            Message::assistant_with_tools(vec![ToolCall {
                id: "call-1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "search".into(),
                    arguments: r#"{"q":"rust"}"#.to_string(),
                },
            }]),
            Message::tool_result("call-1".into(), "search".into(), "result".into()),
        ];
        let projected = project_messages("c1", &original).unwrap();
        store.save_messages("c1", &projected).await.unwrap();

        let loaded = store.get_messages("c1").await.unwrap();
        let restored = restore_messages(&loaded).unwrap();

        assert_eq!(restored.len(), original.len());
        assert_eq!(restored[0].role, Role::System);
        assert_eq!(restored[1].role, Role::User);
        assert!(matches!(&restored[1].content, MessageContent::Parts(_)));
        assert_eq!(
            restored[1].reasoning_content.as_deref(),
            Some("reasoning trace")
        );
        assert_eq!(restored[2].role, Role::Assistant);
        assert_eq!(restored[2].tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(restored[2].tool_calls.as_ref().unwrap()[0].id, "call-1");
        assert_eq!(
            restored[2].tool_calls.as_ref().unwrap()[0].function.name,
            "search"
        );
        assert_eq!(restored[3].role, Role::Tool);
        assert_eq!(restored[3].tool_call_id.as_deref(), Some("call-1"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn stale_meta_is_reconciled_from_records() {
        let base = tmp_base();
        let first_id = {
            let store = FileConversationStore::new(&base).unwrap();
            let conversation = store
                .create_conversation(new_conv("first", None))
                .await
                .unwrap();
            store
                .save_messages("first", &[stored("user", "hello")])
                .await
                .unwrap();
            conversation.id
        };
        std::fs::write(
            base.join("conversations").join("_meta.json"),
            r#"{"next_id":0}"#,
        )
        .unwrap();

        let reopened = FileConversationStore::new(&base).unwrap();
        let second = reopened
            .create_conversation(new_conv("second", None))
            .await
            .unwrap();
        assert!(second.id > first_id.saturating_add(1));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn supplied_message_ids_advance_the_live_counter() -> Result<()> {
        let base = tmp_base();
        let store = FileConversationStore::new(&base)?;
        store.create_conversation(new_conv("c1", None)).await?;

        let mut imported = stored("user", "imported");
        imported.id = Some(1_000);
        store.save_messages("c1", &[imported]).await?;
        store
            .save_messages("c1", &[stored("assistant", "next")])
            .await?;

        let saved = store.get_messages("c1").await?;
        assert_eq!(saved.first().and_then(|message| message.id), Some(1_001));
        let _ = std::fs::remove_dir_all(&base);
        Ok(())
    }

    #[test]
    fn safe_segment_rejects_traversal_and_accepts_typical_ids() {
        assert!(safe_segment("../x").is_err());
        assert!(safe_segment("a/b").is_err());
        assert!(safe_segment("a\\b").is_err());
        assert!(safe_segment("").is_err());
        assert!(safe_segment("conv-1709000000-abc123").is_ok());
        assert!(safe_segment("2026-07-28T10:00:00Z").is_ok());
    }
}
