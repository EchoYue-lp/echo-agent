use crate::compression::{
    CompressionCheckpoint, CompressionInput, CompressionOutput, ContextCompressor,
};
use echo_core::error::Result;
use echo_core::llm::types::{Message, Role};
use echo_core::tokenizer::Tokenizer;
use futures::future::BoxFuture;
use std::time::Instant;

/// 滑动窗口压缩：在 token 限额内保留最近至多 `window_size` 条非 system 消息。
///
/// - system 消息始终保留在列表最前面，不计入窗口计数
/// - 适用于高频、上下文独立的场景，或需要严格控制 token 成本的场景
pub struct SlidingWindowCompressor {
    window_size: usize,
    recent_token_budget: Option<usize>,
}

impl SlidingWindowCompressor {
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size,
            recent_token_budget: None,
        }
    }

    /// Select recent turns by tokens instead of the legacy message-count cap.
    /// The latest real user request is retained in full or admission fails.
    pub fn with_recent_token_budget(mut self, tokens: usize) -> Self {
        self.recent_token_budget = (tokens > 0).then_some(tokens);
        self
    }
}

impl ContextCompressor for SlidingWindowCompressor {
    fn name(&self) -> &str {
        "SlidingWindow"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let start = Instant::now();
            let tokenizer = input.tokenizer();
            let _total_messages = input.messages.len();
            let tokens_before = message_tokens(&input.messages, tokenizer.as_ref());

            let (system_msgs, conv_msgs): (Vec<_>, Vec<_>) = input
                .messages
                .into_iter()
                .partition(|m| m.role == Role::System && !super::summary::is_generated_summary(m));

            let system_count = system_msgs.len();

            let system_tokens = message_tokens(&system_msgs, tokenizer.as_ref());
            let conversation_limit = input.token_limit.saturating_sub(system_tokens);
            let (evicted, kept) = select_recent_tail(
                conv_msgs,
                conversation_limit,
                self.recent_token_budget,
                self.window_size,
                self.recent_token_budget.is_some(),
                tokenizer.as_ref(),
            )?;
            let split_at = evicted.len();

            let mut messages = system_msgs;
            messages.extend(kept);

            let tokens_after = message_tokens(&messages, tokenizer.as_ref());

            let mut checkpoint = CompressionCheckpoint::new(self.name())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(
                    input
                        .focus_instructions
                        .clone()
                        .or(input.current_query.clone()),
                );
            if split_at > 0 && self.recent_token_budget.is_none() {
                checkpoint = checkpoint.with_covered_range(
                    system_count,
                    system_count.saturating_add(split_at).saturating_sub(1),
                );
            }

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

pub(crate) fn message_tokens(messages: &[Message], tokenizer: &dyn Tokenizer) -> usize {
    messages.iter().fold(0usize, |total, message| {
        total.saturating_add(message.content.estimated_tokens(tokenizer))
    })
}

/// Ignore framework-generated user-role notes when locating the active request.
pub(crate) fn is_user_request(message: &Message) -> bool {
    message.role == Role::User
        && !crate::compression::is_context_projection_message(message)
        && message.content.as_text().is_none_or(|text| {
            ![
                "[runtime_context:",
                "[Horizon compact:",
                "[memory_context]",
                "[Relevant historical memories]",
                "[Verifier feedback]",
                "[Hook:",
            ]
            .iter()
            .any(|prefix| text.trim_start().starts_with(prefix))
        })
}

pub(crate) fn ensure_request_fits(
    messages: &[Message],
    limit: usize,
    tokenizer: &dyn Tokenizer,
) -> Result<()> {
    let system = messages
        .iter()
        .filter(|message| {
            message.role == Role::System
                && !message
                    .content
                    .as_text_ref()
                    .is_some_and(|text| text.starts_with("[对话历史摘要]"))
        })
        .fold(0usize, |total, message| {
            total.saturating_add(message_tokens(std::slice::from_ref(message), tokenizer))
        });
    let request = messages
        .iter()
        .rev()
        .find(|message| is_user_request(message))
        .map(|message| message_tokens(std::slice::from_ref(message), tokenizer))
        .unwrap_or(0);
    if request > limit.saturating_sub(system) {
        return Err(echo_core::error::AgentError::ContextLimitExceeded(
            "latest request cannot fit after system and protected context".to_string(),
        )
        .into());
    }
    Ok(())
}

/// One pure selector shared by semantic compression and its safety fallback.
/// Older turns are kept whole. A large active turn may drop older execution
/// blocks, but never its request or half of a tool-call/result block.
pub(crate) fn select_recent_tail(
    messages: Vec<Message>,
    input_limit: usize,
    recent_tokens: Option<usize>,
    message_cap: usize,
    protect_request: bool,
    tokenizer: &dyn Tokenizer,
) -> Result<(Vec<Message>, Vec<Message>)> {
    let latest_user = protect_request
        .then(|| messages.iter().rposition(is_user_request))
        .flatten();
    let request_cost = latest_user
        .and_then(|index| messages.get(index))
        .map(|message| message_tokens(std::slice::from_ref(message), tokenizer))
        .unwrap_or(0);
    if request_cost > input_limit {
        return Err(echo_core::error::AgentError::ContextLimitExceeded(
            format!("latest user request needs {request_cost} tokens, available input budget is {input_limit}")
        ).into());
    }
    // The soft recent allowance can expand to fit one request; the hard input
    // limit still wins. No user input is silently truncated to make it fit.
    let budget = recent_tokens
        .unwrap_or(input_limit)
        .max(request_cost)
        .min(input_limit);
    let cap = if recent_tokens.is_some() {
        usize::MAX
    } else {
        message_cap.max(usize::from(latest_user.is_some()))
    };
    let mut selected = vec![false; messages.len()];
    let mut cost = request_cost;
    let mut count = usize::from(latest_user.is_some());
    if let Some(index) = latest_user
        && let Some(slot) = selected.get_mut(index)
    {
        *slot = true;
    }
    let mut units = Vec::new();
    let mut start = 0usize;
    while start < messages.len() {
        let mut end = start.saturating_add(1);
        if let Some(calls) = messages
            .get(start)
            .filter(|message| message.role == Role::Assistant)
            .and_then(|message| message.tool_calls.as_ref())
        {
            let ids = calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<std::collections::HashSet<_>>();
            let mut scan = end;
            while let Some(message) = messages.get(scan) {
                if is_user_request(message) || message.role == Role::Assistant {
                    break;
                }
                if message
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| ids.contains(id))
                    || (message.role == Role::User && !is_user_request(message))
                {
                    end = scan.saturating_add(1);
                }
                scan = scan.saturating_add(1);
            }
        }
        units.push(start..end);
        start = end;
    }
    if recent_tokens.is_some() {
        // Consider complete turns newest first, splitting only the active one.
        let mut turns = vec![0usize];
        turns.extend(
            messages
                .iter()
                .enumerate()
                .filter_map(|(i, message)| (i > 0 && is_user_request(message)).then_some(i)),
        );
        turns.push(messages.len());
        for turn in turns.windows(2).rev() {
            let (Some(&from), Some(&to)) = (turn.first(), turn.get(1)) else {
                continue;
            };
            let turn_cost = message_tokens(messages.get(from..to).unwrap_or_default(), tokenizer);
            let already = latest_user.is_some_and(|index| index >= from && index < to);
            let extra = turn_cost.saturating_sub(if already { request_cost } else { 0 });
            if cost.saturating_add(extra) <= budget {
                for slot in selected.get_mut(from..to).unwrap_or_default() {
                    *slot = true;
                }
                cost = cost.saturating_add(extra);
            } else if already {
                for unit in units
                    .iter()
                    .rev()
                    .filter(|unit| unit.start >= from && unit.end <= to)
                {
                    if latest_user == Some(unit.start) {
                        continue;
                    }
                    let extra =
                        message_tokens(messages.get(unit.clone()).unwrap_or_default(), tokenizer);
                    if cost.saturating_add(extra) > budget {
                        break;
                    }
                    for slot in selected.get_mut(unit.clone()).unwrap_or_default() {
                        *slot = true;
                    }
                    cost = cost.saturating_add(extra);
                }
                break;
            } else {
                break;
            }
        }
    } else {
        for unit in units.iter().rev() {
            if latest_user == Some(unit.start) {
                continue;
            }
            let extra = message_tokens(messages.get(unit.clone()).unwrap_or_default(), tokenizer);
            let extra_count = unit.end.saturating_sub(unit.start);
            if cost.saturating_add(extra) > budget || count.saturating_add(extra_count) > cap {
                break;
            }
            for slot in selected.get_mut(unit.clone()).unwrap_or_default() {
                *slot = true;
            }
            cost = cost.saturating_add(extra);
            count = count.saturating_add(extra_count);
        }
    }
    let mut kept = Vec::new();
    let mut evicted = Vec::new();
    for (index, message) in messages.into_iter().enumerate() {
        if selected.get(index).copied().unwrap_or(false) {
            kept.push(message);
        } else {
            evicted.push(message);
        }
    }
    Ok((evicted, kept))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::{FunctionCall, Message, ToolCall};
    use echo_core::tokenizer::HeuristicTokenizer;

    #[tokio::test]
    async fn token_tail_keeps_complete_turns_and_splits_large_active_tool_work() -> Result<()> {
        let input = |messages| CompressionInput {
            messages,
            token_limit: 100,
            current_query: None,
            focus_instructions: None,
            cancel_token: None,
            tokenizer: None,
        };
        let compressor = SlidingWindowCompressor::new(1).with_recent_token_budget(30);
        let request = "current exact request🚀";
        let output = compressor
            .compress(input(vec![
                Message::user("old ".repeat(40)),
                Message::assistant("old answer".to_string()),
                Message::user(request.to_string()),
                Message::assistant("result".to_string()),
            ]))
            .await?;
        assert_eq!(output.messages.len(), 2);
        assert_eq!(
            output
                .messages
                .first()
                .and_then(|message| message.content.as_text_ref()),
            Some(request)
        );
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            id: "c".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let output = compressor
            .compress(input(vec![
                Message::user(request.to_string()),
                call,
                Message::tool_result("c".to_string(), "read".to_string(), "large ".repeat(80)),
                Message::assistant("recent result".to_string()),
            ]))
            .await?;
        assert!(
            output
                .messages
                .iter()
                .any(|message| message.content.as_text_ref() == Some(request))
        );
        assert!(
            !output
                .messages
                .iter()
                .any(|message| message.role == Role::Tool || message.tool_calls.is_some())
        );
        assert_eq!(output.evicted.len(), 2);
        assert!(
            output
                .checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.covered_range.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn oversized_request_fails_instead_of_silently_disappearing() {
        let result = SlidingWindowCompressor::new(1)
            .with_recent_token_budget(10)
            .compress(CompressionInput {
                messages: vec![Message::user("中文🚀".repeat(100))],
                token_limit: 20,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn hook_notes_and_rich_results_stay_inside_the_call_batch() -> Result<()> {
        use echo_core::llm::types::{ContentPart, ImageUrl};
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(
            ["a", "b"]
                .iter()
                .map(|id| ToolCall {
                    id: id.to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "read".to_string(),
                        arguments: "{}".to_string(),
                    },
                })
                .collect(),
        );
        let image = Message::user_multimodal(vec![
            ContentPart::Text {
                text: "[runtime_context:ToolResultAttachment] tool image".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".to_string(),
                    detail: None,
                },
            },
        ]);
        let messages = vec![
            Message::user("real current request".to_string()),
            call,
            Message::user("[runtime_context:Hook:PreToolUse] note".to_string()),
            Message::tool_result(
                "a".to_string(),
                "read".to_string(),
                "first result".to_string(),
            ),
            image,
            Message::tool_result(
                "b".to_string(),
                "read".to_string(),
                "second result".to_string(),
            ),
            Message::assistant("recent answer".to_string()),
        ];
        for budget in [30, 2_000] {
            let output = SlidingWindowCompressor::new(1)
                .with_recent_token_budget(budget)
                .compress(CompressionInput {
                    messages: messages.clone(),
                    token_limit: 3_000,
                    current_query: None,
                    focus_instructions: None,
                    cancel_token: None,
                    tokenizer: None,
                })
                .await?;
            assert!(
                output
                    .messages
                    .iter()
                    .any(|message| message.content.as_text_ref() == Some("real current request"))
            );
            let results = output
                .messages
                .iter()
                .filter(|message| message.role == Role::Tool)
                .count();
            assert_eq!(results, if budget == 30 { 0 } else { 2 });
            assert_eq!(
                output
                    .messages
                    .iter()
                    .filter(|message| message.tool_calls.is_some())
                    .count(),
                usize::from(results > 0)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn compression_is_token_bounded_and_keeps_tool_groups_atomic() -> Result<()> {
        let compressor = SlidingWindowCompressor::new(40);
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let result =
            Message::tool_result("call-1".to_string(), "lookup".to_string(), "r".repeat(400));
        let output = compressor
            .compress(CompressionInput {
                messages: vec![Message::user("old".repeat(100)), call, result],
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;

        assert!(message_tokens(&output.messages, &HeuristicTokenizer) <= 35);
        assert!(
            output.messages.is_empty(),
            "oversized tool group must be evicted whole"
        );
        assert_eq!(output.evicted.len(), 3);

        let mut fitting_call = Message::assistant(String::new());
        fitting_call.tool_calls = Some(vec![ToolCall {
            id: "call-2".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let fitting_result =
            Message::tool_result("call-2".to_string(), "lookup".to_string(), "ok".to_string());
        let fitting = compressor
            .compress(CompressionInput {
                messages: vec![
                    Message::user("old".repeat(100)),
                    fitting_call,
                    fitting_result,
                ],
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert_eq!(fitting.messages.len(), 2);
        assert!(
            fitting
                .messages
                .first()
                .is_some_and(|message| message.tool_calls.is_some())
        );
        assert!(
            fitting
                .messages
                .get(1)
                .is_some_and(|message| message.role == Role::Tool)
        );
        Ok(())
    }
}
