//! Responses 协议诊断只观察请求形状和事件坐标，不参与协议校验或完成判定。

use super::LlmConfig;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const TARGET: &str = "echo_agent::provider_diagnostics";
const MAX_EVENTS: u64 = 96;
const MAX_TAIL: usize = 8;
const MAX_LABEL: usize = 64;
const MAX_ITEMS: usize = 128;
static NEXT_TRACE: AtomicU64 = AtomicU64::new(0);

/// 以流式 JSON 序列化计算签名，避免为大图片请求再分配完整正文副本。
#[derive(Default)]
struct Fingerprint {
    digest: Sha256,
    bytes: u64,
}

impl Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.digest.update(bytes);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fingerprint(value: &Value) -> Value {
    let mut writer = Fingerprint::default();
    if serde_json::to_writer(&mut writer, value).is_err() {
        return json!({"serialization_failed": true});
    }
    json!({"sha256": format!("{:x}", writer.digest.finalize()), "serialized_bytes": writer.bytes})
}

fn label(value: &str) -> String {
    value.chars().take(MAX_LABEL).collect()
}

fn identity(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|id| format!("{:x}", Sha256::digest(id.as_bytes())))
}

// phase 区分缺失/null/空字符串/未知值，不能在诊断时先替供应商归一化。
fn phase(value: Option<&Value>) -> Value {
    match value {
        None => json!({"kind": "missing"}),
        Some(Value::Null) => json!({"kind": "null"}),
        Some(Value::String(text)) => json!({
            "kind": "string", "value": label(text), "characters": text.chars().count(),
            "sha256": format!("{:x}", Sha256::digest(text.as_bytes()))
        }),
        Some(other) => json!({"kind": "invalid", "fingerprint": fingerprint(other)}),
    }
}

fn item_summary(item: &Value) -> Value {
    json!({
        "id_sha256": identity(item.get("id")),
        "type": item.get("type").and_then(Value::as_str).map(label),
        "status": item.get("status").and_then(Value::as_str).map(label),
        "phase": phase(item.get("phase")),
        "content_parts": item.get("content").and_then(Value::as_array).map(Vec::len),
    })
}

fn request_summary(config: &LlmConfig, body: &Value, dialect: &str) -> Value {
    let endpoint = reqwest::Url::parse(&config.base_url).ok();
    let tools = body.get("tools").and_then(Value::as_array);
    let tool_shapes = tools
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .map(|tool| {
            json!({
                "name": tool.get("name").and_then(Value::as_str).map(label),
                "type": tool.get("type").and_then(Value::as_str).map(label),
                "strict": tool.get("strict").and_then(Value::as_bool),
                "strict_present": tool.get("strict").is_some(),
                "parameters_fingerprint": tool.get("parameters").map(fingerprint),
            })
        })
        .collect::<Vec<_>>();
    let mut roles = HashMap::<String, u64>::new();
    let mut part_types = HashMap::<String, u64>::new();
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(role) = item.get("role").and_then(Value::as_str) {
            let count = roles.entry(label(role)).or_default();
            *count = count.saturating_add(1);
        }
        for part in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(kind) = part.get("type").and_then(Value::as_str) {
                let count = part_types.entry(label(kind)).or_default();
                *count = count.saturating_add(1);
            }
        }
    }
    json!({
        "protocol": "responses", "dialect": dialect,
        "provider": config.provider_name.as_deref().map(label), "model": label(&config.model),
        // URL 中的 userinfo/query/fragment 可能含密钥，路径也不写入日志。
        "endpoint_scheme": endpoint.as_ref().map(reqwest::Url::scheme),
        "endpoint_host": endpoint.as_ref().and_then(reqwest::Url::host_str),
        "endpoint_port": endpoint.as_ref().and_then(reqwest::Url::port),
        "request_fingerprint": fingerprint(body),
        "input_items": body.get("input").and_then(Value::as_array).map(Vec::len),
        "input_roles": roles, "input_part_types": part_types,
        "tools_present": tools.is_some(), "tool_count": tools.map(Vec::len).unwrap_or_default(),
        "tools_fingerprint": body.get("tools").map(fingerprint), "tool_shapes": tool_shapes,
        "tool_shapes_truncated": tools.is_some_and(|tools| tools.len() > MAX_ITEMS),
        "tool_choice_kind": body.get("tool_choice").map(|value| match value {
            Value::String(text) => label(text),
            Value::Object(_) => "named_tool".into(),
            _ => "invalid".into(),
        }),
        "reasoning_effort": phase(body.pointer("/reasoning/effort")),
        "temperature": body.get("temperature").and_then(Value::as_f64),
        "max_output_tokens": body.get("max_output_tokens").and_then(Value::as_u64),
        "stream": body.get("stream").and_then(Value::as_bool),
        "store": body.get("store").and_then(Value::as_bool),
        "user_present": body.get("user").is_some(),
        "instructions_present": body.get("instructions").is_some(),
        "output_format": body.pointer("/text/format/type").and_then(Value::as_str).map(label),
    })
}

