//! Store-backed memory promoter — L3 memory promotion from evicted messages.
//!
//! Implements [`MemoryPromoter`] to extract key facts from messages evicted
//! during compression and write them to a [`Store`] for later recall.
//!
//! # Fact extraction heuristic
//!
//! 1. **Assistant conclusions** — last paragraph of assistant messages (often summaries)
//! 2. **User questions** — original user inputs are useful context for recall
//! 3. **Keyword signals** — messages containing "decided", "important", "remember", "conclusion"
//! 4. **Skip**: tool results, system messages, very short messages (<50 chars)

use echo_core::llm::types::{Message, Role};
use echo_core::memory::Store;
use echo_state::compression::MemoryPromoter;
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

/// Namespace for L3 promoted memory items.
const L3_NAMESPACE: &[&str] = &["l3_promoted"];

/// Store-backed memory promoter.
///
/// Extracts key facts from evicted messages and writes them to a [`Store`]
/// under the `l3_promoted` namespace.
pub struct StoreMemoryPromoter {
    store: Arc<dyn Store>,
    /// Counter for unique key generation.
    counter: std::sync::atomic::AtomicU64,
}

impl StoreMemoryPromoter {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl MemoryPromoter for StoreMemoryPromoter {
    fn promote(&self, evicted: &[Message]) -> BoxFuture<'_, ()> {
        let facts = extract_key_facts(evicted);
        let store = self.store.clone();
        let seq = self
            .counter
            .fetch_add(facts.len() as u64, std::sync::atomic::Ordering::Relaxed);

        Box::pin(async move {
            for (i, fact) in facts.into_iter().enumerate() {
                let key = format!("l3_{:06}_{}", seq + i as u64, timestamp_short());
                let value = json!({
                    "content": fact,
                    "source": "l3_memory_promotion",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                if let Err(e) = store.put(L3_NAMESPACE, &key, value).await {
                    tracing::debug!(
                        error = %e,
                        "Failed to write promoted memory item"
                    );
                }
            }
        })
    }
}

/// Extract key facts from evicted messages using heuristics.
fn extract_key_facts(messages: &[Message]) -> Vec<String> {
    let mut facts = Vec::new();

    for msg in messages {
        let text = match msg.content.as_text() {
            Some(t) if t.len() >= 50 => t,
            _ => continue, // Skip short or empty messages
        };

        match msg.role {
            Role::System => {
                // Skip system messages — they're not conversation facts
                continue;
            }
            Role::Tool => {
                // Skip tool results — already summarized by horizon compressor
                continue;
            }
            Role::Assistant => {
                // Extract conclusion-like content from assistant messages
                if contains_signal_word(&text) {
                    facts.push(truncate_fact(&text));
                } else {
                    // Take the last paragraph as a potential conclusion
                    if let Some(last_para) = last_paragraph(&text) {
                        if last_para.len() >= 50 {
                            facts.push(truncate_fact(&last_para));
                        }
                    }
                }
            }
            Role::User => {
                // User questions are useful context for recall
                if text.len() >= 50 && !text.starts_with('[') {
                    facts.push(truncate_fact(&text));
                }
            }
            _ => {}
        }
    }

    facts
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

/// Truncate a fact to a reasonable length (~200 chars).
fn truncate_fact(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= 300 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..300])
    }
}

/// Short timestamp for unique key suffix.
fn timestamp_short() -> String {
    let now = chrono::Utc::now();
    now.format("%Y%m%d%H%M%S").to_string()
}

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
        assert!(facts[0].contains("PostgreSQL"));
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
        assert!(facts[0].contains("authentication"));
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
    async fn test_e2e_fact_extraction_and_recall() {
        use echo_state::memory::InMemoryStore;

        let store = Arc::new(InMemoryStore::new());
        let promoter = StoreMemoryPromoter::new(store.clone());

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
        let all_facts = facts.join(" ");
        assert!(
            all_facts.contains("PostgreSQL"),
            "Should preserve the database decision fact"
        );
        assert!(
            all_facts.contains("authentication") || all_facts.contains("microservice"),
            "Should preserve the user's question about authentication"
        );

        // Step 2: Promote to Store (writes via the promoter)
        promoter.promote(&messages).await;

        // Step 3: Verify facts are recallable via Store search
        let results = store.search(L3_NAMESPACE, "PostgreSQL", 5).await;
        assert!(results.is_ok(), "Store search should succeed");
        let items = results.unwrap();
        assert!(
            !items.is_empty(),
            "Should be able to recall the PostgreSQL decision fact from Store"
        );

        // Step 4: Verify quality — the recalled item contains meaningful content
        let recalled = &items[0];
        let content = recalled
            .value
            .get("content")
            .and_then(|v| v.as_str())
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
    }
}
