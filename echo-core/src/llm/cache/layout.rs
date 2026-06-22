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
    pub runtime_context: SegmentRange,
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
/// 分段规则（基于现有消息的 role 和内容标记，零拷贝）：
/// - **system**: 开头连续的 `Role::System` 消息
/// - **canonical**: system 段内（或紧随）含 `"Canonical context"` 文本的消息
/// - **history**: system/canonical 之后、首个 runtime_context 之前的所有消息
/// - **runtime_context**: 尾部以 `[runtime_context:` 开头的消息（可能多条）
#[derive(Debug, Clone)]
pub struct PromptCacheLayout<'a> {
    pub system: &'a [Message],
    pub canonical: &'a [Message],
    pub history: &'a [Message],
    pub runtime_context: &'a [Message],
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
            .unwrap_or(sys_end); // 无 canonical 消息时，canon_start == sys_end，canonical 段为空

        let system_seg = &messages[..canon_start.min(sys_end)];
        let canonical_seg = if canon_start < sys_end {
            &messages[canon_start..sys_end]
        } else {
            &messages[0..0]
        };

        // runtime_context 段：尾部以 [runtime_context: 开头的消息，向前扩展连续块
        let rt_start = messages
            .iter()
            .rposition(|m| {
                m.content
                    .as_text()
                    .map(|t| t.trim_start().starts_with("[runtime_context:"))
                    .unwrap_or(false)
            })
            .map(|last_rt| {
                // 向前扩展连续的 runtime_context 消息
                let mut s = last_rt;
                while s > sys_end {
                    let prev = &messages[s - 1];
                    let is_rt = prev
                        .content
                        .as_text()
                        .map(|t| t.trim_start().starts_with("[runtime_context:"))
                        .unwrap_or(false);
                    if !is_rt {
                        break;
                    }
                    s -= 1;
                }
                s
            })
            .unwrap_or(messages.len());

        let history_seg = &messages[sys_end..rt_start];
        let runtime_seg = &messages[rt_start..];

        Self {
            system: system_seg,
            canonical: canonical_seg,
            history: history_seg,
            runtime_context: runtime_seg,
            tools,
        }
    }

    /// 计算各段在原 messages 数组中的索引范围（供 `CacheHints` 用）。
    pub fn segment_ranges(&self) -> SegmentRanges {
        // 基于 from_messages 的切片排列计算（from_messages 保证顺序：system→canonical→history→runtime_context）
        let sys_len = self.system.len();
        let canon_len = self.canonical.len();
        let hist_len = self.history.len();
        let rt_len = self.runtime_context.len();
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
            runtime_context: SegmentRange {
                start: sys_len + canon_len + hist_len,
                end: sys_len + canon_len + hist_len + rt_len,
            },
        }
    }
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
        Message::user(format!("[runtime_context:{t}]"))
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
        assert_eq!(layout.runtime_context.len(), 1);
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
        assert_eq!(layout.runtime_context.len(), 2);
    }

    #[test]
    fn ranges_match_slice_lengths() {
        let msgs = vec![sys("S"), sys("[Canonical context — x]"), user("h"), rt("t")];
        let layout = PromptCacheLayout::from_messages(&msgs, &[]);
        let r = layout.segment_ranges();
        assert_eq!(r.system.len(), 1);
        assert_eq!(r.canonical.len(), 1);
        assert_eq!(r.history.len(), 1);
        assert_eq!(r.runtime_context.len(), 1);
        assert_eq!(r.runtime_context.end, 4);
    }

    #[test]
    fn runtime_context_change_does_not_affect_history_segment() {
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
        assert!(layout.runtime_context.is_empty());
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

        // runtime_context changes (different content)
        assert_eq!(l_n.runtime_context.len(), 1);
        assert_eq!(l_n1.runtime_context.len(), 1);
        assert_ne!(
            l_n.runtime_context[0].content.as_text(),
            l_n1.runtime_context[0].content.as_text()
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
        assert!(r.history.end <= r.runtime_context.start);
        assert!(r.runtime_context.start <= r.runtime_context.end);
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
