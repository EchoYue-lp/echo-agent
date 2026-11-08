//! 在协议转换完成后诊断实际 prompt；不保留或记录正文。
//!
//! 精确差异比较只接收调用者持有的合成/脱敏请求，不建立跨会话的全局请求缓存。
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WirePromptFingerprint {
    /// 实际 body 的规范 JSON 摘要；与语义摘要分开，保留 shortcut/缓存注记等表示差异。
    pub wire_representation_hash: String,
    pub prompt_hash: String,
    pub tools_hash: String,
    pub system_hash: String,
    pub history_hash: String,
    pub cache_controls_hash: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WirePromptDiff {
    pub is_append_only: bool,
    pub common_messages: usize,
    /// 完整相同组件的规范 JSON 字节数；不是 token，也不是原始 HTTP 字节 LCP。
    pub common_component_bytes: usize,
    /// 路径使用对象成员序号，避免把用户控制的 JSON key 写入日志。
    pub first_difference: Option<String>,
}

fn bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    crate::utils::canonical_json::canonical_json_bytes(value)
}

fn digest(value: &Value) -> Result<String, serde_json::Error> {
    Ok(Sha256::digest(bytes(value)?)
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// 拆出三协议的实际语义序列，数组保持原顺序。缓存注记单独诊断，移动断点不会
/// 被误报成模型正文重写；其它请求参数仍进入 controls，变化视为 generation 更新。
fn prompt(body: &Value) -> (Value, Value, Value, Vec<Value>, Value) {
    let mut controls = body.clone();
    let mut tools = Value::Null;
    let mut system = Value::Null;
    let mut messages = Vec::new();
    let mut cache_controls = Vec::new();
    if let Some(object) = controls.as_object_mut() {
        tools = object.remove("tools").unwrap_or(Value::Null);
        system = object
            .remove("system")
            .or_else(|| object.remove("instructions"))
            .unwrap_or(Value::Null);
        let history = object
            .remove("messages")
            .or_else(|| object.remove("input"))
            .unwrap_or(Value::Null);
        messages = match history {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            other => vec![other],
        };
        // 这些字段不属于模型输入内容；只在顶层移除，不碰工具参数或原生 IDs。
        for key in [
            "stream",
            "stream_options",
            "user",
            "user_id",
            "metadata",
            "prompt_cache_key",
            "store",
        ] {
            object.remove(key);
        }
    }
    strip_cache_controls(&mut tools, "tools", &mut cache_controls);
    strip_cache_controls(&mut system, "system", &mut cache_controls);
    for (index, message) in messages.iter_mut().enumerate() {
        strip_cache_controls(message, &format!("messages[{index}]"), &mut cache_controls);
        // Anthropic 官方 SDK 合同：message.content 字符串是单个 Text block 的
        // 简写。打缓存断点会在这两种表示间转换，规范化只发生于消息自身边界。
        // 不压平 parts，也不递归进入工具 input/schema 或原厂不透明 reasoning。
        if message.get("role").and_then(Value::as_str).is_some()
            && let Some(content) = message.get_mut("content")
            && let Value::String(text) = content
        {
            let text = std::mem::take(text);
            *content = serde_json::json!([{"type":"text","text":text}]);
        }
    }
    (
        tools,
        system,
        controls,
        messages,
        Value::Array(cache_controls),
    )
}

fn strip_cache_controls(value: &mut Value, path: &str, found: &mut Vec<Value>) {
    // 只沿协议的 block 容器前进。工具 input_schema、tool_use.input 和其它任意
    // 用户 JSON 里的同名字段仍是模型语义，不能递归剥除。
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                strip_cache_controls(item, &format!("{path}[{index}]"), found);
            }
        }
        Value::Object(object) => {
            if let Some(control) = object.remove("cache_control") {
                found.push(serde_json::json!({"path":path,"control":control}));
            }
            if let Some(Value::Array(content)) = object.get_mut("content") {
                for (index, block) in content.iter_mut().enumerate() {
                    strip_cache_controls(block, &format!("{path}.content[{index}]"), found);
                }
            }
        }
        _ => {}
    }
}

