use crate::llm::types::{Message, ToolDefinition};

/// 缓存断点的精确目标。
///
/// 用精确枚举点表达断点位置而非用 `Vec<SegmentKind>` 重复表达同段两个断点。
/// Provider 实现负责将断点目标映射为协议级指令（Anthropic `cache_control`、OpenAI 前缀位置等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreakpointTarget {
    /// system 段最后一个 block
    SystemLastBlock,
    /// tools 段最后一个 tool definition
    ToolsLastTool,
    /// conversation history 中指定索引处的消息
    HistoryIndex(usize),
    /// conversation history 中最后一条非 runtime_context 消息
    HistoryLastStable,
}

/// Provider 无关的缓存提示，挂在 `ChatRequest` 上轻量传递。
#[derive(Debug, Clone, Default)]
pub struct CacheHints {
    /// Anthropic 族：断点目标列表（最多 4 个）
    pub breakpoints: Vec<BreakpointTarget>,
    /// 稳定前缀的 SHA-256 哈希前缀（16 位 hex），用于日志观测缓存失效
    pub stable_prefix_hash: Option<String>,
    /// 各段在 flatten messages 中的索引范围 [start, end)
    pub segments: SegmentRanges,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentRanges {
    pub system: SegmentRange,
    pub canonical: SegmentRange,
    pub history: SegmentRange,
    /// 尾部追加式动态事实（框架投影信封 + version）。
    pub dynamic: SegmentRange,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SegmentRange {
    pub start: usize,
    pub end: usize,
}

impl SegmentRange {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 只读 layout view：从现有单一 messages 数组识别分段，不改原数组。
///
/// 分段规则来自**框架投影元数据**，不再按产品 marker 或正文前缀猜测：
/// - **system**: 开头连续的 `Role::System` 消息
/// - **canonical**: system 段内含 `"Canonical context"` 标记的框架重注消息
/// - **dynamic**: 消息末尾连续的一段"追加式动态事实"。判据是该消息带框架投影信封且
///   信封带 `version=`（`ProjectionPlacement::Tail`）。
///   它们位于已见历史之后，只随新版本增长，因此是可复用前缀之后的可变尾段。
/// - **history**: system/canonical 之后、dynamic 之前的全部消息（含边界替换投影和
///   被后继取代的旧动态版本——它们已经是不可变前缀的一部分）
///
/// 旧实现按裸 `[runtime_context:` 向前扩展，会把该尾注之后的普通历史也错算成动态段；
/// 现在只接受**尾部连续块**，且系统消息永不进入 dynamic。
#[derive(Debug, Clone)]
pub struct PromptCacheLayout<'a> {
    pub system: &'a [Message],
    pub canonical: &'a [Message],
    pub history: &'a [Message],
    pub dynamic: &'a [Message],
    pub tools: &'a [ToolDefinition],
}

impl<'a> PromptCacheLayout<'a> {
    /// 从 flatten 后的 messages + tools 识别分段（只读，零拷贝）。
    pub fn from_messages(messages: &'a [Message], tools: &'a [ToolDefinition]) -> Self {
        // system 段：开头连续的 System role 消息
        let sys_end = messages
            .iter()
            .position(|m| m.role != crate::llm::types::Role::System)
            .unwrap_or(messages.len());

        // canonical 段：system 区域内含 "Canonical context" 标记的消息。
        // `to_reinjection_messages` 产生 "[Canonical context — ...]" 或类似格式。
        let canon_start = messages[..sys_end]
            .iter()
            .position(|m| {
                m.content
                    .as_text()
                    .map(|t| t.contains("Canonical context"))
                    .unwrap_or(false)
            })
            .unwrap_or(sys_end);

        let system_seg = &messages[..canon_start.min(sys_end)];
        let canonical_seg = if canon_start < sys_end {
            &messages[canon_start..sys_end]
        } else {
            &messages[0..0]
        };

        // dynamic 段：只取尾部连续块，避免把其后的普通历史误判为动态事实。
        let mut dynamic_start = messages.len();
        while let Some(previous) = dynamic_start.checked_sub(1) {
            let Some(message) = messages.get(previous) else {
                break;
            };
            if message.role == crate::llm::types::Role::System || previous < sys_end {
                break;
            }
            if is_dynamic_tail_message(message) {
                dynamic_start = previous;
            } else {
                break;
            }
        }
        let dynamic_start = dynamic_start.max(sys_end);

        let history_seg = &messages[sys_end..dynamic_start];
        let dynamic_seg = &messages[dynamic_start..];

        Self {
            system: system_seg,
            canonical: canonical_seg,
            history: history_seg,
            dynamic: dynamic_seg,
            tools,
        }
    }

    /// 计算各段在原 messages 数组中的索引范围（供 `CacheHints` 用）。
    pub fn segment_ranges(&self) -> SegmentRanges {
        // 基于 from_messages 的切片排列计算（保证顺序：system→canonical→history→dynamic）
        let sys_len = self.system.len();
        let canon_len = self.canonical.len();
        let hist_len = self.history.len();
        let dyn_len = self.dynamic.len();
        SegmentRanges {
            system: SegmentRange {
                start: 0,
                end: sys_len,
            },
            canonical: SegmentRange {
                start: sys_len,
                end: sys_len + canon_len,
            },
            history: SegmentRange {
                start: sys_len + canon_len,
                end: sys_len + canon_len + hist_len,
            },
            dynamic: SegmentRange {
                start: sys_len + canon_len + hist_len,
                end: sys_len + canon_len + hist_len + dyn_len,
            },
        }
    }
}

/// 判断一条消息是否属于"尾部追加式动态事实"。
///
/// 判据是框架元数据（投影信封带 version）来自框架投影信封，
/// 不接受产品自定义 marker，也不按任意正文内容猜测。
pub fn is_dynamic_tail_message(message: &Message) -> bool {
    if message.role == crate::llm::types::Role::System {
        return false;
    }
    if let Some(envelope) = crate::compression::context_projection_envelope(message)
        && envelope.is_append_version()
    {
        return true;
    }
    message
        .content
        .as_text_ref()
        .is_some_and(crate::compression::is_dynamic_context_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Message;

    fn sys(t: &str) -> Message {
        Message::system(t.to_string())
    }
    fn user(t: &str) -> Message {
        Message::user(t.to_string())
    }
    fn rt(t: &str) -> Message {
        Message::user(format!(
            "<echo-agent-context-projection-v2>\nmarker=\"runtime\"\nplacement=tail\nversion=\"test\"\n\n[runtime_context:{t}]"
        ))
    }

    #[test]
    fn segments_typical_conversation() {
        let msgs = vec![
            sys("You are Echo Agent"),
            sys("[Canonical context — system prompt restored]"),
            user("hello"),
            user("how are you"),
            rt("turn\ncwd: /tmp"),
        ];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        assert_eq!(layout.system.len(), 1);
        assert_eq!(layout.canonical.len(), 1);
        assert_eq!(layout.history.len(), 2);
        assert_eq!(layout.dynamic.len(), 1);
    }

    #[test]
    fn no_canonical_yields_empty_canonical_seg() {
        let msgs = vec![sys("S"), user("hi")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        assert_eq!(layout.canonical.len(), 0);
        assert_eq!(layout.system.len(), 1);
        assert_eq!(layout.history.len(), 1);
    }

    #[test]
    fn multiple_trailing_runtime_context_grouped() {
        let msgs = vec![
            sys("S"),
            user("hi"),
            rt("turn\nctx1"),
            rt("Hook:PostCompact\nctx2"),
        ];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        assert_eq!(layout.history.len(), 1);
        assert_eq!(layout.dynamic.len(), 2);
    }

    #[test]
    fn ranges_match_slice_lengths() {
        let msgs = vec![sys("S"), sys("[Canonical context — x]"), user("h"), rt("t")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let r = layout.segment_ranges();
        assert_eq!(r.system.len(), 1);
        assert_eq!(r.canonical.len(), 1);
        assert_eq!(r.history.len(), 1);
        assert_eq!(r.dynamic.len(), 1);
        assert_eq!(r.dynamic.end, 4);
    }

    #[test]
    fn dynamic_tail_change_does_not_affect_history_segment() {
        let msgs1 = vec![sys("S"), user("h"), rt("turn\ncwd: /a")];
        let msgs2 = vec![sys("S"), user("h"), rt("turn\ncwd: /b")];
        let l1 = PromptCacheLayout::from_messages(&msgs1, &[]);
        let l2 = PromptCacheLayout::from_messages(&msgs2, &[]);
        // history 段应该完全相同（不受 runtime_context 变化影响）
        assert_eq!(l1.history.len(), l2.history.len());
        assert_eq!(
            l1.history[0].content.as_text(),
            l2.history[0].content.as_text()
        );
    }

    #[test]
    fn empty_messages_all_empty_segments() {
        let msgs: Vec<Message> = vec![];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        assert!(layout.system.is_empty());
        assert!(layout.canonical.is_empty());
        assert!(layout.history.is_empty());
        assert!(layout.dynamic.is_empty());
    }

    // ── Cross-turn stability tests ──

    /// Simulate turn N → turn N+1: system and canonical should remain identical
    /// between turns; only history grows and runtime_context changes.
    #[test]
    fn system_and_canonical_stable_across_turns() {
        // Turn N
        let turn_n = vec![
            sys("You are Echo Agent"),
            sys("[Canonical context — project rules restored]"),
            user("hello"),
            user("what is Rust?"),
            rt("turn\ncwd: /project"),
        ];
        // Turn N+1: same system + canonical, one more history message, different runtime
        let turn_n1 = vec![
            sys("You are Echo Agent"),
            sys("[Canonical context — project rules restored]"),
            user("hello"),
            user("what is Rust?"),
            user("thanks, now explain ownership"),
            rt("turn\ncwd: /project\nmemory: recall about borrow checker"),
        ];

        let l_n = PromptCacheLayout::from_messages(&turn_n, &[]);
        let l_n1 = PromptCacheLayout::from_messages(&turn_n1, &[]);

        // system and canonical segments must be identical
        assert_eq!(l_n.system.len(), l_n1.system.len());
        assert_eq!(l_n.canonical.len(), l_n1.canonical.len());
        for (a, b) in l_n.system.iter().zip(l_n1.system.iter()) {
            assert_eq!(a.content.as_text(), b.content.as_text());
        }
        for (a, b) in l_n.canonical.iter().zip(l_n1.canonical.iter()) {
            assert_eq!(a.content.as_text(), b.content.as_text());
        }

        // history grows (N+1 has +1 message)
        assert_eq!(l_n.history.len() + 1, l_n1.history.len());

        // dynamic tail changes (different content)
        assert_eq!(l_n.dynamic.len(), 1);
        assert_eq!(l_n1.dynamic.len(), 1);
        assert_ne!(
            l_n.dynamic[0].content.as_text(),
            l_n1.dynamic[0].content.as_text()
        );
    }

    /// Verify that segment_ranges correctly reflects the ordering invariant:
    /// system ≤ canonical ≤ history ≤ runtime_context.
    #[test]
    fn segment_ranges_are_monotonic() {
        let msgs = vec![
            sys("S1"),
            sys("S2"),
            sys("[Canonical context — x]"),
            user("h1"),
            user("h2"),
            rt("turn\nctx"),
        ];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let r = layout.segment_ranges();

        assert!(r.system.start <= r.system.end);
        assert!(r.system.end <= r.canonical.start);
        assert!(r.canonical.start <= r.canonical.end);
        assert!(r.canonical.end <= r.history.start);
        assert!(r.history.start <= r.history.end);
        assert!(r.history.end <= r.dynamic.start);
        assert!(r.dynamic.start <= r.dynamic.end);
    }

    /// When there's no canonical context, system segment absorbs all system messages.
    #[test]
    fn no_canonical_all_system_messages_go_to_system_segment() {
        let msgs = vec![
            sys("You are Echo Agent"),
            sys("Additional instruction"),
            user("hi"),
        ];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        assert_eq!(layout.system.len(), 2);
        assert_eq!(layout.canonical.len(), 0);
        assert_eq!(layout.history.len(), 1);
    }
}
