//! Agent 记忆工具：remember / recall / forget
//!
//! 使用 LangGraph 对齐的 [`Store`] API 实现持久化长期记忆。
//!
//! | 工具       | 对应 Store 操作                              |
//! |------------|---------------------------------------------|
//! | `remember` | `store.put(namespace, uuid, value)`          |
//! | `recall`   | `store.search(namespace, query, limit)`      |
//! | `forget`   | `store.delete(namespace, key)`              |

use crate::error::ToolError;
use crate::memory::store::{Store, StoreItem};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

// ── RememberTool ─────────────────────────────────────────────────────────────

/// 将重要信息存入持久化 Store
///
/// 内部调用 `store.put(namespace, uuid, {"content": ..., "importance": ..., "tags": [...]})`
pub struct RememberTool {
    pub store: Arc<dyn Store>,
    /// 存储命名空间，如 `["alice", "memories"]`
    pub namespace: Vec<String>,
}

impl RememberTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

#[async_trait::async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "将值得长期保留的信息存入持久记忆库（跨会话保存）。\
         适合记录用户偏好、重要结论、待办事项、关键事实等内容。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要记住的具体内容，请简洁、完整地描述"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "标签列表，用于分类检索（可选），例如 [\"偏好\", \"编程\"]"
                },
                "importance": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "重要程度（1-10），默认 5；越高越优先被召回"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let content = parameters
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

        let tags: Vec<String> = parameters
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let importance = parameters
            .get("importance")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 10))
            .unwrap_or(5);

        let key = uuid::Uuid::new_v4().to_string();
        let value = json!({
            "content": content,
            "importance": importance,
            "tags": tags,
        });

        debug!(key = %key, importance = importance, "💡 remember 工具写入 Store");

        let ns: Vec<&str> = self.ns_refs();
        self.store.put(&ns, &key, value).await?;

        let tag_str = if tags.is_empty() {
            String::new()
        } else {
            format!("（标签：{}）", tags.join(", "))
        };

        Ok(ToolResult::success(format!(
            "✅ 已记住（ID: {}，重要程度: {}）：\"{}\"{tag_str}",
            key.get(..8).unwrap_or(&key),
            importance,
            content,
        )))
    }
}

// ── RecallTool ───────────────────────────────────────────────────────────────

/// 从持久化 Store 中检索相关历史记忆
///
/// 内部调用 `store.search(namespace, query, limit)`
pub struct RecallTool {
    pub store: Arc<dyn Store>,
    pub namespace: Vec<String>,
}

impl RecallTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

#[async_trait::async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        "在持久记忆库中搜索相关历史记忆，返回最匹配的若干条。\
         可用关键词、主题或自然语言片段进行搜索。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词或描述，例如 \"用户偏好\" 或 \"上次提到的项目名称\""
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "最多返回条数（默认 5）"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let query = parameters
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

        let limit = parameters
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 20) as usize)
            .unwrap_or(5);

        debug!(query = %query, limit = limit, "🔍 recall 工具查询 Store");

        let ns: Vec<&str> = self.ns_refs();
        let items = self.store.search(&ns, query, limit).await?;

        if items.is_empty() {
            return Ok(ToolResult::success(format!(
                "未找到与「{}」相关的记忆。",
                query
            )));
        }

        let mut lines = vec![format!("找到 {} 条相关记忆：", items.len())];
        for (i, item) in items.iter().enumerate() {
            lines.push(format!(
                "{}. [ID:{}] {}",
                i + 1,
                item.key.get(..8).unwrap_or(&item.key),
                format_store_item(item),
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ── ForgetTool ───────────────────────────────────────────────────────────────

/// 根据记忆 ID（key）删除一条记忆，或清空命名空间下所有记忆
///
/// 内部调用 `store.delete(namespace, key)`
pub struct ForgetTool {
    pub store: Arc<dyn Store>,
    pub namespace: Vec<String>,
}

impl ForgetTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

#[async_trait::async_trait]
impl Tool for ForgetTool {
    fn name(&self) -> &str {
        "forget"
    }

    fn description(&self) -> &str {
        "删除指定 ID 的记忆条目。ID 可通过 recall 工具返回结果中获取（取前8位即可）。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "要删除的记忆 ID（通过 recall 获取前8位前缀即可）"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let id_prefix = parameters
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("id".to_string()))?;

        let ns: Vec<&str> = self.ns_refs();

        // 先尝试精确匹配，如失败则按前缀搜索全 key
        let full_key = self.store.get(&ns, id_prefix).await?.map(|item| item.key);

        // 尝试直接删除（用户可能传入了完整 key）
        let deleted = if let Some(key) = &full_key {
            self.store.delete(&ns, key).await?
        } else {
            // 假设用户传入的就是完整 key（UUID 格式）
            self.store.delete(&ns, id_prefix).await?
        };

        if deleted {
            Ok(ToolResult::success(format!(
                "🗑️ 已删除记忆 ID: {}",
                id_prefix
            )))
        } else {
            Ok(ToolResult::success(format!(
                "未找到 ID 为「{}」的记忆条目，无需删除。\n提示：请通过 recall 工具查找正确的 ID。",
                id_prefix
            )))
        }
    }
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

fn format_store_item(item: &StoreItem) -> String {
    match &item.value {
        Value::Object(map) => {
            let content = map
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("(无内容)");
            let importance = map.get("importance").and_then(|v| v.as_u64());
            let tags = map
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());

            let mut parts = vec![content.to_string()];
            if let Some(imp) = importance {
                parts.push(format!("[★{}]", imp));
            }
            if let Some(t) = tags {
                parts.push(format!("[{}]", t));
            }
            parts.join(" ")
        }
        other => other.to_string(),
    }
}