pub fn wire_prompt_fingerprint(body: &Value) -> Result<WirePromptFingerprint, serde_json::Error> {
    let (tools, system, controls, messages, cache_controls) = prompt(body);
    // Chat/Responses 的指令在消息序列内；独立摘要必须覆盖它们，同时保持
    // 完整 prompt 中的原始位置和顺序不动。
    let message_instructions: Vec<_> = messages
        .iter()
        .filter(|message| {
            matches!(
                message.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            )
        })
        .collect();
    Ok(WirePromptFingerprint {
        wire_representation_hash: digest(body)?,
        prompt_hash: digest(&serde_json::json!([tools, system, controls, messages]))?,
        tools_hash: digest(&tools)?,
        system_hash: digest(&serde_json::json!([system, message_instructions]))?,
        history_hash: digest(&serde_json::json!(messages))?,
        cache_controls_hash: digest(&cache_controls)?,
        message_count: messages.len(),
    })
}

/// 生产默认只记录固定长度摘要。关闭 debug 时不复制完整请求，headers 从不入参。
pub fn log_wire_prompt(body: &Value, protocol: &'static str) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    if let Ok(summary) = wire_prompt_fingerprint(body) {
        tracing::debug!(
            protocol,
            wire_representation_hash = %summary.wire_representation_hash,
            prompt_hash = %summary.prompt_hash,
            tools_hash = %summary.tools_hash,
            system_hash = %summary.system_hash,
            history_hash = %summary.history_hash,
            cache_controls_hash = %summary.cache_controls_hash,
            message_count = summary.message_count,
            "Post-adapter prompt fingerprint"
        );
    }
}

/// 对真实 adapter 输出作隔离对照；正常历史增长不算缓存破坏。
pub fn wire_prompt_prefix_diff(
    previous: &Value,
    current: &Value,
) -> Result<WirePromptDiff, serde_json::Error> {
    let (pt, ps, pc, pm, _) = prompt(previous);
    let (ct, cs, cc, cm, _) = prompt(current);
    let mut result = WirePromptDiff {
        is_append_only: false,
        common_messages: 0,
        common_component_bytes: 0,
        first_difference: None,
    };
    for (name, previous, current) in [("tools", pt, ct), ("system", ps, cs), ("controls", pc, cc)] {
        if previous != current {
            result.first_difference = Some(first_difference(&previous, &current, name, 0));
            return Ok(result);
        }
        result.common_component_bytes = result
            .common_component_bytes
            .saturating_add(bytes(&previous)?.len());
    }
    for (index, previous) in pm.iter().enumerate() {
        let Some(current) = cm.get(index) else {
            result.first_difference = Some("messages.length".into());
            return Ok(result);
        };
        if previous != current {
            result.first_difference = Some(first_difference(
                previous,
                current,
                &format!("messages[{index}]"),
                0,
            ));
            return Ok(result);
        }
        result.common_messages = result.common_messages.saturating_add(1);
        result.common_component_bytes = result
            .common_component_bytes
            .saturating_add(bytes(previous)?.len());
    }
    result.is_append_only = true;
    Ok(result)
}

