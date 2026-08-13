//! Store-backed memory promoter — L3 memory promotion from evicted messages.
//!
//! Implements [`MemoryPromoter`] to extract key facts from messages evicted
//! during compression and write them to a [`crate::memory::Store`] for later recall.
//!
//! # Fact extraction heuristic
//!
//! 1. **Assistant conclusions** — last paragraph of assistant messages (often summaries)
//! 2. **User questions** — original user inputs are useful context for recall
//! 3. **Keyword signals** — messages containing "decided", "important", "remember", "conclusion"
//! 4. **Skip**: tool results, system messages, very short messages (<50 chars)
//!
//! # Deduplication
//!
//! Facts are deduplicated by content hash: instead of a sequential counter,
//! the key is derived from a hash of the fact text. This means the same fact
//! extracted multiple times will overwrite (upsert) rather than create duplicates.
//!
use echo_core::llm::types::{Message, Role};
use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
use echo_state::compression::{MemoryPromoter, MemoryPromotionReceipt};
use futures::future::BoxFuture;
use std::sync::Arc;

/// Store-backed memory promoter.
///
/// Extracts key facts from messages evicted during compression and writes them
/// as typed memories (`MemoryMeta`) to the unified namespace
/// (`crate::evolution::layer::WARM_NAMESPACE` = `["agent","memories"]`) for
/// later recall. (stage4 A2: previously raw JSON to `["l3_promoted"]` —
/// 割裂点1: promoter-written facts now flow through the same typed path as
/// agent-written memories and are recallable by composite-score recall.)
pub struct StoreMemoryPromoter {
    layer_manager: Arc<crate::evolution::MemoryLayerManager>,
}

impl StoreMemoryPromoter {
    /// Create a promoter that writes through the canonical layered-memory
    /// authority (content-based dedup is always on).
    pub fn new(layer_manager: Arc<crate::evolution::MemoryLayerManager>) -> Self {
        Self { layer_manager }
    }

    /// Compute a deterministic content-based key for deduplication.
    ///
    /// Uses FNV-1a hash (deterministic across process restarts) to ensure
    /// the same fact always maps to the same key, enabling cross-session dedup.
    fn content_key(fact: &str) -> String {
        durable_memory_content_key(fact)
    }
}

/// Stable content-derived key shared by every compression memory writer.
pub(crate) fn durable_memory_content_key(content: &str) -> String {
    let hash = echo_core::utils::hash::fnv1a_64(content.trim().as_bytes());
    format!("l3_{hash:016x}")
}

impl MemoryPromoter for StoreMemoryPromoter {
    fn promote(
        &self,
        evicted: &[Message],
    ) -> BoxFuture<'_, echo_core::error::Result<MemoryPromotionReceipt>> {
        let facts = extract_key_facts(evicted);
        let submitted = facts.len();
        let layer_manager = self.layer_manager.clone();

        Box::pin(async move {
            let mut promoted = 0usize;
            let mut deduplicated = 0usize;
            for (fact, fact_type) in facts.into_iter() {
                let key = Self::content_key(&fact);
                let memory_type = fact_type_to_memory_type(fact_type);
                let recall_weight = if contains_signal_word(&fact) {
                    0.7
                } else {
                    0.4
                };
                let meta = MemoryMeta::new(memory_type, MemorySource::L3Promotion, "l3_promotion")
                    .with_recall_weight(recall_weight);
                if layer_manager
                    .locate(&key)
                    .await
                    .is_some_and(|(_, existing)| existing.content.trim() == fact.trim())
                {
                    deduplicated = deduplicated.saturating_add(1);
                    continue;
                }
                layer_manager.write_memory(&key, &fact, meta).await?;
                promoted = promoted.saturating_add(1);
            }
            Ok(MemoryPromotionReceipt {
                submitted,
                promoted,
                deduplicated,
            })
        })
    }
}

/// Map a promoter fact-type string to a `MemoryType`.
fn fact_type_to_memory_type(fact_type: &str) -> MemoryType {
    match fact_type {
        "error" => MemoryType::ErrorResolution,
        "decision" => MemoryType::ArchitectureDecision,
        "user_preference" => MemoryType::UserPreference,
        "pending_task" | "user_query" => MemoryType::WorkflowPattern,
        "tool_output" | "file_reference" | "assistant_conclusion" | "general" => {
            MemoryType::ProjectFact
        }
        _ => MemoryType::ProjectFact,
    }
}