pub(super) struct ResponsesDiagnostics {
    trace_id: String,
    started: Instant,
    request: Value,
    native_id: Option<String>,
    events: u64,
    deltas: u64,
    emitted: u64,
    suppressed: u64,
    first_deltas: HashSet<String>,
    tail: VecDeque<Value>,
    terminal_seen: bool,
    finished: bool,
}

impl ResponsesDiagnostics {
    pub(super) fn start(config: &LlmConfig, body: &Value, dialect: &str) -> Self {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let diagnostic = Self {
            trace_id: format!(
                "responses-{}-{epoch}-{}",
                std::process::id(),
                NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
            ),
            started: Instant::now(),
            request: request_summary(config, body, dialect),
            native_id: None,
            events: 0,
            deltas: 0,
            emitted: 0,
            suppressed: 0,
            first_deltas: HashSet::new(),
            tail: VecDeque::new(),
            terminal_seen: false,
            finished: false,
        };
        diagnostic.emit("request", &diagnostic.request, false);
        diagnostic
    }

    fn emit(&self, stage: &str, detail: &Value, failed: bool) {
        let receipt = json!({"schema_version": 1, "trace_id": self.trace_id,
            "stage": stage, "elapsed_ms": self.started.elapsed().as_millis(),
            "native_request_id_sha256": self.native_id, "detail": detail});
        // 共用框架日志 target，避免宿主仅开启 echo_agent 时漏掉集成层证据。
        if failed {
            tracing::error!(target: TARGET, responses_diagnostic = %receipt, "Responses stream diagnostic");
        } else {
            tracing::info!(target: TARGET, responses_diagnostic = %receipt, "Responses stream diagnostic");
        }
    }

    pub(super) fn event(&mut self, event: &Value, observed_phases: &HashMap<String, String>) {
        self.events = self.events.saturating_add(1);
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        let delta = kind.ends_with(".delta");
        if delta {
            self.deltas = self.deltas.saturating_add(1);
        }
        if let Some(id) = identity(event.pointer("/response/id")) {
            self.native_id = Some(id);
        }
        let item = event.get("item");
        let previous = item
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .and_then(|id| observed_phases.get(id));
        let current = item.and_then(|item| item.get("phase"));
        let changed = previous.is_some_and(|before| {
            current.is_some_and(|after| after.as_str() != Some(before.as_str()))
        });
        let previous_value = previous.map(|value| json!(value));
        let terminal = matches!(
            kind,
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        );
        self.terminal_seen |= terminal;
        let response = event.get("response");
        let detail = json!({
            "event_ordinal": self.events, "event_type": label(kind),
            "sequence_number": event.get("sequence_number").and_then(Value::as_u64),
            "output_index": event.get("output_index").and_then(Value::as_u64),
            "content_index": event.get("content_index").and_then(Value::as_u64),
            "summary_index": event.get("summary_index").and_then(Value::as_u64),
            "item_id_sha256": identity(event.get("item_id")),
            "item": item.map(item_summary),
            "previous_phase": phase(previous_value.as_ref()), "phase_changed": changed,
            "part_type": event.pointer("/part/type").and_then(Value::as_str).map(label),
            "delta_bytes": event.get("delta").and_then(Value::as_str).map(str::len),
            "response_status": response.and_then(|response| response.get("status")).and_then(Value::as_str).map(label),
            "output_count": response.and_then(|response| response.get("output")).and_then(Value::as_array).map(Vec::len),
            "output": response.and_then(|response| response.get("output")).and_then(Value::as_array)
                .map(|items| items.iter().take(MAX_ITEMS).map(item_summary).collect::<Vec<_>>()),
            "provider_error_present": event.get("error").is_some_and(|value| !value.is_null())
                || event.pointer("/response/error").is_some_and(|value| !value.is_null()),
        });
        if self.tail.len() == MAX_TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(detail.clone());
        let first_delta = if delta && self.first_deltas.len() < MAX_ITEMS {
            self.first_deltas.insert(format!(
                "{}:{:?}:{:?}:{:?}",
                label(kind),
                identity(event.get("item_id")),
                event.get("output_index").and_then(Value::as_u64),
                event.get("content_index").and_then(Value::as_u64)
            ))
        } else {
            false
        };
        if changed || terminal || self.emitted < MAX_EVENTS && (!delta || first_delta) {
            self.emitted = self.emitted.saturating_add(1);
            self.emit("event", &detail, false);
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
        }
    }

