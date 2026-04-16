//! Self-Reflection 类型定义

use serde::{Deserialize, Serialize};

/// 评估结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Critique {
    /// 质量评分（0.0 - 10.0）
    pub score: f64,
    /// 是否通过质量阈值
    pub passed: bool,
    /// 详细反馈
    pub feedback: String,
    /// 改进建议
    #[serde(default)]
    pub suggestions: Vec<String>,
}

/// 评估结果结构化输出（LLM JSON 解析用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueOutput {
    pub score: f64,
    pub passed: bool,
    pub feedback: String,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

impl From<CritiqueOutput> for Critique {
    fn from(output: CritiqueOutput) -> Self {
        Self {
            score: output.score,
            passed: output.passed,
            feedback: output.feedback,
            suggestions: output.suggestions,
        }
    }
}

/// 单次反思迭代记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionRecord {
    /// 迭代序号（0-based）
    pub iteration: usize,
    /// 当前回答
    pub answer: String,
    /// 评估结果
    pub critique: Critique,
    /// 反思文本（分析失败原因）
    pub reflection_text: String,
    /// 修正后的回答
    pub refined_answer: Option<String>,
}

/// 反思经验（用于情景记忆）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionExperience {
    /// 唯一 ID
    pub id: String,
    /// 经验教训（如 "确认数据可用性后再查询"）
    pub lesson: String,
    /// 错误模式（如 "假设数据存在但不检查"）
    pub error_pattern: String,
    /// 任务类别
    #[serde(default)]
    pub task_category: Option<String>,
    /// 被引用次数
    #[serde(default)]
    pub use_count: u32,
}

impl ReflectionExperience {
    pub fn new(lesson: impl Into<String>, error_pattern: impl Into<String>) -> Self {
        Self {
            id: format!("exp_{}", uuid::Uuid::new_v4().as_simple()),
            lesson: lesson.into(),
            error_pattern: error_pattern.into(),
            task_category: None,
            use_count: 0,
        }
    }

    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.task_category = Some(category.into());
        self
    }
}

/// 修正提示词构建器 trait
pub trait RefinementPromptBuilder: Send + Sync {
    /// 构建修正提示词
    ///
    /// - `task`: 原始任务
    /// - `current_answer`: 当前回答
    /// - `critique`: 评估结果
    /// - `reflection`: 反思文本
    /// - `iteration`: 当前迭代次数
    fn build_prompt(
        &self,
        task: &str,
        current_answer: &str,
        critique: &Critique,
        reflection: &str,
        iteration: usize,
    ) -> String;
}

/// 默认修正提示词构建器
pub struct DefaultRefinementPromptBuilder;