/// Extract key facts from evicted messages using heuristics.
fn extract_key_facts(messages: &[Message]) -> Vec<(String, &'static str)> {
    let mut facts = Vec::new();

    for msg in messages {
        let text = match msg.content.as_text() {
            Some(t) if t.chars().count() >= 50 => t,
            _ => continue, // Skip short or empty messages
        };

        match msg.role {
            Role::System => {
                // Skip system messages — they're not conversation facts
                continue;
            }
            Role::Tool => {
                // Extract a tool digest: key findings from tool outputs
                if let Some(digest) = tool_digest(&text) {
                    facts.push((truncate_fact(&digest), "tool_output"));
                }
                continue;
            }
            Role::Assistant => {
                // Extract conclusion-like content from assistant messages
                if contains_signal_word(&text) {
                    facts.push((truncate_fact(&text), classify_fact_type(&text, &msg.role)));
                } else {
                    // Take the last paragraph as a potential conclusion
                    if let Some(last_para) = last_paragraph(&text)
                        && last_para.chars().count() >= 50
                    {
                            facts.push((
                                truncate_fact(&last_para),
                                classify_fact_type(&last_para, &msg.role),
                            ));
                        }
                }
            }
            Role::User
                // User questions are useful context for recall
                if text.chars().count() >= 50 && !text.starts_with('[') => {
                    facts.push((truncate_fact(&text), classify_fact_type(&text, &msg.role)));
                }
            _ => {}
        }
    }

    facts
}

/// Classify a fact by content patterns.
fn classify_fact_type(text: &str, role: &Role) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("todo") || lower.contains("fixme") || lower.contains("待处理") {
        "pending_task"
    } else if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("panic")
        || lower.contains("失败")
        || lower.contains("错误")
    {
        "error"
    } else if lower.contains("decided") || lower.contains("decision") || lower.contains("决定") {
        "decision"
    } else if lower.contains(".rs")
        || lower.contains(".py")
        || lower.contains(".js")
        || lower.contains(".toml")
        || lower.contains("src/")
        || lower.contains("/file")
    {
        "file_reference"
    } else if lower.contains("prefer")
        || lower.contains("don't")
        || lower.contains("must")
        || lower.contains("should")
        || lower.contains("不要")
        || lower.contains("必须")
    {
        "user_preference"
    } else {
        if *role == Role::User {
            "user_query"
        } else if *role == Role::Assistant {
            "assistant_conclusion"
        } else {
            "general"
        }
    }
}

/// Extract a concise digest from tool output — key findings, errors, file paths.
/// Returns `None` if nothing meaningful can be extracted.
fn tool_digest(text: &str) -> Option<String> {
    let mut highlights: Vec<&str> = Vec::new();

    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error")
            || lower.contains("fail")
            || lower.contains("panic")
            || lower.contains("test result")
            || lower.contains("pass")
            || lower.contains("失败")
            || lower.contains("错误")
            || lower.contains("异常")
            || lower.contains("测试")
            || lower.contains("通过")
            || ((lower.contains(".rs")
                || lower.contains(".py")
                || lower.contains(".js")
                || lower.contains(".toml")
                || lower.contains("src/"))
                && line.chars().count() < 200)
        {
            highlights.push(line.trim());
        }
    }

    if highlights.is_empty() {
        None
    } else {
        Some(
            highlights
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
        )
    }
}

/// Check if text contains signal words indicating important facts.
fn contains_signal_word(text: &str) -> bool {
    let lower = text.to_lowercase();
    const SIGNALS: &[&str] = &[
        "decided",
        "decision",
        "conclusion",
        "important",
        "remember",
        "discovered",
        "found that",
        "result:",
        "summary:",
        "决定",
        "结论",
        "发现",
        "重要",
        "总结",
    ];
    SIGNALS.iter().any(|s| lower.contains(s))
}

/// Extract the last paragraph from text (separated by double newline).
fn last_paragraph(text: &str) -> Option<String> {
    text.rsplit("\n\n").next().map(|s| s.trim().to_string())
}

/// Truncate a fact to a reasonable length (~300 chars), UTF-8 safe.
fn truncate_fact(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 300 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(300).collect::<String>())
    }
}

