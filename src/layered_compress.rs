//! Layered compression — L1 (fold tool calls) → L2 (sliding window) → L3 (memory).
//!
//! **Note**: The L1 tool-folding logic is now integrated into
//! [`AdaptiveCompressor`](crate::compression::levels::AdaptiveCompressor) via the
//! `l1_fold_consecutive_tools` config. This module remains for standalone use.
//!
//! # Levels
//! - **L1**: Fold repeated same-tool results into summary lines (0 LLM cost)
//! - **L2**: Apply sliding window or LLM summarization (existing compressors)
//! - **L3**: Promote key facts to long-term memory (memory_write)

use crate::llm::types::Message;

/// L1: Fold repeated tool results. Keeps the latest N per tool, replaces older with summary.
/// Returns the count of original messages that were replaced (NOT net reduction —
/// folding N messages inserts 1 summary, so net reduction is `folded - 1`).
pub fn l1_fold_tools(messages: &mut Vec<Message>, keep_latest: usize) -> usize {
    let mut folded = 0;
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role.as_str() == "tool" {
            let start = i;
            i += 1;
            while i < messages.len() && messages[i].role.as_str() == "tool" {
                i += 1;
            }
            let count = i - start;
            if count > keep_latest {
                let to_remove = count - keep_latest;
                let fold_msg = Message::user(format!(
                    "[L1 fold: {to_remove} consecutive tool results collapsed]"
                ));
                messages.drain(start..start + to_remove);
                messages.insert(start, fold_msg);
                folded += to_remove; // count of original messages replaced
                i = start + 1 + keep_latest;
            }
        } else {
            i += 1;
        }
    }
    folded
}

/// L3: Extract important facts and write to memory (best-effort).
pub fn l3_promote_memory(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| {
            m.role.as_str() == "system" || m.content.as_text().unwrap_or_default().len() > 500
        })
        .filter_map(|m| m.content.as_text())
        .map(|t| t.chars().take(200).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_l1_fold() {
        // Use Message::tool_result which produces role="tool"
        let mut msgs = vec![
            Message::tool_result("1".into(), "read_file".into(), "content a".into()),
            Message::tool_result("2".into(), "read_file".into(), "content b".into()),
        ];
        let folded = l1_fold_tools(&mut msgs, 1);
        assert_eq!(folded, 1, "2 tool msgs with keep=1 should fold 1");
        assert_eq!(msgs.len(), 2, "should have fold msg + last result = 2");
    }
}
