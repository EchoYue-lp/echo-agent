//! Built-in intent classifier implementations.
//!
//! Two classification strategies are provided:
//!
//! | Strategy | When to use | Cost |
//! |----------|-------------|------|
//! | [`KeywordClassifier`] | Fast zero-cost first pass; product layer supplies all triggers | 0 tokens |
//! | [`LlmIntentClassifier`] | Semantic understanding; works with any skill set | ~500 tokens |
//!
//! Use [`ChainedClassifier`] to combine them: try keywords first, fall back to LLM.

use super::{Intent, IntentClassifier};
use crate::llm::LlmClient;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

// ── KeywordClassifier ────────────────────────────────────────────────

/// Zero-cost keyword-based intent classifier.
///
/// Maps trigger words/phrases to [`Intent::SkillRequired`] or
/// [`Intent::DirectAnswer`].  If no keyword matches, returns
/// [`Intent::Fallback`].
///
/// **The classifier contains zero hardcoded keywords.**  All triggers
/// are supplied by the product layer — e.g. derived from skill
/// `description` text, since the standard `SKILL.md` format has no
/// trigger field.
///
/// # Example
///
/// ```
/// use echo_agent::intent::{KeywordClassifier, Intent};
///
/// let mut classifier = KeywordClassifier::default();
/// // Product layer registers explicit triggers for programmatic descriptors.
/// // Standard SKILL.md files have no trigger frontmatter field.
/// classifier.add_skill_keywords("coding", &["写代码", "编程", "debug", "refactor"]);
/// classifier.add_skill_keywords("paper-search", &["论文", "文献", "arxiv"]);
/// // Product layer registers direct-answer keywords
/// classifier.add_direct_keywords(&["你好", "hello", "thanks"]);
/// ```
#[derive(Clone)]
pub struct KeywordClassifier {
    /// trigger_word -> all owning skill names (case-insensitive)
    skill_keywords: HashMap<String, Vec<String>>,
    /// trigger phrases for DirectAnswer (case-insensitive)
    direct_keywords: Vec<String>,
    /// Whether DirectAnswer routing is enabled
    enable_direct_answer: bool,
}

impl Default for KeywordClassifier {
    fn default() -> Self {
        Self {
            skill_keywords: HashMap::new(),
            direct_keywords: Vec::new(),
            enable_direct_answer: true,
        }
    }
}

impl KeywordClassifier {
    /// Create an empty classifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register keywords that trigger a skill activation.
    ///
    /// Typically called by the product layer for programmatic descriptors.
    /// Standard `SKILL.md` files have no trigger frontmatter field; callers
    /// may derive terms from descriptions when a keyword fast path is desired.
    pub fn add_skill_keywords(&mut self, skill_name: &str, trigger_words: &[&str]) {
        for word in trigger_words {
            let owners = self.skill_keywords.entry(word.to_lowercase()).or_default();
            if !owners.iter().any(|owner| owner == skill_name) {
                owners.push(skill_name.to_string());
                owners.sort();
            }
        }
    }

    /// Batch-register skill keywords from a map of skill_name → triggers.
    ///
    /// Convenience method for loading all skills at once.
    pub fn add_skill_keyword_map(&mut self, map: &HashMap<String, Vec<String>>) {
        for (skill_name, triggers) in map {
            let refs: Vec<&str> = triggers.iter().map(|s| s.as_str()).collect();
            self.add_skill_keywords(skill_name, &refs);
        }
    }

    /// Register keywords that trigger a direct answer (no skill, no ReAct).
    pub fn add_direct_keywords(&mut self, trigger_words: &[&str]) {
        for word in trigger_words {
            self.direct_keywords.push(word.to_lowercase());
        }
    }

    /// Enable or disable the DirectAnswer shortcut.
    pub fn set_enable_direct_answer(&mut self, enable: bool) {
        self.enable_direct_answer = enable;
    }