impl RefinementPromptBuilder for DefaultRefinementPromptBuilder {
    fn build_prompt(
        &self,
        task: &str,
        current_answer: &str,
        critique: &Critique,
        reflection: &str,
        iteration: usize,
    ) -> String {
        let suggestions_text = if critique.suggestions.is_empty() {
            String::new()
        } else {
            format!(
                "\n改进建议:\n{}",
                critique
                    .suggestions
                    .iter()
                    .map(|s| format!("- {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        format!(
            "原始任务: {}\n\n\
             你的上一版回答:\n{}\n\n\
             评估反馈（评分: {:.1}/10.0）:\n{}{}\n\n\
             反思分析:\n{}\n\n\
             这是第 {} 次改进。请根据以上评估反馈和反思分析，提供更准确、更完整的回答。",
            task,
            current_answer,
            critique.score,
            critique.feedback,
            suggestions_text,
            reflection,
            iteration + 1,
        )
    }
}

/// 反思提示词构建器（生成反思文本）
pub trait ReflectionPromptBuilder: Send + Sync {
    /// 构建反思提示词
    fn build_reflection_prompt(&self, task: &str, answer: &str, critique: &Critique) -> String;
}

/// 默认反思提示词构建器
pub struct DefaultReflectionPromptBuilder;

impl ReflectionPromptBuilder for DefaultReflectionPromptBuilder {
    fn build_reflection_prompt(&self, task: &str, answer: &str, critique: &Critique) -> String {
        let errors_text = if critique.suggestions.is_empty() {
            String::new()
        } else {
            format!(
                "\n具体问题:\n{}",
                critique
                    .suggestions
                    .iter()
                    .map(|s| format!("- {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        format!(
            "任务: {}\n\n\
             生成的回答:\n{}\n\n\
             评估结果: 评分 {:.1}/10.0，未通过。\n\
             评估反馈: {}{}\n\n\
             请深入分析上述回答中存在的问题，思考：\n\
             1. 为什么会产生这些错误或不足？\n\
             2. 根本原因是什么？\n\
             3. 下次应该如何避免类似问题？\n\n\
             请输出简洁的反思文本。",
            task, answer, critique.score, critique.feedback, errors_text,
        )
    }
}

/// 返回 Critique 结构的 JSON Schema（用于 LLM 结构化输出）
pub fn critique_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "score": {
                "type": "number",
                "description": "质量评分（0.0 到 10.0，10.0 为最高）"
            },
            "passed": {
                "type": "boolean",
                "description": "是否通过质量标准"
            },
            "feedback": {
                "type": "string",
                "description": "详细的评估反馈，说明优缺点"
            },
            "suggestions": {
                "type": "array",
                "items": { "type": "string" },
                "description": "具体的改进建议列表"
            }
        },
        "required": ["score", "passed", "feedback"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critique_output_parse() {
        let json = r#"{"score": 8.5, "passed": true, "feedback": "回答准确", "suggestions": ["可以更详细"]}"#;
        let output: CritiqueOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.score, 8.5);
        assert!(output.passed);
        assert_eq!(output.suggestions.len(), 1);
    }

    #[test]
    fn test_critique_output_default_suggestions() {
        let json = r#"{"score": 6.0, "passed": false, "feedback": "不完整"}"#;
        let output: CritiqueOutput = serde_json::from_str(json).unwrap();
        assert!(output.suggestions.is_empty());
    }

    #[test]
    fn test_refinement_prompt_builder() {
        let builder = DefaultRefinementPromptBuilder;
        let critique = Critique {
            score: 5.0,
            passed: false,
            feedback: "不够准确".to_string(),
            suggestions: vec!["增加示例".to_string()],
        };
        let prompt = builder.build_prompt("解释 Rust", "Rust 是...", &critique, "需要更详细", 0);
        assert!(prompt.contains("解释 Rust"));
        assert!(prompt.contains("不够准确"));
        assert!(prompt.contains("增加示例"));
        assert!(prompt.contains("第 1 次改进"));
    }

    #[test]
    fn test_reflection_prompt_builder() {
        let builder = DefaultReflectionPromptBuilder;
        let critique = Critique {
            score: 4.0,
            passed: false,
            feedback: "概念有误".to_string(),
            suggestions: vec!["修正定义".to_string()],
        };
        let prompt = builder.build_reflection_prompt("解释所有权", "Rust 有 GC...", &critique);
        assert!(prompt.contains("解释所有权"));
        assert!(prompt.contains("概念有误"));
        assert!(prompt.contains("修正定义"));
    }

    #[test]
    fn test_reflection_experience() {
        let exp =
            ReflectionExperience::new("确认数据后再查询", "假设数据存在").with_category("database");
        assert!(exp.id.starts_with("exp_"));
        assert_eq!(exp.lesson, "确认数据后再查询");
        assert_eq!(exp.task_category, Some("database".to_string()));
    }

    #[test]
    fn test_critique_schema() {
        let schema = critique_output_schema();
        assert!(schema.is_object());
        assert!(schema["properties"]["score"].is_object());
        assert!(schema["properties"]["passed"].is_object());
    }
}
