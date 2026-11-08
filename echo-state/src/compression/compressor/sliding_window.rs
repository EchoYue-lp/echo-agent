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
            let (evicted, kept, evicted_prefix_len) = select_recent_tail(
                conv_msgs,
                conversation_limit,
                self.recent_token_budget,
                self.window_size,
                self.recent_token_budget.is_some(),
                tokenizer.as_ref(),
            )?;

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
            if let Some(split_at) = evicted_prefix_len
                && self.recent_token_budget.is_none()
            {
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
/// blocks, but never its request or half of a tool-call/result block. The last
/// tool batch without a later assistant response is still current model input.
pub(crate) fn select_recent_tail(
    messages: Vec<Message>,
    input_limit: usize,
    recent_tokens: Option<usize>,
    message_cap: usize,
    protect_request: bool,
    tokenizer: &dyn Tokenizer,
) -> Result<(Vec<Message>, Vec<Message>, Option<usize>)> {
    let latest_request = messages.iter().rposition(is_user_request);
    let latest_user = protect_request.then_some(latest_request).flatten();
    let request_cost = latest_user
        .and_then(|index| messages.get(index))
        .map(|message| message_tokens(std::slice::from_ref(message), tokenizer))
        .unwrap_or(0);
    if request_cost > input_limit {
        return Err(echo_core::error::AgentError::ContextLimitExceeded(
            format!("latest user request needs {request_cost} tokens, available input budget is {input_limit}")
        ).into());
    }
    let mut selected = vec![false; messages.len()];
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
    // 后继assistant消息才表明原工具组已被模型消费；没有它的新工具组仍是当前输入。
    // 复用原分组边界，不从工具正文猜意图，也不另建持久化交付frontier。
    let pending_tool_unit = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
        .filter(|index| latest_request.is_none_or(|user| *index > user))
        .filter(|index| {
            messages.get(*index).is_some_and(|message| {
                message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
            })
        })
        .and_then(|index| units.iter().find(|unit| unit.start == index));
    if let Some(unit) = pending_tool_unit {
        // 消息条数模式也不能用旧组挤掉新请求；当前工具输入与所属请求一起保留。
        if let Some(index) = latest_request
            && let Some(slot) = selected.get_mut(index)
        {
            *slot = true;
        }
        for slot in selected.get_mut(unit.clone()).unwrap_or_default() {
            *slot = true;
        }
    }
    let required_cost = messages
        .iter()
        .zip(&selected)
        .filter(|(_, required)| **required)
        .fold(0usize, |cost, (message, _)| {
            cost.saturating_add(message_tokens(std::slice::from_ref(message), tokenizer))
        });
    if required_cost > input_limit {
        return Err(echo_core::error::AgentError::ContextLimitExceeded(format!(
            "latest request and unconsumed tool batch need {required_cost} tokens, available input budget is {input_limit}"
        )).into());
    }
    // 当前请求和未消费工具组可突破soft recent，但不能扩大原hard input预算。
    let budget = recent_tokens
        .unwrap_or(input_limit)
        .max(required_cost)
        .min(input_limit);
    let mut cost = required_cost;
    let mut count = selected.iter().filter(|selected| **selected).count();
    let cap = if recent_tokens.is_some() {
        usize::MAX
    } else {
        message_cap.max(count)
    };
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
            let selected_cost = messages
                .get(from..to)
                .unwrap_or_default()
                .iter()
                .zip(selected.get(from..to).unwrap_or_default())
                .filter(|(_, selected)| **selected)
                .fold(0usize, |cost, (message, _)| {
                    cost.saturating_add(message_tokens(std::slice::from_ref(message), tokenizer))
                });
            let already = selected
                .get(from..to)
                .unwrap_or_default()
                .iter()
                .any(|selected| *selected);
            let extra = turn_cost.saturating_sub(selected_cost);
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
                    if selected
                        .get(unit.clone())
                        .is_some_and(|slots| slots.iter().all(|slot| *slot))
                    {
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
            if selected
                .get(unit.clone())
                .is_some_and(|slots| slots.iter().all(|slot| *slot))
            {
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
    // 直接从同一次选择记录判定前缀，不能把非连续驱逐数量伪报成covered_range。
    let prefix_len = selected.iter().take_while(|keep| !**keep).count();
    let evicted_count = selected.iter().filter(|keep| !**keep).count();
    let evicted_prefix_len = (prefix_len > 0 && prefix_len == evicted_count).then_some(prefix_len);
    let mut kept = Vec::new();
    let mut evicted = Vec::new();
    for (index, message) in messages.into_iter().enumerate() {
        if selected.get(index).copied().unwrap_or(false) {
            kept.push(message);
        } else {
            evicted.push(message);
        }
    }
    Ok((evicted, kept, evicted_prefix_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::{FunctionCall, Message, ToolCall};
    use echo_core::tokenizer::HeuristicTokenizer;

    #[test]
    fn unconsumed_tool_group_expands_recent_allowance_inside_original_hard_budget() -> Result<()> {
        let tokenizer = HeuristicTokenizer;
        let page = "fresh page 完整🚀 ".repeat(180);
        let result = Message::tool_result("new-page".into(), "read_artifact".into(), page.clone());
        let page_tokens = message_tokens(std::slice::from_ref(&result), &tokenizer);
        assert!(page_tokens > 400 && page_tokens <= 2000);
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            provider_source: None,
            id: "new-page".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_artifact".into(),
                arguments: "{\"offset\":23628,\"limit\":2000}".into(),
            },
        }]);
        let request = Message::user("按原文继续读取，不丢页".into());
        let (evicted, kept, _) = select_recent_tail(
            vec![
                Message::user("old history ".repeat(1000)),
                Message::assistant("old answer".into()),
                request.clone(),
                call.clone(),
                result.clone(),
            ],
            8000,
            Some(400),
            1,
            true,
            &tokenizer,
        )?;
        assert!(kept.iter().any(|message| {
            message.role == Role::User
                && message.content.as_text_ref() == request.content.as_text_ref()
        }));
        assert!(kept.iter().any(|message| {
            message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "new-page"))
        }));
        assert!(
            kept.iter().any(|message| {
                message.tool_call_id.as_deref() == Some("new-page")
                    && message.content.as_text_ref() == Some(page.as_str())
            }),
            "尚未交模型的新工具页必须完整保留"
        );
        assert!(message_tokens(&kept, &tokenizer) <= 8000);
        assert_eq!(evicted.len(), 2);
        Ok(())
    }

    #[test]
    fn unconsumed_tool_group_over_hard_budget_returns_original_typed_limit() {
        let tokenizer = HeuristicTokenizer;
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            provider_source: None,
            id: "too-large".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_artifact".into(),
                arguments: "{}".into(),
            },
        }]);
        let result = select_recent_tail(
            vec![
                Message::user("current request".into()),
                call,
                Message::tool_result(
                    "too-large".into(),
                    "read_artifact".into(),
                    "unconsumed page ".repeat(8000),
                ),
            ],
            8000,
            Some(400),
            1,
            true,
            &tokenizer,
        );
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Agent(error))
                if matches!(error.as_ref(), echo_core::error::AgentError::ContextLimitExceeded(_))
        ));
    }

    #[test]
    fn count_window_does_not_pin_an_old_tool_group_ahead_of_a_new_request() -> Result<()> {
        let call = Message::assistant_with_tools(vec![ToolCall {
            provider_source: None,
            id: "old-call".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        }]);
        let (_, kept, _) = select_recent_tail(
            vec![
                Message::user("old request".into()),
                call,
                Message::tool_result("old-call".into(), "read".into(), "old result".into()),
                Message::user("new request".into()),
            ],
            8000,
            None,
            1,
            false,
            &HeuristicTokenizer,
        )?;
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept.first()
                .and_then(|message| message.content.as_text_ref()),
            Some("new request")
        );
        Ok(())
    }

    #[tokio::test]
    async fn count_window_nonprefix_eviction_does_not_claim_a_prefix_covered_range() -> Result<()> {
        let call = |id: &str| {
            Message::assistant_with_tools(vec![ToolCall {
                provider_source: None,
                id: id.into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            }])
        };
        let output = SlidingWindowCompressor::new(1)
            .compress(CompressionInput {
                messages: vec![
                    Message::user("current request".into()),
                    call("old"),
                    Message::tool_result("old".into(), "read".into(), "already consumed".into()),
                    call("new"),
                    Message::tool_result("new".into(), "read".into(), "unconsumed".into()),
                ],
                token_limit: 8000,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert_eq!(output.messages.len(), 3);
        assert_eq!(output.evicted.len(), 2);
        assert!(
            output
                .checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.covered_range.is_none())
        );
        Ok(())
    }

    #[test]
    fn count_window_keeps_the_current_request_with_its_unconsumed_tool_group() -> Result<()> {
        let call = Message::assistant_with_tools(vec![ToolCall {
            provider_source: None,
            id: "current-call".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        }]);
        let (_, kept, _) = select_recent_tail(
            vec![
                Message::user("current request".into()),
                call,
                Message::tool_result(
                    "current-call".into(),
                    "read".into(),
                    "current result".into(),
                ),
            ],
            8000,
            None,
            1,
            false,
            &HeuristicTokenizer,
        )?;
        assert_eq!(kept.len(), 3);
        assert_eq!(
            kept.first()
                .and_then(|message| message.content.as_text_ref()),
            Some("current request")
        );
        Ok(())
    }

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
            provider_source: None,
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
                    provider_source: None,
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
        let call = Message::assistant_with_tools(vec![ToolCall {
            provider_source: None,
            id: "call-1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
        }]);
        let unconsumed = vec![
            Message::user("old".repeat(100)),
            call,
            Message::tool_result("call-1".into(), "lookup".into(), "r".repeat(400)),
        ];
        let oversized = compressor
            .compress(CompressionInput {
                messages: unconsumed.clone(),
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await;
        assert!(
            matches!(oversized, Err(echo_core::error::ReactError::Agent(error))
            if matches!(error.as_ref(), echo_core::error::AgentError::ContextLimitExceeded(_)))
        );

        // 真实后继assistant已消费该组后，历史才能沿原35-token预算整组淘汰。
        let mut historical = unconsumed;
        historical.push(Message::assistant("consumed response".into()));
        let output = compressor
            .compress(CompressionInput {
                messages: historical,
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert!(message_tokens(&output.messages, &HeuristicTokenizer) <= 35);
        assert_eq!(output.messages.len(), 1);
        assert_eq!(
            output
                .messages
                .first()
                .and_then(|message| message.content.as_text_ref()),
            Some("consumed response")
        );
        assert!(
            output
                .messages
                .iter()
                .all(|message| message.role != Role::Tool && message.tool_calls.is_none())
        );
        assert_eq!(output.evicted.len(), 3);

        let fitting_call = Message::assistant_with_tools(vec![ToolCall {
            provider_source: None,
            id: "call-2".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
        }]);
        let fitting = compressor
            .compress(CompressionInput {
                messages: vec![
                    Message::user("old".repeat(100)),
                    Message::assistant("old answer ".repeat(100)),
                    Message::user("current request".into()),
                    fitting_call,
                    Message::tool_result("call-2".into(), "lookup".into(), "ok".into()),
                ],
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert!(message_tokens(&fitting.messages, &HeuristicTokenizer) <= 35);
        assert_eq!(fitting.messages.len(), 3);
        assert_eq!(
            fitting
                .messages
                .first()
                .and_then(|message| message.content.as_text_ref()),
            Some("current request")
        );
        assert!(
            fitting
                .messages
                .get(1)
                .is_some_and(|message| message.tool_calls.is_some())
        );
        assert!(
            fitting
                .messages
                .get(2)
                .is_some_and(|message| message.role == Role::Tool)
        );
        assert_eq!(fitting.evicted.len(), 2);
        Ok(())
    }
}