/// FNV-1a hash — deterministic across process restarts.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_key_facts_skips_short_messages() {
        let messages = vec![
            Message::user("Hi".to_string()),      // too short
            Message::assistant("OK".to_string()), // too short
        ];
        let facts = extract_key_facts(&messages);
        assert!(facts.is_empty());
    }

    #[test]
    fn test_extract_key_facts_skips_tool_results() {
        let messages = vec![Message::tool_result(
            "call_1".into(),
            "read_file".into(),
            "x".repeat(500), // long tool output
        )];
        let facts = extract_key_facts(&messages);
        assert!(facts.is_empty(), "Tool results should be skipped");
    }

    #[test]
    fn test_extract_key_facts_signal_words() {
        let messages = vec![Message::assistant(
            "After analysis, we decided to use PostgreSQL for the database layer. \
             This provides better JSON support and full-text search capabilities."
                .to_string(),
        )];
        let facts = extract_key_facts(&messages);
        assert_eq!(facts.len(), 1);
        assert!(
            facts
                .first()
                .is_some_and(|(fact, _)| fact.contains("PostgreSQL"))
        );
    }

    #[test]
    fn test_extract_key_facts_user_questions() {
        let messages = vec![Message::user(
            "How should we handle authentication in the microservice architecture? \
             We need to support both OAuth2 and API keys."
                .to_string(),
        )];
        let facts = extract_key_facts(&messages);
        assert_eq!(facts.len(), 1);
        assert!(
            facts
                .first()
                .is_some_and(|(fact, _)| fact.contains("authentication"))
        );
    }

    #[test]
    fn durable_key_is_shared_for_equivalent_content() {
        assert_eq!(
            durable_memory_content_key("user prefers Rust"),
            durable_memory_content_key("  user prefers Rust\n")
        );
    }

    #[test]
    fn test_truncate_fact() {
        let short = "This is a short fact.";
        assert_eq!(truncate_fact(short), short);

        let long = "x".repeat(500);
        let truncated = truncate_fact(&long);
        assert!(truncated.len() <= 310); // 300 + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_contains_signal_word() {
        assert!(contains_signal_word("We decided to use Rust"));
        assert!(contains_signal_word("这是一个重要的决定"));
        assert!(!contains_signal_word("Hello world"));
    }

    /// RFC 5.2.2: End-to-end quality verification.
    ///
    /// Builds a conversation with important facts → extracts key facts →
    /// writes to an in-memory Store → verifies the facts are recallable.
    #[tokio::test]
    async fn test_e2e_fact_extraction_and_recall() -> echo_core::error::Result<()> {
        use crate::evolution::MemoryLayerManager;
        use crate::evolution::audit::NullChangeLog;
        use echo_core::memory::Store;
        use echo_state::memory::InMemoryStore;

        let store = Arc::new(InMemoryStore::new());
        let memory_dir = tempfile::tempdir()?;
        let layer_manager = Arc::new(MemoryLayerManager::new(
            memory_dir.path().to_path_buf(),
            store.clone(),
            Box::new(NullChangeLog),
        ));
        let promoter = StoreMemoryPromoter::new(layer_manager);

        // Build a conversation with important facts that should be preserved
        let messages = vec![
            // This assistant message contains a decision (signal word)
            Message::assistant(
                "After thorough analysis, we decided to use PostgreSQL for the database layer \
                 because it provides better JSON support and full-text search capabilities. \
                 This is an important architectural decision."
                    .to_string(),
            ),
            // This user message is a meaningful question
            Message::user(
                "How should we handle authentication in the microservice architecture? \
                 We need to support both OAuth2 and API keys for different clients."
                    .to_string(),
            ),
            // This assistant message has a conclusion in the last paragraph
            Message::assistant(
                "Let me analyze the authentication options available.\n\n\
                 In conclusion, we recommend using JWT tokens with OAuth2 for external \
                 clients and API keys for internal service-to-service communication. \
                 This provides the best balance of security and simplicity."
                    .to_string(),
            ),
            // This should be skipped (tool result)
            Message::tool_result(
                "call_1".into(),
                "read_file".into(),
                "file contents here that are not important facts".repeat(20),
            ),
            // This should be skipped (too short)
            Message::user("ok".to_string()),
        ];

        // Step 1: Extract facts
        let facts = extract_key_facts(&messages);
        assert!(
            facts.len() >= 2,
            "Should extract at least 2 facts (decision + user question), got {}",
            facts.len()
        );

        // Verify extracted facts contain key information
        let all_facts: String = facts
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            all_facts.contains("PostgreSQL"),
            "Should preserve the database decision fact"
        );
        assert!(
            all_facts.contains("authentication") || all_facts.contains("microservice"),
            "Should preserve the user's question about authentication"
        );

        // Step 2: Promote to Store (writes via the promoter)
        promoter.promote(&messages).await?;

        // Step 3: Verify facts are recallable via Store search
        let results = store.search(&["agent", "memories"], "PostgreSQL", 5).await;
        assert!(results.is_ok(), "Store search should succeed");
        let items = results.unwrap_or_default();
        assert!(
            !items.is_empty(),
            "Should be able to recall the PostgreSQL decision fact from Store"
        );

        // Step 4: Verify quality — the recalled item contains meaningful content
        let content = items
            .first()
            .and_then(|recalled| recalled.value.get("content"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            content.contains("PostgreSQL"),
            "Recalled content should contain the key fact, got: {}",
            content
        );
        assert!(
            content.len() >= 50,
            "Recalled content should be meaningful (≥50 chars), got {} chars",
            content.len()
        );
        Ok(())
    }
}
