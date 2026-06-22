use crate::llm::types::{Message, ToolDefinition};
use sha2::{Digest, Sha256};

/// 计算稳定前缀的 SHA-256 哈希（canonical 序列化），用于日志观测缓存失效。
///
/// 关键：必须跨进程可复现，所以：
/// 1. 用 SHA-256 而非 `DefaultHasher`（后者跨进程/跨版本不稳）
/// 2. tools schema 用 canonical JSON（`serde_json::to_string` 对 `Value::Object`
///    内部使用 `BTreeMap`，默认 sorted keys），避免 key 顺序不确定
/// 3. 只 hash 稳定段（system + canonical + tools + history），不含 runtime_context
pub fn stable_prefix_hash(
    system: &[Message],
    canonical: &[Message],
    tools: &[ToolDefinition],
    history: &[Message],
) -> String {
    let mut hasher = Sha256::new();

    for m in system {
        hash_message(&mut hasher, m);
    }
    for m in canonical {
        hash_message(&mut hasher, m);
    }
    for t in tools {
        hash_tool(&mut hasher, t);
    }
    for m in history {
        hash_message(&mut hasher, m);
    }

    let result = hasher.finalize();
    // 16 位 hex 前缀足够诊断用，日志可读
    let hex: String = result[..8].iter().map(|b| format!("{:02x}", b)).collect();
    hex
}

fn hash_message(hasher: &mut Sha256, m: &Message) {
    hasher.update(b"MSG:");
    hasher.update(m.role.as_str().as_bytes());
    hasher.update(b":");
    if let Some(text) = m.content.as_text() {
        hasher.update(text.as_bytes());
    }
    hasher.update(b"\n");
}

fn hash_tool(hasher: &mut Sha256, t: &ToolDefinition) {
    hasher.update(b"TOOL:");
    hasher.update(t.function.name.as_bytes());
    hasher.update(b":");
    // canonical JSON：sorted keys，确保跨进程一致
    let canonical = canonical_json_string(&t.function.parameters);
    hasher.update(canonical.as_bytes());
    hasher.update(b"\n");
}

fn canonical_json_string(v: &serde_json::Value) -> String {
    // `serde_json::to_string` 对 `Value::Object` 内部使用 `BTreeMap`
    // （feature "preserve_order" 关闭时），已 sorted keys。
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_across_calls() {
        let h1 = stable_prefix_hash(&[], &[], &[], &[]);
        let h2 = stable_prefix_hash(&[], &[], &[], &[]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_when_history_grows() {
        let sys = &[Message::system("S".to_string())];
        let h1 = stable_prefix_hash(sys, &[], &[], &[Message::user("H1".to_string())]);
        let h2 = stable_prefix_hash(
            sys,
            &[],
            &[],
            &[
                Message::user("H1".to_string()),
                Message::user("H2".to_string()),
            ],
        );
        // history 增长导致 hash 变化（符合预期：新消息进入前缀）
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_ignores_order_in_tools_schema() {
        // 两个 key 顺序不同但内容相同的 JSON 应产生相同 hash
        let v1: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        let v2: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        // Value::Object 默认 BTreeMap，已排序
        assert_eq!(canonical_json_string(&v1), canonical_json_string(&v2));
    }
}
