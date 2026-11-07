use crate::llm::types::{Message, ToolDefinition};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Cache-relevant request fingerprints recorded with one model call.
///
/// `stable_prefix_hash` covers the full provider-cacheable prefix, including
/// conversation history. Component hashes isolate changes in the stable
/// system/canonical prefix and tool schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCacheFingerprint {
    pub stable_prefix_hash: String,
    pub system_prefix_hash: String,
    pub tools_schema_hash: String,
    pub history_hash: String,
    pub stable_prefix_message_count: usize,
    pub tool_count: usize,
}

/// 计算稳定前缀的 SHA-256 哈希（canonical 序列化），用于日志观测缓存失效。
///
/// 关键：必须跨进程可复现，所以：
/// 1. 用 SHA-256 而非 `DefaultHasher`（后者跨进程/跨版本不稳）
/// 2. tools schema uses the shared recursive canonical JSON encoder, avoiding
///    map-order differences across processes and feature combinations
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
    result
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Build canonical component fingerprints for cache diagnostics.
pub fn prompt_cache_fingerprint(
    system: &[Message],
    canonical: &[Message],
    tools: &[ToolDefinition],
    history: &[Message],
) -> PromptCacheFingerprint {
    let mut stable_prefix = Sha256::new();
    let mut system_prefix = Sha256::new();
    let mut tools_schema = Sha256::new();
    let mut history_hash = Sha256::new();

    for message in system.iter().chain(canonical.iter()) {
        hash_message(&mut stable_prefix, message);
        hash_message(&mut system_prefix, message);
    }
    for tool in tools {
        hash_tool(&mut stable_prefix, tool);
        hash_tool(&mut tools_schema, tool);
    }
    for message in history {
        hash_message(&mut stable_prefix, message);
        hash_message(&mut history_hash, message);
    }

    PromptCacheFingerprint {
        stable_prefix_hash: finish_hash(stable_prefix),
        system_prefix_hash: finish_hash(system_prefix),
        tools_schema_hash: finish_hash(tools_schema),
        history_hash: finish_hash(history_hash),
        stable_prefix_message_count: system
            .len()
            .saturating_add(canonical.len())
            .saturating_add(history.len()),
        tool_count: tools.len(),
    }
}

fn finish_hash(hasher: Sha256) -> String {
    let result = hasher.finalize();
    result
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    let canonical = crate::utils::canonical_json::canonical_json_bytes(&t.function.parameters)
        .unwrap_or_default();
    hasher.update(canonical);
    hasher.update(b"\n");
}

/// 协议适配前 canonical 消息的完整指纹。
///
/// 与 `PromptCacheFingerprint` 的区别：这里覆盖**全部模型可见字段**——tool
/// description/strict、content parts（含图片 detail 与文件）、tool calls 与
/// tool result 配对、需要回传的 reasoning（含签名/不透明块）。粗 hash 通过而
/// canonical 内容变化由它和 `prompt_prefix_diff` 定位；真正的协议后证据使用
/// [`super::wire`]，不能将本层摘要当作实际 HTTP prompt 的证明。
///
/// 只输出摘要与路径，绝不写正文：生产日志因此可以安全记录。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullPromptFingerprint {
    /// 整段模型可见 prompt（tools + 全部消息）的 SHA-256 前 16 hex。
    pub prompt_hash: String,
    /// tools 段（name/description/parameters/strict）的摘要。
    pub tools_schema_hash: String,
    /// system 段（含 canonical 重注）的摘要。
    pub system_hash: String,
    /// 非 system 历史（含不可变旧动态版本）的摘要。
    pub history_hash: String,
    /// 尾部追加式动态事实的摘要。
    pub dynamic_hash: String,
    pub message_count: usize,
    pub tool_count: usize,
}

/// 两段 prompt 的公共前缀度量与首次差异定位。
///
/// `common_prefix_bytes` 是**规范化 JSON 字节数**，不是 token 数；没有准确
/// tokenizer 时不得把它叫作 token。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPrefixDiff {
    /// 逐条完全相同的消息数。
    pub common_messages: usize,
    /// 公共前缀的规范化 JSON 字节数。
    pub common_prefix_bytes: usize,
    /// 首个差异字段路径；纯追加时为 `None`（追加不是缓存破坏）。
    pub first_difference: Option<PromptFieldDifference>,
    /// 本次是否只是在已见历史之后追加（合法增长）。
    pub is_append_only: bool,
}