    pub(super) fn finish(&mut self, stage: &str, failed: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        // 错误收尾重复带请求形状，INFO 被宿主过滤时仍可取到关联证据。
        self.emit(
            stage,
            &json!({"request": self.request, "event_count": self.events,
            "delta_count": self.deltas, "suppressed_events": self.suppressed,
            "provider_terminal_seen": self.terminal_seen, "last_events": self.tail}),
            failed,
        );
    }
}

impl Drop for ResponsesDiagnostics {
    fn drop(&mut self) {
        // 消费者提前丢弃流不是 provider/运行时成功，也不是伪造的失败终态。
        self.finish("consumer_dropped", false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::client::JsonSseEvent;
    use echo_core::error::{LlmError, Result};
    use echo_core::llm::LlmApiProtocol;
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::instrument::WithSubscriber;
    use tracing::{Event, Metadata, Subscriber, span};

    #[derive(Clone, Default)]
    struct RecordingSubscriber(Arc<Mutex<Vec<Value>>>);

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            if event.metadata().target() != TARGET {
                return;
            }
            struct Receipt(Option<Value>);
            impl Visit for Receipt {
                fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                    if field.name() == "responses_diagnostic" {
                        self.0 = serde_json::from_str(&format!("{value:?}")).ok();
                    }
                }
            }
            let mut receipt = Receipt(None);
            event.record(&mut receipt);
            if let Some(value) = receipt.0
                && let Ok(mut events) = self.0.lock()
            {
                events.push(value);
            }
        }
    }

    fn config() -> Result<LlmConfig> {
        LlmConfig::for_provider(
            "custom-provider",
            "https://api.deepseek.com",
            "KEY_SECRET",
            "deepseek-flash",
            LlmApiProtocol::Responses,
        )
    }

    #[test]
    fn request_fingerprint_covers_actual_body_without_logging_payload_or_url_credentials()
    -> Result<()> {
        let mut config = config()?;
        config.base_url = "https://username:URL_SECRET@api.deepseek.com/private/URL_SECRET?key=QUERY_SECRET#FRAGMENT_SECRET".into();
        let body = json!({"model":"deepseek-flash","input":[{"role":"user","content":[
            {"type":"input_text","text":"BODY_SECRET中文🙂"},
            {"type":"input_image","image_url":"IMAGE_SECRET"}]}],
            "tools":[{"type":"function","name":"read_file","description":"DESCRIPTION_SECRET",
                "strict":false,"parameters":{"type":"object","properties":{"path":{"type":"string","default":"DEFAULT_SECRET"}}}}],
            "user":"USER_SECRET","instructions":"INSTRUCTION_SECRET","reasoning":{"effort":"low"},"stream":true});
        let summary = request_summary(&config, &body, "DeepSeekFlash");
        let text = summary.to_string();
        for secret in [
            "KEY_SECRET",
            "URL_SECRET",
            "QUERY_SECRET",
            "FRAGMENT_SECRET",
            "BODY_SECRET",
            "IMAGE_SECRET",
            "DESCRIPTION_SECRET",
            "DEFAULT_SECRET",
            "USER_SECRET",
            "INSTRUCTION_SECRET",
        ] {
            assert!(!text.contains(secret), "diagnostic leaked {secret}");
        }
        assert_eq!(summary.get("tool_count"), Some(&json!(1)));
        assert_eq!(
            summary.pointer("/input_part_types/input_image"),
            Some(&json!(1))
        );
        assert_eq!(
            summary.pointer("/request_fingerprint/sha256"),
            Some(&json!(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&body)?)
            )))
        );
        assert_eq!(
            summary.pointer("/request_fingerprint/serialized_bytes"),
            Some(&json!(serde_json::to_vec(&body)?.len()))
        );
        Ok(())
    }

    #[test]
    fn phases_preserve_missing_null_empty_unknown_and_unicode_without_unbounded_text() {
        assert_eq!(phase(None), json!({"kind":"missing"}));
        assert_eq!(phase(Some(&Value::Null)), json!({"kind":"null"}));
        assert_eq!(phase(Some(&json!(""))).get("value"), Some(&json!("")));
        let raw = "未来阶段🙂".repeat(100);
        let full = json!(raw);
        let recorded = phase(Some(&full));
        assert_eq!(
            recorded.get("characters"),
            Some(&json!(raw.chars().count()))
        );
        assert_eq!(
            recorded
                .get("value")
                .and_then(Value::as_str)
                .map(|text| text.chars().count()),
            Some(MAX_LABEL)
        );
        assert_eq!(
            phase(Some(&json!(false))).get("kind"),
            Some(&json!("invalid"))
        );
    }

    #[tokio::test]
    async fn actual_adapter_logs_conflicting_phases_and_keeps_original_failure() -> Result<()> {
        let recorder = RecordingSubscriber::default();
        let events = recorder.0.clone();
        let outcome = async {
            let diagnostic = ResponsesDiagnostics::start(&config()?, &json!({"input":"BODY_SECRET","stream":true}), "DeepSeekFlash");
            let wire = futures::stream::iter([
                Ok(JsonSseEvent::Data(json!({"type":"response.created","sequence_number":0,"response":{"id":"NATIVE_SECRET","model":"deepseek-flash","status":"in_progress"}}))),
                Ok(JsonSseEvent::Data(json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":"ITEM_SECRET","type":"message","phase":"commentary","content":[]}}))),
                Ok(JsonSseEvent::Data(json!({"type":"response.output_item.done","sequence_number":2,"output_index":0,"item":{"id":"ITEM_SECRET","type":"message","phase":"final_answer","content":[{"type":"output_text","text":"BODY_SECRET"}]}}))),
            ]);
            let chunks = super::super::adapt_responses_stream(wire, "custom-provider".into(), Some(diagnostic)).collect::<Vec<_>>().await;
            Ok::<_, echo_core::error::ReactError>(chunks)
        }.with_subscriber(recorder).await?;
        let original = outcome
            .into_iter()
            .find_map(std::result::Result::err)
            .ok_or_else(|| {
                LlmError::InvalidResponse("original phase conflict was swallowed".into())
            })?;
        assert!(original.to_string().contains("Responses raw phase changed"));
        let receipts = events
            .lock()
            .map_err(|_| LlmError::InvalidResponse("recorded log lock poisoned".into()))?;
        let end = receipts
            .iter()
            .find(|row| row.get("stage") == Some(&json!("adapter_error")))
            .ok_or_else(|| {
                LlmError::InvalidResponse("adapter failure diagnostic missing".into())
            })?;
        assert_eq!(end.pointer("/detail/event_count"), Some(&json!(3)));
        let conflict = end
            .pointer("/detail/last_events")
            .and_then(Value::as_array)
            .and_then(|events| events.last())
            .ok_or_else(|| LlmError::InvalidResponse("phase event missing".into()))?;
        assert_eq!(
            conflict.pointer("/previous_phase/value"),
            Some(&json!("commentary"))
        );
        assert_eq!(
            conflict.pointer("/item/phase/value"),
            Some(&json!("final_answer"))
        );
        assert_eq!(conflict.get("phase_changed"), Some(&json!(true)));
        let trace = end.get("trace_id");
        assert!(trace.and_then(Value::as_str).is_some());
        assert!(receipts.iter().all(|row| row.get("trace_id") == trace));
        let all = serde_json::to_string(&*receipts)?;
        for secret in ["KEY_SECRET", "BODY_SECRET", "NATIVE_SECRET", "ITEM_SECRET"] {
            assert!(!all.contains(secret));
        }
        assert_eq!(
            receipts
                .iter()
                .filter(|row| row.get("stage") == Some(&json!("adapter_error")))
                .count(),
            1
        );
        assert!(
            !receipts
                .iter()
                .any(|row| row.get("stage") == Some(&json!("consumer_dropped")))
        );
        Ok(())
    }

    #[test]
    fn token_frames_are_bounded_and_late_phase_conflict_survives_the_log_cap() -> Result<()> {
        let mut diagnostic =
            ResponsesDiagnostics::start(&config()?, &json!({"stream":true}), "DeepSeekFlash");
        let phases = HashMap::from([("item".to_string(), "commentary".to_string())]);
        for index in 0..1000_u64 {
            diagnostic.event(&json!({"type":"response.output_text.delta","item_id":"item","output_index":0,"content_index":0,"sequence_number":index,"delta":"TOKEN_SECRET"}),&phases);
        }
        assert_eq!(diagnostic.emitted, 1);
        assert_eq!(diagnostic.suppressed, 999);
        for index in 0..150_u64 {
            diagnostic.event(
                &json!({"type":"response.content_part.done","output_index":index}),
                &phases,
            );
        }
        let emitted = diagnostic.emitted;
        diagnostic.event(&json!({"type":"response.output_item.done","output_index":0,"item":{"id":"item","type":"message","phase":"final_answer","content":[{"type":"output_text","text":"TOKEN_SECRET"}]}}),&phases);
        assert_eq!(diagnostic.emitted, emitted.saturating_add(1));
        assert_eq!(diagnostic.tail.len(), MAX_TAIL);
        assert_eq!(
            diagnostic
                .tail
                .back()
                .and_then(|row| row.get("phase_changed")),
            Some(&json!(true))
        );
        assert!(!serde_json::to_string(&diagnostic.tail)?.contains("TOKEN_SECRET"));
        diagnostic.finish("test_finished", false);
        Ok(())
    }
}