    fn classify_inner(&self, input: &str) -> Intent {
        let lower = input.trim().to_lowercase();

        // 1. SkillRequired: weighted score-based keyword matching.
        //    Longer triggers get higher weight (more specific = more reliable).
        //    Multiple triggers matching the same skill boosts confidence.
        //    If two skills score similarly, confidence drops (ambiguous input).
        let mut skill_scores: HashMap<&str, f32> = HashMap::new();
        for (trigger, skills) in &self.skill_keywords {
            if let Some(weight) = Self::match_weight(&lower, trigger) {
                for skill in skills {
                    *skill_scores.entry(skill.as_str()).or_insert(0.0) += weight;
                }
            }
        }

        if !skill_scores.is_empty() {
            // Sort by score descending
            let mut scored: Vec<(&str, f32)> = skill_scores.into_iter().collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));

            let Some((best_skill, best_score)) = scored.first().copied() else {
                return Intent::Fallback;
            };
            let runner_up_score = scored.get(1).map(|(_, s)| *s).unwrap_or(0.0);
            if scored
                .get(1)
                .is_some_and(|(_, score)| (*score - best_score).abs() <= f32::EPSILON)
            {
                return Intent::Fallback;
            }

            // Confidence based on:
            // - Absolute score (higher = more triggers matched)
            // - Gap to runner-up (bigger gap = less ambiguous)
            let gap_ratio = if best_score > 0.0 {
                (best_score - runner_up_score) / best_score
            } else {
                0.0
            };

            // Base confidence from score (0.5 at score=1, scaling up)
            let score_confidence = (0.5 + best_score * 0.1).min(0.95);
            // Penalty for ambiguity (small gap = ambiguous)
            let ambiguity_penalty = if gap_ratio < 0.3 { 0.15 } else { 0.0 };
            let confidence = (score_confidence - ambiguity_penalty).max(0.3);

            // Only accept keyword match if confidence >= 0.7
            // Lower confidence → Fallback → let LLM classifier or ReAct handle it
            if confidence >= 0.7 {
                return Intent::SkillRequired {
                    skill_name: best_skill.to_string(),
                    confidence,
                };
            }
        }

        // 2. DirectAnswer: explicit keywords registered by product layer
        if self.enable_direct_answer {
            for word in &self.direct_keywords {
                if Self::match_weight(&lower, word).is_some() {
                    return Intent::DirectAnswer { confidence: 0.85 };
                }
            }
        }

        // 3. No match — let downstream decide (LLM fallback or full ReAct)
        Intent::Fallback
    }

    /// Match a trigger against input and return a weight score.
    ///
    /// Returns `None` if no match, `Some(weight)` where weight reflects
    /// match quality:
    /// - Longer triggers → higher weight (more specific)
    /// - CJK triggers ≥2 chars → weight = char count
    /// - ASCII triggers with word boundary → weight = word count
    /// - Short triggers (1 char CJK, <3 char ASCII) → lower weight
    fn match_weight(input: &str, trigger: &str) -> Option<f32> {
        let has_cjk = trigger.chars().any(|c| c > '\u{2E80}');

        if has_cjk {
            // CJK triggers: require at least 2 characters (filter single-char noise)
            let char_count = trigger.chars().count();
            if char_count < 2 {
                return None;
            }
            if input.contains(trigger) {
                // Weight by character count: longer = more specific
                Some(char_count as f32)
            } else {
                None
            }
        } else {
            // ASCII/Latin triggers: require word-boundary match
            if let Some(pos) = input.find(trigger) {
                let before_ok = pos == 0 || !input.as_bytes()[pos - 1].is_ascii_alphanumeric();
                let end = pos + trigger.len();
                let after_ok = end >= input.len() || !input.as_bytes()[end].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    // Weight by word count in trigger (multi-word = more specific)
                    let word_count = trigger.split_whitespace().count().max(1);
                    Some(word_count as f32 * 2.0)
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}

impl IntentClassifier for KeywordClassifier {
    fn classify<'a>(
        &'a self,
        user_input: &'a str,
        _context: &'a [Message],
    ) -> BoxFuture<'a, Intent> {
        let intent = self.classify_inner(user_input);
        Box::pin(async move { intent })
    }
}