/// 首个差异的定位结果，只含下标与字段路径，不含内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFieldDifference {
    pub message_index: usize,
    /// 例如 `messages[3].tool_calls[0].function.arguments`。
    pub path: String,
}

/// 计算完整模型可见 prompt 的指纹。
pub fn full_prompt_fingerprint(
    messages: &[Message],
    tools: &[ToolDefinition],
) -> FullPromptFingerprint {
    let mut tools_hasher = Sha256::new();
    for tool in tools {
        hash_wire_value(
            &mut tools_hasher,
            b"TOOL:",
            serde_json::to_value(tool).ok().as_ref(),
        );
    }

    let mut system_hasher = Sha256::new();
    let mut history_hasher = Sha256::new();
    let mut dynamic_hasher = Sha256::new();
    let mut prompt_hasher = Sha256::new();

    // tools 属于模型可见前缀的一部分：prompt_hash 必须把它们算进去。
    for tool in tools {
        hash_wire_value(
            &mut prompt_hasher,
            b"TOOL:",
            serde_json::to_value(tool).ok().as_ref(),
        );
    }

    for message in messages {
        let value = wire_message_value(message);
        hash_wire_value(&mut prompt_hasher, b"MSG:", value.as_ref());
        if message.role == crate::llm::types::Role::System {
            hash_wire_value(&mut system_hasher, b"MSG:", value.as_ref());
        } else if crate::llm::cache::layout::is_dynamic_tail_message(message) {
            hash_wire_value(&mut dynamic_hasher, b"MSG:", value.as_ref());
        } else {
            hash_wire_value(&mut history_hasher, b"MSG:", value.as_ref());
        }
    }

    FullPromptFingerprint {
        prompt_hash: finish_hash(prompt_hasher),
        tools_schema_hash: finish_hash(tools_hasher),
        system_hash: finish_hash(system_hasher),
        history_hash: finish_hash(history_hasher),
        dynamic_hash: finish_hash(dynamic_hasher),
        message_count: messages.len(),
        tool_count: tools.len(),
    }
}

/// 比较两段 prompt 的消息序列：定位首个语义差异，并区分"合法追加"与"前缀破坏"。
///
/// 数组顺序不重排：参数、parts、tool calls 的顺序都属于语义。
pub fn prompt_prefix_diff(previous: &[Message], current: &[Message]) -> PromptPrefixDiff {
    let mut common_messages = 0usize;
    let mut common_prefix_bytes = 0usize;
    let mut first_difference = None;

    for (index, previous_message) in previous.iter().enumerate() {
        let Some(current_message) = current.get(index) else {
            first_difference = Some(PromptFieldDifference {
                message_index: index,
                path: "messages[length]".to_string(),
            });
            break;
        };
        let previous_value = wire_message_value(previous_message);
        let current_value = wire_message_value(current_message);
        if previous_value == current_value {
            common_messages += 1;
            if let Some(value) = previous_value.as_ref() {
                common_prefix_bytes =
                    common_prefix_bytes.saturating_add(canonical_value_bytes(value).len());
            }
            continue;
        }
        let mut path = format!("messages[{index}]");
        let mut difference = None;
        if let (Some(previous_value), Some(current_value)) = (&previous_value, &current_value) {
            first_field_difference(previous_value, current_value, &mut path, 0, &mut difference);
        }
        first_difference = Some(PromptFieldDifference {
            message_index: index,
            path: difference.unwrap_or(path),
        });
        break;
    }

    let is_append_only = first_difference.is_none() && current.len() >= previous.len();
    PromptPrefixDiff {
        common_messages,
        common_prefix_bytes,
        first_difference,
        is_append_only,
    }
}

/// 模型可见的消息视图：去掉运行期治理字段。
fn wire_message_value(message: &Message) -> Option<serde_json::Value> {
    let mut value = serde_json::to_value(message).ok()?;
    if let Some(object) = value.as_object_mut() {
        object.remove("execution_status");
        object.remove("approved_assistant_blocks");
    }
    Some(value)
}

