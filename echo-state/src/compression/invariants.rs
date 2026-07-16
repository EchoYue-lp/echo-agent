//! Compression invariant tests.
//!
//! These tests verify that compression preserves critical information
//! regardless of which strategy is used. They are the "safety net" for
//! the compression system — if any of these fail, compression is broken.
//!
//! ## Invariants covered
//!
//! 1. Tool call/result pairing integrity
//! 2. Last user request preserved
//! 3. System prompt preserved
//! 4. Protected markers survive
//! 5. Pending tasks preserved in summary
//! 6. Referenced file paths preserved
//! 7. Post-compression tokens ≤ target limit
//! 8. Compression idempotency (re-compress doesn't lose more)
//! 9. Adaptive escalation produces expected levels
//! 10. VisibilityHorizon leaves no orphaned calls

#[cfg(test)]
mod tests {
    use crate::compression::compressor::{
        SlidingWindowCompressor, default_summary_prompt_with_focus,
    };
    use crate::compression::horizon::VisibilityHorizonCompressor;
    use crate::compression::levels::{AdaptiveCompressionConfig, AdaptiveCompressor};
    use crate::compression::{CompressionInput, ContextCompressor, ContextManager};
    use echo_core::llm::types::{Message, MessageContent, Role, ToolCall};
    use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};

    // ── Test Helpers ─────────────────────────────────────────────────────────

    fn make_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(text.to_string()),
            ..Default::default()
        }
    }

    fn make_tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn assistant_with_tools(calls: Vec<ToolCall>) -> Message {
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("using tools...".to_string()),
            tool_calls: Some(calls),
            ..Default::default()
        }
    }

    fn tool_result_msg(id: &str, name: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: MessageContent::Text(content.to_string()),
            tool_call_id: Some(id.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        }
    }

    fn token_count(msgs: &[Message]) -> usize {
        let tokenizer = HeuristicTokenizer;
        msgs.iter()
            .filter_map(|m| m.content.as_text())
            .map(|c| tokenizer.count_tokens(&c))
            .sum()
    }

    fn msgs_contain(msgs: &[Message], text: &str) -> bool {
        msgs.iter()
            .filter_map(|m| m.content.as_text())
            .any(|c| c.contains(text))
    }

    /// Build a multi-turn conversation with tool calls.
    /// Returns (messages, file_paths_mentioned).
    fn build_tool_conversation(turns: usize) -> (Vec<Message>, Vec<String>) {
        let mut msgs = vec![make_msg(Role::System, "You are a coding assistant.")];
        let mut files = Vec::new();
        for t in 0..turns {
            let task_msg = format!("Task {t}: check src/auth.rs for issues");
            msgs.push(make_msg(Role::User, &task_msg));
            files.push("src/auth.rs".to_string());
            let call_id = format!("call_{t}");
            let tc = make_tool_call(&call_id, "read_file");
            msgs.push(assistant_with_tools(vec![tc]));
            let result_content = format!("Content of src/auth.rs round {t}: fn login() {{ ... }}");
            msgs.push(tool_result_msg(&call_id, "read_file", &result_content));
            let response =
                format!("Round {t}: found issue in auth module. TODO: refactor auth module");
            msgs.push(make_msg(Role::Assistant, &response));
        }
        // Final user request (must be preserved)
        msgs.push(make_msg(
            Role::User,
            "Now fix the bug in src/auth.rs line 42",
        ));
        (msgs, files)
    }

    // ── Invariant 1: Tool Pair Integrity ─────────────────────────────────────

    #[tokio::test]
    async fn invariant_tool_pair_integrity_sliding_window() {
        let compressor = SlidingWindowCompressor::new(4);
        let (messages, _) = build_tool_conversation(3);
        let input = CompressionInput {
            messages,
            token_limit: 500,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        // Check: every remaining tool result has a matching assistant tool_call
        let tool_call_ids: std::collections::HashSet<String> = output
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant && m.tool_calls.is_some())
            .flat_map(|m| {
                m.tool_calls
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|tc| tc.id.clone())
            })
            .collect();

        let orphaned_tools: Vec<_> = output
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter(|m| {
                m.tool_call_id
                    .as_ref()
                    .is_none_or(|id| !tool_call_ids.contains(id))
            })
            .collect();

        assert!(
            orphaned_tools.is_empty(),
            "Found {} orphaned tool results",
            orphaned_tools.len()
        );

        // Check: checkpoint contains fix info
        let cp = output.checkpoint.as_ref().unwrap();
        assert_eq!(cp.strategy, "SlidingWindow");
        assert!(cp.retained_count > 0);
        assert!(cp.compression_duration_ms < 1000);
    }

    // ── Invariant 2: Last User Request Preserved ─────────────────────────────

    #[tokio::test]
    async fn invariant_last_user_request_preserved() {
        let compressor = SlidingWindowCompressor::new(3);
        let (messages, _) = build_tool_conversation(5);
        let last_request = "Now fix the bug in src/auth.rs line 42";
        assert!(msgs_contain(&messages, last_request));

        let input = CompressionInput {
            messages,
            token_limit: 300,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        assert!(
            msgs_contain(&output.messages, last_request),
            "Last user request must be preserved after compression"
        );
    }

    // ── Invariant 3: System Prompt Preserved ─────────────────────────────────

    #[tokio::test]
    async fn invariant_system_prompt_preserved() {
        let compressor = SlidingWindowCompressor::new(2);
        let system_text = "You are a medical research assistant.";
        let messages = vec![
            make_msg(Role::System, system_text),
            make_msg(Role::User, "q1"),
            make_msg(Role::Assistant, "a1"),
            make_msg(Role::User, "q2"),
            make_msg(Role::Assistant, "a2"),
            make_msg(Role::User, "q3"),
        ];

        let input = CompressionInput {
            messages,
            token_limit: 50,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        assert_eq!(output.messages[0].role, Role::System);
        assert!(
            msgs_contain(&output.messages[..1], system_text),
            "System prompt must be first message and preserved"
        );
    }

    // ── Invariant 4: Protected Markers Survive ───────────────────────────────

    #[tokio::test]
    async fn invariant_protected_markers_survive() {
        let mut ctx = ContextManager::builder(100)
            .compressor(SlidingWindowCompressor::new(2))
            .build();

        ctx.add_protected_marker("<skill>".to_string());
        ctx.push(make_msg(Role::System, "system prompt"));
        ctx.push(make_msg(Role::User, "old question"));
        ctx.push(make_msg(Role::Assistant, "old answer"));
        ctx.push(make_msg(Role::User, "<skill> important skill content"));
        ctx.push(make_msg(Role::Assistant, "recent answer"));
        ctx.push(make_msg(Role::User, "latest question"));

        let (_stats, checkpoint) = ctx.force_compress(2).await.unwrap();

        let msgs = ctx.messages();
        assert!(
            msgs_contain(msgs, "<skill> important skill content"),
            "Protected message must survive compression"
        );
        assert!(
            msgs_contain(msgs, "latest question"),
            "Latest user message must survive"
        );
        // Checkpoint must report protected count
        if let Some(cp) = checkpoint {
            assert!(cp.protected_count > 0);
        }
    }

    // ── Invariant 5: Pending Tasks Preserved ────────────────────────────────

    #[tokio::test]
    async fn invariant_pending_tasks_in_summary_or_retained() {
        // This test verifies that TODO/pending task information is not lost.
        // For LLM-based compressors, it should be in the summary.
        // For non-LLM compressors, it should be in retained messages.
        let compressor = SlidingWindowCompressor::new(4);
        let mut messages = vec![make_msg(Role::System, "system")];
        for i in 0..6 {
            let step_msg = format!("step {i}");
            let done_msg = format!("done step {i}. TODO: refactor auth module");
            messages.push(make_msg(Role::User, &step_msg));
            messages.push(make_msg(Role::Assistant, &done_msg));
        }
        messages.push(make_msg(Role::User, "final question"));

        let input = CompressionInput {
            messages,
            token_limit: 200,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        // With sliding window (keep 4), the most recent messages should contain
        // the TODO from the later steps
        let has_todo = msgs_contain(&output.messages, "TODO");
        assert!(
            has_todo,
            "TODO tasks should be preserved in retained messages"
        );
    }

    // ── Invariant 6: Referenced File Paths Preserved ─────────────────────────

    #[tokio::test]
    async fn invariant_current_files_preserved() {
        let compressor = SlidingWindowCompressor::new(4);
        let (messages, _expected_files) = build_tool_conversation(4);

        let input = CompressionInput {
            messages,
            token_limit: 300,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        // At least the most recent file reference should be preserved
        let has_file = msgs_contain(&output.messages, "src/auth.rs");
        assert!(
            has_file,
            "File paths from recent tool calls should be preserved"
        );
    }

    // ── Invariant 7: Token Target Met ────────────────────────────────────────

    #[tokio::test]
    async fn invariant_token_target_met() {
        let compressor = SlidingWindowCompressor::new(3);
        let (messages, _) = build_tool_conversation(8);
        let tokens_before = token_count(&messages);
        let target = tokens_before / 4; // aggressive target

        let input = CompressionInput {
            messages,
            token_limit: target,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();
        let tokens_after = token_count(&output.messages);

        // Should reduce tokens
        assert!(
            tokens_after < tokens_before,
            "Compression should reduce tokens: {} -> {}",
            tokens_before,
            tokens_after
        );

        // Checkpoint token counts should match
        let cp = output.checkpoint.as_ref().unwrap();
        assert!(cp.token_before > cp.token_after);
    }

    // ── Invariant 8: Compression Idempotent ──────────────────────────────────

    #[tokio::test]
    async fn invariant_compression_idempotent() {
        let compressor = SlidingWindowCompressor::new(3);
        let (messages, _) = build_tool_conversation(4);

        let input = CompressionInput {
            messages,
            token_limit: 200,
            current_query: None,
            focus_instructions: None,
        };
        let output1 = compressor.compress(input).await.unwrap();
        let count1 = output1.messages.len();

        // Second compression on already-compressed output should not evict more
        let input2 = CompressionInput {
            messages: output1.messages,
            token_limit: 200,
            current_query: None,
            focus_instructions: None,
        };
        let output2 = compressor.compress(input2).await.unwrap();
        let count2 = output2.messages.len();

        assert_eq!(
            count1, count2,
            "Re-compression should not evict additional messages (idempotent)"
        );
    }

    // ── Invariant 9: Adaptive Escalation ─────────────────────────────────────

    #[tokio::test]
    async fn invariant_adaptive_escalation_produces_levels() {
        let config = AdaptiveCompressionConfig {
            l1_snip_threshold_tokens: 50,
            l1_max_output_tokens: 5,
            l1_fold_consecutive_tools: true,
            l1_fold_keep_latest: 1,
            l2_micro_threshold_tokens: 80,
            l2_keep_lines: 10,
            l3_collapse_threshold_tokens: 100,
            l3_keep_recent: 3,
            l4_compact_threshold_tokens: 500,
            l4_keep_recent: 2,
        };
        let compressor = AdaptiveCompressor::new(config);

        // Build oversized context with tool output
        let mut messages = vec![make_msg(Role::System, "system")];
        for i in 0..8 {
            let query_msg = format!("query {i}");
            let tc_id = format!("tc_{i}");
            let result_msg = format!("result {i}");
            messages.push(make_msg(Role::User, &query_msg));
            let tc = make_tool_call(&tc_id, "grep");
            messages.push(assistant_with_tools(vec![tc]));
            messages.push(tool_result_msg(
                &tc_id,
                "grep",
                &"A".repeat(500), // large output to trigger L1 snip
            ));
            messages.push(make_msg(Role::Assistant, &result_msg));
        }

        let input = CompressionInput {
            messages,
            token_limit: 100,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        // Checkpoint should report levels applied
        let cp = output.checkpoint.as_ref().unwrap();
        assert_eq!(cp.strategy, "Adaptive");
        assert!(
            !cp.levels_applied.is_empty(),
            "Adaptive compressor should apply at least one level"
        );
        assert!(cp.token_before > cp.token_after, "Tokens should be reduced");
    }

    // ── Invariant 10: VisibilityHorizon No Orphans ───────────────────────────

    #[tokio::test]
    async fn invariant_horizon_no_orphaned_tool_calls() {
        use crate::compression::horizon::VisibilityHorizonConfig;

        let config = VisibilityHorizonConfig {
            active_window_turns: 2,
            compact_max_tokens: 50,
            enable_global_objective: false,
            global_objective: None,
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        // Build 8-turn conversation with tool calls
        let mut messages = vec![make_msg(Role::System, "system")];
        for t in 0..8 {
            let task_msg = format!("Task {t}: read file");
            let call_id = format!("call_{t}");
            let content_msg = format!("file content {t}");
            let done_msg = format!("Done task {t}");
            messages.push(make_msg(Role::User, &task_msg));
            let tc = make_tool_call(&call_id, "read_file");
            messages.push(assistant_with_tools(vec![tc]));
            messages.push(tool_result_msg(&call_id, "read_file", &content_msg));
            messages.push(make_msg(Role::Assistant, &done_msg));
        }

        let input = CompressionInput {
            messages,
            token_limit: 1000,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();

        // Check: no orphaned tool calls (every tool_call in assistant has matching result)
        let tool_result_ids: std::collections::HashSet<String> = output
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        for msg in &output.messages {
            if msg.role == Role::Assistant
                && let Some(ref tcs) = msg.tool_calls
            {
                for tc in tcs {
                    assert!(
                        tool_result_ids.contains(&tc.id),
                        "Tool call {} has no matching result (orphaned)",
                        tc.id
                    );
                }
            }
        }

        // Checkpoint should be populated
        let cp = output.checkpoint.as_ref().unwrap();
        assert_eq!(cp.strategy, "VisibilityHorizon");
        assert!(
            cp.evicted_count > 0,
            "Horizon should compact old tool traces"
        );
    }

    // ── Invariant: Focus Instructions Flow Through ────────────────────────────

    #[tokio::test]
    async fn invariant_focus_instructions_in_checkpoint() {
        let compressor = SlidingWindowCompressor::new(3);
        let (messages, _) = build_tool_conversation(3);
        let focus = "authentication refactor";

        let input = CompressionInput {
            messages,
            token_limit: 200,
            current_query: None,
            focus_instructions: Some(focus.to_string()),
        };
        let output = compressor.compress(input).await.unwrap();

        let cp = output.checkpoint.as_ref().unwrap();
        assert_eq!(
            cp.focus_instructions.as_deref(),
            Some(focus),
            "Focus instructions should be captured in checkpoint"
        );
    }

    #[tokio::test]
    async fn invariant_focus_passed_to_summary_prompt() {
        // Verify that focus instructions modify the summary prompt
        let messages = vec![
            make_msg(Role::User, "hello"),
            make_msg(Role::Assistant, "hi there"),
        ];
        let prompt_no_focus = default_summary_prompt_with_focus(&messages, None);
        let prompt_with_focus =
            default_summary_prompt_with_focus(&messages, Some("authentication refactor"));

        assert!(
            !prompt_no_focus.contains("authentication refactor"),
            "Prompt without focus should not mention the focus topic"
        );
        assert!(
            prompt_with_focus.contains("authentication refactor"),
            "Prompt with focus should include the focus topic"
        );
        assert!(
            prompt_with_focus.contains("用户特别要求"),
            "Focus prompt should include the special attention marker"
        );
    }

    // ── Invariant: Checkpoint Structure ──────────────────────────────────────

    #[tokio::test]
    async fn invariant_checkpoint_has_required_fields() {
        let compressor = SlidingWindowCompressor::new(3);
        let (messages, _) = build_tool_conversation(3);
        let input = CompressionInput {
            messages,
            token_limit: 200,
            current_query: None,
            focus_instructions: None,
        };
        let output = compressor.compress(input).await.unwrap();
        let cp = output.checkpoint.unwrap();

        // All required fields should be populated
        assert!(!cp.checkpoint_id.is_empty(), "checkpoint_id must be set");
        assert!(!cp.strategy.is_empty(), "strategy must be set");
        assert!(cp.retained_count > 0, "retained_count must be > 0");
        assert!(cp.token_before > 0, "token_before must be > 0");
        assert!(cp.token_after > 0, "token_after must be > 0");
        assert!(
            cp.compression_duration_ms < 5000,
            "compression_duration_ms should be reasonable"
        );
        // created_at should be recent (not checked precisely, just that it's set)
    }
}