// ── KeywordClassifierConfig ────────────────────────────────────────────

/// Configuration for building a [`KeywordClassifier`] from static data.
#[derive(Debug, Clone, Default)]
pub struct KeywordClassifierConfig {
    pub skill_keyword_map: Vec<(String, Vec<String>)>,
    pub direct_keywords: Vec<String>,
    pub enable_direct_answer: bool,
}

impl KeywordClassifierConfig {
    pub fn build(self) -> KeywordClassifier {
        let mut classifier = KeywordClassifier::new();
        classifier.set_enable_direct_answer(self.enable_direct_answer);
        for (skill, triggers) in self.skill_keyword_map {
            let refs: Vec<&str> = triggers.iter().map(|s| s.as_str()).collect();
            classifier.add_skill_keywords(&skill, &refs);
        }
        classifier.add_direct_keywords(
            &self
                .direct_keywords
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
        );
        classifier
    }
}

// ── LlmIntentClassifier ──────────────────────────────────────────────

/// A skill description used by the LLM classifier to understand available skills.
#[derive(Debug, Clone)]
pub struct SkillDescription {
    /// Skill identifier (e.g. "coding", "paper-search")
    pub name: String,
    /// One-line description of what this skill does
    pub description: String,
    /// Example trigger phrases (for the LLM to understand intent patterns)
    pub example_triggers: Vec<String>,
}

/// LLM-based intent classifier using a lightweight classification prompt.
///
/// Instead of relying on exhaustive keyword lists, sends the user input
/// along with available skill descriptions to an LLM, which returns a
/// structured classification result.
///
/// # Classification Prompt
///
/// The classifier constructs a prompt like:
///
/// ```text
/// You are an intent classifier. Given the user input, decide:
/// - DIRECT: simple greeting/question, answer directly
/// - SKILL: needs a specific skill
/// - FALLBACK: complex task, needs full agent loop
///
/// Available skills:
/// 1. coding — Code generation, debugging, refactoring
/// 2. paper-search — Academic literature search
/// ...
///
/// User input: {input}
/// Respond in JSON: {"intent": "DIRECT|SKILL|FALLBACK", "skill": "name", "confidence": 0.0-1.0}
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use echo_agent::intent::classifier::{LlmIntentClassifier, SkillDescription};
///
/// let skills = vec![
///     SkillDescription {
///         name: "coding".into(),
///         description: "Code generation, debugging, refactoring".into(),
///         example_triggers: vec!["帮我写个排序算法".into()],
///     },
/// ];
/// let classifier = LlmIntentClassifier::new(llm_client, skills);
/// ```
pub struct LlmIntentClassifier {
    llm: Arc<dyn LlmClient>,
    skills: Vec<SkillDescription>,
}

impl LlmIntentClassifier {
    pub fn new(llm: Arc<dyn LlmClient>, skills: Vec<SkillDescription>) -> Self {
        Self { llm, skills }
    }

    /// Build the classification prompt for a given user input.
    fn build_prompt(&self, user_input: &str) -> String {
        let skill_list = if self.skills.is_empty() {
            "(no skills available)".to_string()
        } else {
            self.skills
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let examples = if s.example_triggers.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " (examples: {})",
                            s.example_triggers
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    format!("{}. {} — {}{}", i + 1, s.name, s.description, examples)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            r#"You are an intent classifier for an AI agent system.

Given the user's input, classify it into exactly ONE of:
- DIRECT: simple greeting, casual chat, or a straightforward factual question that can be answered directly
- SKILL: a task that matches one of the available specialized skills
- FALLBACK: a complex or ambiguous task that needs the full agent reasoning loop

Available skills:
{skill_list}

Rules:
- If the input clearly matches a skill's domain, choose SKILL and specify which skill
- If the input is a simple greeting, weather query, time query, or short factual question, choose DIRECT
- If unsure or the task is complex/multi-step, choose FALLBACK
- Confidence should be 0.0-1.0; only return DIRECT or SKILL if confidence >= 0.7

User input: {user_input}

Respond ONLY with valid JSON (no markdown):
{{"intent": "DIRECT|SKILL|FALLBACK", "skill": "skill_name_or_null", "confidence": 0.0}}"#
        )
    }