fn first_difference(previous: &Value, current: &Value, path: &str, depth: usize) -> String {
    if depth >= 12 {
        return path.into();
    }
    match (previous, current) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<_> = left.keys().chain(right.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for (index, key) in keys.into_iter().enumerate() {
                let next = format!("{path}.member[{index}]");
                match (left.get(key), right.get(key)) {
                    (Some(a), Some(b)) if a != b => {
                        return first_difference(a, b, &next, depth.saturating_add(1));
                    }
                    (None, _) | (_, None) => return next,
                    _ => {}
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for (index, a) in left.iter().enumerate() {
                let next = format!("{path}[{index}]");
                match right.get(index) {
                    Some(b) if a != b => {
                        return first_difference(a, b, &next, depth.saturating_add(1));
                    }
                    None => return next,
                    _ => {}
                }
            }
            if left.len() != right.len() {
                return format!("{path}.length");
            }
        }
        _ => {}
    }
    path.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_message_text_shortcuts_are_semantically_equivalent() -> Result<(), serde_json::Error> {
        let shortcut = json!({"messages":[{"role":"user","content":"same"}]});
        let blocks = json!({"messages":[{"role":"user","content":[{"type":"text","text":"same","cache_control":{"type":"ephemeral"}}]}]});
        assert_eq!(
            wire_prompt_fingerprint(&shortcut)?.prompt_hash,
            wire_prompt_fingerprint(&blocks)?.prompt_hash
        );
        assert_ne!(
            wire_prompt_fingerprint(&shortcut)?.wire_representation_hash,
            wire_prompt_fingerprint(&blocks)?.wire_representation_hash
        );
        assert!(wire_prompt_prefix_diff(&shortcut, &blocks)?.is_append_only);
        for content in [
            json!([{"type":"text","text":"different"}]),
            json!([{"type":"text","text":"same","extra":true}]),
            json!([{"type":"text","text":"same"},{"type":"text","text":"extra"}]),
            json!(null),
            json!([]),
            json!(""),
        ] {
            let changed = json!({"messages":[{"role":"user","content":content}]});
            assert!(!wire_prompt_prefix_diff(&shortcut, &changed)?.is_append_only);
        }
        let a = json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"reason","signature":"original"},{"type":"text","text":"answer"}]}],"tools":[{"input_schema":{"content":"same"}}]});
        let mut changed = a.clone();
        if let Some(value) = changed.pointer_mut("/messages/0/content/0/signature") {
            *value = json!("changed");
        }
        assert!(!wire_prompt_prefix_diff(&a, &changed)?.is_append_only);
        let mut changed = a.clone();
        if let Some(parts) = changed
            .pointer_mut("/messages/0/content")
            .and_then(Value::as_array_mut)
        {
            parts.reverse();
        }
        assert!(!wire_prompt_prefix_diff(&a, &changed)?.is_append_only);
        let mut changed = a.clone();
        if let Some(value) = changed.pointer_mut("/tools/0/input_schema/content") {
            *value = json!([{"type":"text","text":"same"}]);
        }
        assert!(!wire_prompt_prefix_diff(&a, &changed)?.is_append_only);
        Ok(())
    }

    #[test]
    fn system_summary_covers_in_history_instructions() -> Result<(), serde_json::Error> {
        for field in ["messages", "input"] {
            let mut previous = json!({(field):[{"role":"system","content":"original"}]});
            let summary = wire_prompt_fingerprint(&previous)?;
            if let Some(content) = previous.pointer_mut(&format!("/{field}/0/content")) {
                *content = json!("updated");
            }
            assert_ne!(
                summary.system_hash,
                wire_prompt_fingerprint(&previous)?.system_hash
            );
        }
        Ok(())
    }

    #[test]
    fn wire_diagnostics_cover_omitted_fields_without_logging_keys() -> Result<(), serde_json::Error>
    {
        let base = json!({"model":"m","tools":[{"description":"a","strict":false}],"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"private"}}]}]});
        for pointer in [
            "/tools/0/description",
            "/tools/0/strict",
            "/messages/0/content/0/image_url/url",
        ] {
            let mut changed = base.clone();
            if let Some(value) = changed.pointer_mut(pointer) {
                *value = json!("changed");
            }
            assert_ne!(
                wire_prompt_fingerprint(&base)?.prompt_hash,
                wire_prompt_fingerprint(&changed)?.prompt_hash
            );
            assert!(!wire_prompt_prefix_diff(&base, &changed)?.is_append_only);
        }
        let mut changed = base.clone();
        changed["tools"][0]["private-schema-key"] = json!(true);
        let diff = wire_prompt_prefix_diff(&base, &changed)?;
        assert!(!serde_json::to_string(&diff)?.contains("private-schema-key"));
        Ok(())
    }

    #[test]
    fn moving_breakpoints_preserves_semantics_and_input_schema() -> Result<(), serde_json::Error> {
        let a = json!({"tools":[{"input_schema":{"cache_control":"semantic"}}],"messages":[{"role":"user","content":[{"type":"text","text":"a","cache_control":{"type":"ephemeral"}}]}]});
        let mut b = a.clone();
        if let Some(block) = b
            .pointer_mut("/messages/0/content/0")
            .and_then(Value::as_object_mut)
        {
            block.remove("cache_control");
        }
        assert!(wire_prompt_prefix_diff(&a, &b)?.is_append_only);
        assert_ne!(
            wire_prompt_fingerprint(&a)?.cache_controls_hash,
            wire_prompt_fingerprint(&b)?.cache_controls_hash
        );
        b["tools"][0]["input_schema"]["cache_control"] = json!("changed");
        assert!(!wire_prompt_prefix_diff(&a, &b)?.is_append_only);
        Ok(())
    }
}