fn hash_wire_value(hasher: &mut Sha256, prefix: &[u8], value: Option<&serde_json::Value>) {
    hasher.update(prefix);
    if let Some(value) = value {
        hasher.update(canonical_value_bytes(value));
    }
    hasher.update(b"\n");
}

fn canonical_value_bytes(value: &serde_json::Value) -> Vec<u8> {
    crate::utils::canonical_json::canonical_json_bytes(value).unwrap_or_default()
}

/// 深度有界的字段级首差异定位，只返回字段路径。
fn first_field_difference(
    previous: &serde_json::Value,
    current: &serde_json::Value,
    path: &mut String,
    depth: usize,
    out: &mut Option<String>,
) {
    if out.is_some() || depth > 8 || previous == current {
        return;
    }
    match (previous, current) {
        (serde_json::Value::Object(previous_map), serde_json::Value::Object(current_map)) => {
            let mut keys: Vec<&String> = previous_map.keys().chain(current_map.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                let previous_value = previous_map.get(key);
                let current_value = current_map.get(key);
                let base_len = path.len();
                path.push('.');
                path.push_str(key);
                match (previous_value, current_value) {
                    (Some(previous_value), Some(current_value)) => {
                        first_field_difference(previous_value, current_value, path, depth + 1, out);
                    }
                    _ => {
                        if out.is_none() {
                            *out = Some(path.clone());
                        }
                    }
                }
                path.truncate(base_len);
                if out.is_some() {
                    return;
                }
            }
        }
        (serde_json::Value::Array(previous_items), serde_json::Value::Array(current_items)) => {
            let max = previous_items.len().max(current_items.len());
            for index in 0..max {
                let base_len = path.len();
                path.push_str(&format!("[{index}]"));
                match (previous_items.get(index), current_items.get(index)) {
                    (Some(previous_item), Some(current_item)) => {
                        first_field_difference(previous_item, current_item, path, depth + 1, out);
                    }
                    _ => {
                        if out.is_none() {
                            *out = Some(path.clone());
                        }
                    }
                }
                path.truncate(base_len);
                if out.is_some() {
                    return;
                }
            }
        }
        _ => {
            if out.is_none() {
                *out = Some(path.clone());
            }
        }
    }
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
    fn component_hashes_isolate_history_changes() {
        let system = &[Message::system("stable".to_string())];
        let first = prompt_cache_fingerprint(system, &[], &[], &[Message::user("one".to_string())]);
        let second = prompt_cache_fingerprint(
            system,
            &[],
            &[],
            &[
                Message::user("one".to_string()),
                Message::user("two".to_string()),
            ],
        );

        assert_eq!(first.system_prefix_hash, second.system_prefix_hash);
        assert_eq!(first.tools_schema_hash, second.tools_schema_hash);
        assert_ne!(first.history_hash, second.history_hash);
        assert_ne!(first.stable_prefix_hash, second.stable_prefix_hash);
    }

    #[test]
    fn hash_ignores_order_in_tools_schema() -> Result<(), serde_json::Error> {
        // 两个 key 顺序不同但内容相同的 JSON 应产生相同 hash
        let v1: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#)?;
        let v2: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#)?;
        assert_eq!(
            crate::utils::canonical_json::canonical_json_bytes(&v1)?,
            crate::utils::canonical_json::canonical_json_bytes(&v2)?
        );
        Ok(())
    }

    // ── Full prompt fingerprint / prefix diff ──────────────────────────

    fn tool(name: &str, description: &str, strict: Option<bool>) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::llm::types::FunctionSpec {
                name: name.to_string(),
                description: description.to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                strict,
            },
        }
    }

    #[test]
    fn full_fingerprint_detects_tool_description_change_missed_by_rough_hash() {
        let messages = vec![Message::user("hello".to_string())];
        let before = vec![tool("read", "Read a file", None)];
        let after = vec![tool("read", "Read a file or directory", None)];
        // 旧粗 hash 只覆盖 name+parameters，看不到 description 变化。
        assert_eq!(
            prompt_cache_fingerprint(&[], &[], &before, &messages).tools_schema_hash,
            prompt_cache_fingerprint(&[], &[], &after, &messages).tools_schema_hash
        );
        assert_ne!(
            full_prompt_fingerprint(&messages, &before).tools_schema_hash,
            full_prompt_fingerprint(&messages, &after).tools_schema_hash
        );
    }

    #[test]
    fn full_fingerprint_detects_strict_and_parts_and_tool_call_and_reasoning() {
        let base = full_prompt_fingerprint(&[Message::user("hi".to_string())], &[]);

        let strict_variant = full_prompt_fingerprint(
            &[Message::user("hi".to_string())],
            &[tool("read", "Read", Some(true))],
        );
        assert_ne!(base.prompt_hash, strict_variant.prompt_hash);

        let multimodal = Message::user_with_image("hi", "image/png", "AAAA");
        assert_ne!(
            base.prompt_hash,
            full_prompt_fingerprint(std::slice::from_ref(&multimodal), &[]).prompt_hash
        );
        let mut detail_changed = multimodal.clone();
        if let crate::llm::types::MessageContent::Parts(parts) = &mut detail_changed.content
            && let Some(crate::llm::types::ContentPart::ImageUrl { image_url }) = parts.get_mut(1)
        {
            image_url.detail = Some("high".to_string());
        }
        assert_ne!(
            full_prompt_fingerprint(&[multimodal], &[]).prompt_hash,
            full_prompt_fingerprint(&[detail_changed], &[]).prompt_hash
        );

        let call = Message::assistant_with_tools(vec![crate::llm::types::ToolCall {
            provider_source: None,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: crate::llm::types::FunctionCall {
                name: "read".to_string(),
                arguments: "{\"path\":\"a\"}".to_string(),
            },
        }]);
        let mut changed_call = call.clone();
        if let Some(calls) = changed_call.tool_calls.as_mut()
            && let Some(first) = calls.first_mut()
        {
            first.function.arguments = "{\"path\":\"b\"}".to_string();
        }
        assert_ne!(
            full_prompt_fingerprint(&[call], &[]).prompt_hash,
            full_prompt_fingerprint(&[changed_call], &[]).prompt_hash
        );

        let mut reasoned = Message::assistant("answer".to_string());
        reasoned.reasoning_content = Some("step".to_string());
        let mut reasoned_changed = reasoned.clone();
        reasoned_changed.reasoning_content = Some("step 2".to_string());
        assert_ne!(
            full_prompt_fingerprint(&[reasoned], &[]).prompt_hash,
            full_prompt_fingerprint(&[reasoned_changed], &[]).prompt_hash
        );
    }

    #[test]
    fn prefix_diff_locates_first_changed_field_path() {
        let previous = vec![
            Message::system("stable".to_string()),
            Message::user("question".to_string()),
            Message::assistant_with_tools(vec![crate::llm::types::ToolCall {
                provider_source: None,
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: crate::llm::types::FunctionCall {
                    name: "read".to_string(),
                    arguments: "{\"path\":\"a\"}".to_string(),
                },
            }]),
        ];
        let mut current = previous.clone();
        if let Some(message) = current.get_mut(2)
            && let Some(calls) = message.tool_calls.as_mut()
            && let Some(first) = calls.first_mut()
        {
            first.function.arguments = "{\"path\":\"b\"}".to_string();
        }
        let diff = prompt_prefix_diff(&previous, &current);
        assert_eq!(diff.common_messages, 2);
        assert!(!diff.is_append_only);
        assert_eq!(
            diff.first_difference.as_ref().map(|d| d.message_index),
            Some(2)
        );
        assert_eq!(
            diff.first_difference.as_ref().map(|d| d.path.as_str()),
            Some("messages[2].tool_calls[0].function.arguments")
        );
    }

    #[test]
    fn prefix_diff_treats_append_only_growth_as_not_a_break() {
        let previous = vec![
            Message::system("stable".to_string()),
            Message::user("question".to_string()),
        ];
        let mut current = previous.clone();
        current.push(Message::assistant("answer".to_string()));
        let diff = prompt_prefix_diff(&previous, &current);
        assert_eq!(diff.common_messages, 2);
        assert!(diff.first_difference.is_none());
        assert!(diff.is_append_only);
        assert!(diff.common_prefix_bytes > 0);
    }
}