    /// Parse the LLM's JSON response into an Intent.
    fn parse_response(response: &str) -> Intent {
        // Extract JSON from response (handle potential markdown wrapping)
        let json_str = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        // Try to parse as JSON
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
            let confidence = parsed
                .get("confidence")
                .and_then(|v| v.as_f64())
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                .unwrap_or(0.0) as f32;

            match parsed
                .get("intent")
                .and_then(|v| v.as_str())
                .unwrap_or("FALLBACK")
            {
                "DIRECT" => Intent::DirectAnswer { confidence },
                "SKILL" => {
                    if let Some(skill_name) = parsed
                        .get("skill")
                        .and_then(|v| v.as_str())
                        .filter(|s| *s != "null")
                    {
                        Intent::SkillRequired {
                            skill_name: skill_name.to_string(),
                            confidence,
                        }
                    } else {
                        Intent::Fallback
                    }
                }
                _ => Intent::Fallback,
            }
        } else {
            // Failed to parse JSON — fallback
            Intent::Fallback
        }
    }
}

impl IntentClassifier for LlmIntentClassifier {
    fn classify<'a>(
        &'a self,
        user_input: &'a str,
        _context: &'a [Message],
    ) -> BoxFuture<'a, Intent> {
        let prompt = self.build_prompt(user_input);
        let llm = self.llm.clone();

        Box::pin(async move {
            let messages = vec![Message::user(prompt)];
            match llm.chat_simple(messages).await {
                Ok(response) => Self::parse_response(&response),
                Err(e) => {
                    tracing::warn!(error = %e, "LLM intent classification failed, falling back");
                    Intent::Fallback
                }
            }
        })
    }
}

// ── ChainedClassifier ────────────────────────────────────────────────

/// Chains multiple classifiers: tries each in order, returns the first
/// non-`Fallback` result.
///
/// # Typical Usage
///
/// ```rust,ignore
/// use echo_agent::intent::classifier::{ChainedClassifier, KeywordClassifier, LlmIntentClassifier};
///
/// let chain = ChainedClassifier::new(vec![
///     Box::new(keyword_classifier),  // fast, zero-cost
///     Box::new(llm_classifier),      // semantic fallback
/// ]);
/// ```
pub struct ChainedClassifier {
    classifiers: Vec<Box<dyn IntentClassifier>>,
}

impl ChainedClassifier {
    pub fn new(classifiers: Vec<Box<dyn IntentClassifier>>) -> Self {
        Self { classifiers }
    }
}

