//! Three-source fusion intent classifier.
//!
//! Combines keyword matching (0 token, fast), LLM semantic classification
//! (~500 tokens, lazy), and hook activation suggestions (from
//! UserPromptSubmit hooks) into a single [`IntentClassifier`]
//! implementation.

use crate::intent::classifier::{KeywordClassifier, LlmIntentClassifier};
use crate::intent::{Intent, IntentClassifier};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};

/// Confidence threshold for fast-path keyword bypass.
const HIGH_CONFIDENCE: f32 = 0.7;

/// Three-source fusion supervisor implementing [`IntentClassifier`].
pub struct TriggerSupervisor {
    keyword_classifier: KeywordClassifier,
    llm_classifier: LlmIntentClassifier,
    /// Shared slot written by the prepare phase after UserPromptSubmit hooks
    /// resolve.  P1 writes `fire_lifecycle_hook().activate_skill` here; the
    /// supervisor reads (and consumes) it once per classification.
    hook_activation_slot: Arc<Mutex<Option<(String, String)>>>,
}

impl TriggerSupervisor {
    pub fn new(
        keyword_classifier: KeywordClassifier,
        llm_classifier: LlmIntentClassifier,
        hook_activation_slot: Arc<Mutex<Option<(String, String)>>>,
    ) -> Self {
        Self {
            keyword_classifier,
            llm_classifier,
            hook_activation_slot,
        }
    }

    /// Fuse three sources into a final intent.
    ///
    /// Rules (order matters):
    /// 1. Keyword high confidence (f32 ≥ 0.7) → fast path, skip LLM
    /// 2. LLM high confidence → adopt LLM
    /// 3. Both low but hook slot has activation → adopt hook
    /// 4. All low → Fallback
    fn fuse(kw: Intent, llm: Intent, hook: Option<(String, String)>) -> Intent {
        if kw.confidence().unwrap_or(0.0) >= HIGH_CONFIDENCE {
            return kw;
        }
        if llm.confidence().unwrap_or(0.0) >= HIGH_CONFIDENCE {
            return llm;
        }
        if let Some((skill_name, _reason)) = hook {
            return Intent::SkillRequired {
                skill_name,
                confidence: 0.6,
            };
        }
        Intent::Fallback
    }
}

impl IntentClassifier for TriggerSupervisor {
    fn classify<'a>(
        &'a self,
        user_input: &'a str,
        context: &'a [crate::llm::types::Message],
    ) -> BoxFuture<'a, Intent> {
        Box::pin(async move {
            // First: keyword classifier (0 tokens, fast)
            let kw = self.keyword_classifier.classify(user_input, context).await;

            // Fast path: skip LLM entirely
            if kw.confidence().unwrap_or(0.0) >= HIGH_CONFIDENCE {
                return kw;
            }

            // Second: LLM semantic classifier
            let llm = self.llm_classifier.classify(user_input, context).await;

            // Third: hook activation from this turn's UserPromptSubmit
            let hook = self
                .hook_activation_slot
                .lock()
                .ok()
                .and_then(|mut guard| guard.take());

            Self::fuse(kw, llm, hook)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_high_confidence_takes_priority() {
        let kw = Intent::SkillRequired {
            skill_name: "docx".into(),
            confidence: 0.9,
        };
        let llm = Intent::SkillRequired {
            skill_name: "pdf".into(),
            confidence: 0.85,
        };
        let result = TriggerSupervisor::fuse(kw, llm, None);
        assert_eq!(result.skill_name().unwrap(), "docx");
    }

    #[test]
    fn llm_wins_when_keyword_low() {
        let kw = Intent::SkillRequired {
            skill_name: "pdf".into(),
            confidence: 0.3,
        };
        let llm = Intent::SkillRequired {
            skill_name: "docx".into(),
            confidence: 0.85,
        };
        let result = TriggerSupervisor::fuse(kw, llm, None);
        assert_eq!(result.skill_name().unwrap(), "docx");
    }

    #[test]
    fn hook_adopted_when_both_low() {
        let hook = Some(("docx".into(), "detected .docx".into()));
        let result = TriggerSupervisor::fuse(Intent::Fallback, Intent::Fallback, hook);
        assert_eq!(result.skill_name().unwrap(), "docx");
        assert!(result.confidence().unwrap() >= 0.5);
    }

    #[test]
    fn all_low_returns_fallback() {
        let r = TriggerSupervisor::fuse(Intent::Fallback, Intent::Fallback, None);
        assert!(matches!(r, Intent::Fallback));
    }

    #[test]
    fn keyword_beats_llm_even_when_both_high() {
        let kw = Intent::SkillRequired {
            skill_name: "brainstorming".into(),
            confidence: 0.95,
        };
        let llm = Intent::SkillRequired {
            skill_name: "writing-plans".into(),
            confidence: 0.9,
        };
        let r = TriggerSupervisor::fuse(kw, llm, None);
        assert_eq!(r.skill_name().unwrap(), "brainstorming");
    }
}