impl IntentClassifier for ChainedClassifier {
    fn classify<'a>(
        &'a self,
        user_input: &'a str,
        context: &'a [Message],
    ) -> BoxFuture<'a, Intent> {
        // Collect owned futures so we can iterate through classifiers
        Box::pin(async move {
            for classifier in &self.classifiers {
                let intent = classifier.classify(user_input, context).await;
                if !matches!(intent, Intent::Fallback) {
                    return intent;
                }
            }
            Intent::Fallback
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_classifier_empty_returns_fallback() {
        let classifier = KeywordClassifier::default();
        // No keywords registered → everything is Fallback
        assert!(matches!(
            classifier.classify_inner("Hello"),
            Intent::Fallback
        ));
        assert!(matches!(
            classifier.classify_inner("帮我写代码"),
            Intent::Fallback
        ));
    }

    #[test]
    fn test_keyword_classifier_skill_match() {
        let mut classifier = KeywordClassifier::default();
        classifier.add_skill_keywords("coding", &["写代码", "编程", "debug"]);
        let intent = classifier.classify_inner("帮我写代码实现一个排序算法");
        assert!(matches!(intent, Intent::SkillRequired { .. }));
        assert_eq!(intent.skill_name(), Some("coding"));
    }

    #[test]
    fn test_keyword_classifier_direct_match() {
        let mut classifier = KeywordClassifier::default();
        classifier.add_direct_keywords(&["你好", "hello", "thanks"]);
        let intent = classifier.classify_inner("Hello, how are you?");
        assert!(matches!(intent, Intent::DirectAnswer { .. }));
    }

    #[test]
    fn test_keyword_classifier_direct_disabled() {
        let mut classifier = KeywordClassifier::default();
        classifier.add_direct_keywords(&["hello"]);
        classifier.set_enable_direct_answer(false);
        // DirectAnswer disabled → falls through to Fallback
        assert!(matches!(
            classifier.classify_inner("hello"),
            Intent::Fallback
        ));
    }

    #[test]
    fn test_keyword_classifier_case_insensitive() {
        let mut classifier = KeywordClassifier::default();
        classifier.add_skill_keywords("coding", &["Debug"]);
        // Input is lowercase, trigger was registered as "Debug" → normalized to "debug"
        let intent = classifier.classify_inner("please DEBUG this");
        assert!(matches!(intent, Intent::SkillRequired { .. }));
    }

    #[test]
    fn test_llm_classifier_parse_direct() {
        let response = r#"{"intent": "DIRECT", "skill": null, "confidence": 0.95}"#;
        let intent = LlmIntentClassifier::parse_response(response);
        assert!(matches!(
            intent,
            Intent::DirectAnswer {
                confidence: c
            } if (c - 0.95).abs() < 0.01
        ));
    }

    #[test]
    fn test_llm_classifier_parse_skill() {
        let response = r#"{"intent": "SKILL", "skill": "coding", "confidence": 0.88}"#;
        let intent = LlmIntentClassifier::parse_response(response);
        assert_eq!(intent.skill_name(), Some("coding"));
    }

    #[test]
    fn test_llm_classifier_parse_fallback() {
        let response = r#"{"intent": "FALLBACK", "skill": null, "confidence": 0.6}"#;
        let intent = LlmIntentClassifier::parse_response(response);
        assert!(matches!(intent, Intent::Fallback));
    }

    #[test]
    fn test_llm_classifier_parse_invalid_json() {
        let response = "I think this is a coding task";
        let intent = LlmIntentClassifier::parse_response(response);
        assert!(matches!(intent, Intent::Fallback));
    }

    #[test]
    fn test_llm_classifier_parse_markdown_wrapped() {
        let response = r#"```json
{"intent": "SKILL", "skill": "paper-search", "confidence": 0.85}
```"#;
        let intent = LlmIntentClassifier::parse_response(response);
        assert_eq!(intent.skill_name(), Some("paper-search"));
    }

    #[test]
    fn test_llm_classifier_build_prompt_contains_skills() {
        let classifier = LlmIntentClassifier::new(
            // Use a dummy — we only test prompt building, not actual LLM calls
            Arc::new(crate::testing::MockLlmClient::new()),
            vec![
                SkillDescription {
                    name: "coding".into(),
                    description: "Code generation and debugging".into(),
                    example_triggers: vec!["write a function".into()],
                },
                SkillDescription {
                    name: "paper-search".into(),
                    description: "Academic literature search".into(),
                    example_triggers: vec!["find papers about".into()],
                },
            ],
        );
        let prompt = classifier.build_prompt("帮我写个排序算法");
        assert!(prompt.contains("coding"));
        assert!(prompt.contains("paper-search"));
        assert!(prompt.contains("帮我写个排序算法"));
    }

    #[test]
    fn test_config_builder() {
        let config = KeywordClassifierConfig {
            skill_keyword_map: vec![("coding".into(), vec!["debug".into()])],
            direct_keywords: vec!["thanks".into()],
            enable_direct_answer: true,
        };
        let classifier = config.build();

        let intent = classifier.classify_inner("debug this code");
        assert!(matches!(intent, Intent::SkillRequired { .. }));

        let intent = classifier.classify_inner("thanks for your help");
        assert!(matches!(intent, Intent::DirectAnswer { .. }));
    }

    /// RFC 3.1.4: Integration test for programmatic skill trigger words
    /// through the KeywordClassifier.
    ///
    /// For each skill, at least 2 representative trigger queries are tested.
    /// Negative cases (DirectAnswer, Fallback) are also verified.
    #[test]
    fn test_11_skill_trigger_routing() {
        let mut classifier = KeywordClassifier::default();

        // Programmatic descriptors may still register explicit trigger words;
        // standard SKILL.md files are routed by their descriptions instead.
        classifier.add_skill_keywords(
            "coding",
            &[
                "写代码",
                "编程",
                "调试",
                "重构",
                "bug",
                "报错",
                "代码审查",
                "code review",
                "debug",
                "refactor",
                "stack trace",
                "编译错误",
                "单元测试",
                "写个",
                "写一段",
                "写脚本",
                "写函数",
                "写类",
                "实现",
            ],
        );
        classifier.add_skill_keywords(
            "data-visualization",
            &[
                "图表",
                "可视化",
                "柱状图",
                "折线图",
                "饼图",
                "散点图",
                "chart",
                "plot",
                "visualization",
                "画图",
                "画个图",
            ],
        );
        classifier.add_skill_keywords(
            "data-wrangling",
            &[
                "加载数据",
                "读取csv",
                "读取excel",
                "数据清洗",
                "缺失值处理",
                "eda",
                "探索性分析",
                "数据预处理",
                "data wrangling",
                "data cleaning",
                "dataframe",
                "数据质量",
            ],
        );
        classifier.add_skill_keywords(
            "doc-writing",
            &[
                "写报告",
                "写文档",
                "写文章",
                "撰写",
                "写邮件",
                "写方案",
                "write report",
                "write document",
                "draft",
                "起草",
                "写总结",
            ],
        );
        classifier.add_skill_keywords(
            "evidence-medicine",
            &[
                "医学文献",
                "临床研究",
                "护理研究",
                "临床试验",
                "系统综述",
                "meta分析",
                "pubmed",
                "medical research",
                "clinical trial",
                "systematic review",
                "循证医学",
                "骨科护理",
                "药物治疗证据",
            ],
        );
        classifier.add_skill_keywords(
            "git-workflow",
            &[
                "git",
                "分支",
                "提交",
                "commit",
                "pr",
                "mr",
                "merge",
                "合并",
                "冲突",
                "branch",
                "cherry-pick",
                "rebase",
                "版本控制",
            ],
        );
        classifier.add_skill_keywords(
            "paper-reader",
            &[
                "读论文",
                "分析论文",
                "论文解读",
                "读这篇",
                "paper reading",
                "paper analysis",
                "方法论",
                "实验结果",
            ],
        );
        classifier.add_skill_keywords(
            "paper-search",
            &[
                "论文检索",
                "学术论文",
                "文献检索",
                "arxiv",
                "semantic scholar",
                "引用数",
                "paper search",
                "academic paper",
                "research paper",
                "找论文",
                "文献综述",
            ],
        );
        classifier.add_skill_keywords(
            "statistical-analysis",
            &[
                "假设检验",
                "t检验",
                "卡方检验",
                "回归分析",
                "anova",
                "p值",
                "显著性检验",
                "statistics",
                "hypothesis test",
                "regression",
                "统计建模",
                "置信区间",
                "效应量",
            ],
        );
        classifier.add_skill_keywords(
            "translation",
            &[
                "翻译",
                "translate",
                "翻译一下",
                "翻译成中文",
                "翻译成英文",
                "中英对照",
                "本地化",
                "localization",
                "i18n",
                "多语言",
            ],
        );
        classifier.add_skill_keywords(
            "web-search",
            &[
                "上网查",
                "网上搜索",
                "最新新闻",
                "搜索网页",
                "search web",
                "look up",
                "fact check",
                "事实核查",
                "网络调研",
            ],
        );

        // DirectAnswer keywords
        classifier.add_direct_keywords(&["你好", "hello", "hi", "thanks", "谢谢"]);

        // ── Positive cases: each skill gets at least 2 test queries ──
        let test_cases: Vec<(&str, &str)> = vec![
            // coding
            ("帮我写代码实现排序", "coding"),
            ("这段代码有个 bug", "coding"),
            ("帮我写个 Python 脚本", "coding"),
            ("实现一个单例模式", "coding"),
            // data-visualization
            ("画一个柱状图", "data-visualization"),
            ("生成可视化图表", "data-visualization"),
            // data-wrangling
            ("读取CSV文件并清洗", "data-wrangling"),
            ("缺失值处理怎么做", "data-wrangling"),
            // doc-writing
            ("帮我写报告", "doc-writing"),
            ("撰写技术文档", "doc-writing"),
            // evidence-medicine
            ("搜索 PubMed 文献", "evidence-medicine"),
            ("系统综述怎么做", "evidence-medicine"),
            // git-workflow
            ("git 怎么提交", "git-workflow"),
            ("合并分支冲突了", "git-workflow"),
            // paper-reader
            ("帮我读论文", "paper-reader"),
            ("分析这篇论文的方法论", "paper-reader"),
            // paper-search
            ("找论文 arxiv", "paper-search"),
            ("学术论文检索", "paper-search"),
            // statistical-analysis
            ("做个 t检验", "statistical-analysis"),
            ("回归分析怎么做", "statistical-analysis"),
            // translation
            ("翻译这段话", "translation"),
            ("翻译成英文", "translation"),
            // web-search
            ("上网查一下最新消息", "web-search"),
            ("搜索网页找资料", "web-search"),
        ];

        let mut passed = 0;
        let mut failed = Vec::new();

        for (query, expected_skill) in &test_cases {
            let intent = classifier.classify_inner(query);
            match intent {
                Intent::SkillRequired { ref skill_name, .. } if skill_name == expected_skill => {
                    passed += 1;
                }
                other => {
                    failed.push(format!(
                        "  '{}' → expected '{}', got {:?}",
                        query, expected_skill, other
                    ));
                }
            }
        }

        assert!(
            failed.is_empty(),
            "{}/{} trigger routing tests failed:\n{}",
            failed.len(),
            test_cases.len(),
            failed.join("\n")
        );
        assert_eq!(
            passed,
            test_cases.len(),
            "All {} trigger tests should pass",
            test_cases.len()
        );

        // ── Negative cases ──

        // DirectAnswer: greetings
        let intent = classifier.classify_inner("你好");
        assert!(
            matches!(intent, Intent::DirectAnswer { .. }),
            "Greeting should be DirectAnswer, got {:?}",
            intent
        );

        // Fallback: generic question with no trigger match
        let intent = classifier.classify_inner("今天天气怎么样");
        assert!(
            matches!(intent, Intent::Fallback),
            "Weather query should be Fallback, got {:?}",
            intent
        );

        // Fallback: unrelated question
        let intent = classifier.classify_inner("推荐一本好看的小说");
        assert!(
            matches!(intent, Intent::Fallback),
            "Novel recommendation should be Fallback, got {:?}",
            intent
        );
    }
}
