//! Plan-and-Execute 类型定义

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 执行计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// 计划唯一 ID（自动生成）
    #[serde(default = "generate_plan_id")]
    pub id: Option<String>,
    /// 可读 slug（如 "swift-fox"）
    #[serde(default)]
    pub slug: Option<String>,
    /// 版本号（每次 replan 递增）
    #[serde(default)]
    pub version: u32,
    /// 计划中的步骤列表
    pub steps: Vec<PlanStep>,
    /// 计划的整体目标描述
    pub goal: Option<String>,
    /// 父计划 ID（用于增量重规划追踪）
    #[serde(default)]
    pub parent_plan_id: Option<String>,
    /// 附加元数据
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// 创建时间戳（秒）
    #[serde(default)]
    pub created_at: u64,
    /// 最后更新时间戳（秒）
    #[serde(default)]
    pub updated_at: u64,
}

fn generate_plan_id() -> Option<String> {
    Some(format!("plan_{}", uuid::Uuid::new_v4().as_simple()))
}

fn now_secs() -> u64 {
    crate::utils::time::now_secs()
}

impl Plan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        let now = now_secs();
        Self {
            id: generate_plan_id(),
            slug: None,
            version: 1,
            steps,
            goal: None,
            parent_plan_id: None,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = Some(goal.into());
        self
    }

    /// 设置可读 slug
    pub fn with_slug(mut self, slug: impl Into<String>) -> Self {
        self.slug = Some(slug.into());
        self
    }

    /// 设置父计划 ID（用于增量重规划）
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_plan_id = Some(parent_id.into());
        self
    }

    /// 添加元数据
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// 返回所有已完成步骤的数量
    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count()
    }

    /// 检查计划是否全部完成
    pub fn is_completed(&self) -> bool {
        self.steps.iter().all(|s| s.status == StepStatus::Completed)
    }

    /// 触碰更新时间
    pub fn touch(&mut self) {
        self.updated_at = now_secs();
    }

    /// 将 Plan 转换为 Task DAG，用于统一执行模型
    ///
    /// 每个 PlanStep 映射为一个 Task，`step_N` 格式的依赖转换为任务 ID 引用。
    /// 转换后可直接交给 `TaskExecutor` 并行执行。
    pub fn to_task_dag(&self) -> Vec<crate::tasks::Task> {
        let now = now_secs();
        self.steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                // 将 "step_N" 格式的依赖转换为 "plan_step_N" 任务 ID
                let deps: Vec<String> = step
                    .dependencies
                    .iter()
                    .filter_map(|dep| {
                        if let Some(idx_str) = dep.strip_prefix("step_") {
                            if let Ok(idx) = idx_str.parse::<usize>() {
                                return Some(format!("plan_step_{}", idx));
                            }
                        }
                        // 描述型依赖：尝试匹配步骤索引
                        self.steps
                            .iter()
                            .enumerate()
                            .find(|(_, s)| {
                                s.description.contains(dep.as_str()) || dep.contains(&s.description)
                            })
                            .map(|(matched_idx, _)| format!("plan_step_{}", matched_idx))
                    })
                    .collect();

                crate::tasks::Task {
                    id: format!("plan_step_{}", i),
                    description: step.description.clone(),
                    subject: step.description.clone(),
                    status: crate::tasks::TaskStatus::Pending,
                    dependencies: deps,
                    priority: 5,
                    result: None,
                    reasoning: step.expected_output.clone(),
                    assigned_agent: None,
                    tags: vec![],
                    parent_id: self.id.clone(),
                    created_at: now,
                    updated_at: now,
                    timeout_secs: 0,
                    max_retries: 0,
                    retry_count: 0,
                }
            })
            .collect()
    }

    // ── 验证与自动修复 ──────────────────────────────────────────────────

    /// 验证计划的有效性，返回所有发现的问题
    pub fn validate(&self) -> Vec<PlanValidationIssue> {
        let mut issues = Vec::new();

        // 1. 至少有一个步骤
        if self.steps.is_empty() {
            issues.push(PlanValidationIssue {
                severity: IssueSeverity::Error,
                message: "Plan has no steps".to_string(),
                fix: Some("Add at least one step".to_string()),
            });
        }

        // 2. 检查步骤间的循环依赖
        let desc_set: std::collections::HashSet<&str> =
            self.steps.iter().map(|s| s.description.as_str()).collect();

        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.dependencies {
                // 自环检测
                if dep == &format!("step_{}", i) {
                    issues.push(PlanValidationIssue {
                        severity: IssueSeverity::Error,
                        message: format!("Step {} depends on itself", i),
                        fix: Some("Remove self-dependency".to_string()),
                    });
                }
            }
        }

        // 3. 检查缺失的依赖引用
        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.dependencies {
                if !dep.starts_with("step_") {
                    // 可能是描述引用，尝试匹配
                    let matched = self.steps.iter().any(|s| {
                        s.description.contains(dep.as_str()) || dep.contains(&s.description)
                    });
                    if !matched && !desc_set.contains(dep.as_str()) {
                        issues.push(PlanValidationIssue {
                            severity: IssueSeverity::Warning,
                            message: format!("Step {} has unresolvable dependency: {}", i, dep),
                            fix: Some("Remove or fix the dependency reference".to_string()),
                        });
                    }
                } else if let Ok(idx) = dep.trim_start_matches("step_").parse::<usize>() {
                    if idx >= self.steps.len() {
                        issues.push(PlanValidationIssue {
                            severity: IssueSeverity::Error,
                            message: format!(
                                "Step {} depends on non-existent step index {}",
                                i, idx
                            ),
                            fix: Some("Fix the dependency index".to_string()),
                        });
                    }
                }
            }
        }

        // 4. 检查步骤描述是否为空
        for (i, step) in self.steps.iter().enumerate() {
            if step.description.trim().is_empty() {
                issues.push(PlanValidationIssue {
                    severity: IssueSeverity::Warning,
                    message: format!("Step {} has empty description", i),
                    fix: Some("Provide a meaningful description".to_string()),
                });
            }
        }

        issues
    }

    /// 自动修复可修复的问题
    ///
    /// 修复项：
    /// - 移除自环依赖
    /// - 移除指向不存在步骤的依赖
    /// - 为空描述生成占位描述
    pub fn auto_fix(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        let steps_len = self.steps.len();

        for (i, step) in self.steps.iter_mut().enumerate() {
            // 移除自环
            let self_dep = format!("step_{}", i);
            let before = step.dependencies.len();
            step.dependencies.retain(|d| d != &self_dep);
            if step.dependencies.len() < before {
                fixes.push(format!("Removed self-dependency from step {}", i));
            }

            // 移除无效索引引用
            step.dependencies.retain(|d| {
                if let Some(idx_str) = d.strip_prefix("step_") {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        return idx < steps_len;
                    }
                }
                true // 保留非索引引用（描述匹配型）
            });

            // 修复空描述
            if step.description.trim().is_empty() {
                step.description = format!("Unnamed step {}", i);
                fixes.push(format!("Filled empty description for step {}", i));
            }
        }

        if !fixes.is_empty() {
            self.touch();
        }

        fixes
    }

    /// 获取指定步骤的所有下游步骤（依赖于此步骤的步骤）
    pub fn downstream_steps(&self, step_idx: usize) -> Vec<usize> {
        let step_id = format!("step_{}", step_idx);
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.dependencies.contains(&step_id))
            .map(|(i, _)| i)
            .collect()
    }

    /// 递归获取所有下游步骤（包括间接依赖）
    pub fn downstream_steps_recursive(&self, step_idx: usize) -> Vec<usize> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.collect_downstream(step_idx, &mut result, &mut visited);
        result
    }

    fn collect_downstream(
        &self,
        step_idx: usize,
        result: &mut Vec<usize>,
        visited: &mut std::collections::HashSet<usize>,
    ) {
        if visited.contains(&step_idx) {
            return;
        }
        visited.insert(step_idx);
        for downstream in self.downstream_steps(step_idx) {
            if !result.contains(&downstream) {
                result.push(downstream);
            }
            self.collect_downstream(downstream, result, visited);
        }
    }
}

/// 计划验证问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanValidationIssue {
    /// 严重程度
    pub severity: IssueSeverity,
    /// 问题描述
    pub message: String,
    /// 建议修复方式
    #[serde(default)]
    pub fix: Option<String>,
}

/// 问题严重程度
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueSeverity {
    /// 错误：计划无法执行
    Error,
    /// 警告：计划可执行但可能有问题
    Warning,
}

/// 计划中的单个步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// 步骤描述
    pub description: String,
    /// 步骤状态
    pub status: StepStatus,
    /// 预期输入（对上一步结果的依赖描述）
    pub expected_input: Option<String>,
    /// 预期输出描述
    pub expected_output: Option<String>,
    /// 依赖的步骤索引列表（从 LLM 规划结果生成）
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl PlanStep {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            status: StepStatus::Pending,
            expected_input: None,
            expected_output: None,
            dependencies: Vec::new(),
        }
    }

    pub fn with_expected_input(mut self, input: impl Into<String>) -> Self {
        self.expected_input = Some(input.into());
        self
    }

    pub fn with_expected_output(mut self, output: impl Into<String>) -> Self {
        self.expected_output = Some(output.into());
        self
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }
}

// ── 结构化输出类型（LLM 响应解析用） ──────────────────────────────────────────

/// LLM 返回的结构化计划输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    /// 步骤列表
    pub steps: Vec<PlanStepOutput>,
}

/// LLM 返回的单个步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepOutput {
    /// 步骤描述
    pub description: String,
    /// 依赖的步骤描述（模糊匹配后转换为索引）
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// 预期输出
    #[serde(default)]
    pub expected_output: Option<String>,
}

/// 返回 JSON Schema 用于 LLM 结构化输出
pub fn plan_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {
                            "type": "string",
                            "description": "步骤的详细描述"
                        },
                        "dependencies": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "依赖的步骤描述关键词（可为空）"
                        },
                        "expected_output": {
                            "type": "string",
                            "description": "步骤的预期输出"
                        }
                    },
                    "required": ["description"]
                },
                "minItems": 1
            }
        },
        "required": ["steps"]
    })
}

/// 步骤执行状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// 等待执行
    Pending,
    /// 正在执行
    Running,
    /// 已完成
    Completed,
    /// 执行失败
    Failed,
}

/// 单个步骤的执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// 步骤在计划中的索引
    pub step_index: usize,
    /// 步骤描述
    pub description: String,
    /// 执行输出
    pub output: String,
    /// 是否成功
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_step_status() {
        let step = PlanStep::new("test step");
        assert_eq!(step.status, StepStatus::Pending);
        assert_eq!(step.description, "test step");
    }

    #[test]
    fn test_plan() {
        let plan = Plan::new(vec![
            PlanStep::new("step 1"),
            PlanStep::new("step 2"),
            PlanStep::new("step 3"),
        ]);
        assert_eq!(plan.steps.len(), 3);
        assert!(plan.id.is_some());
        assert_eq!(plan.version, 1);
    }

    #[test]
    fn test_plan_auto_id() {
        let plan = Plan::new(vec![PlanStep::new("test")]);
        assert!(plan.id.as_ref().unwrap().starts_with("plan_"));
    }

    #[test]
    fn test_plan_with_metadata() {
        let plan =
            Plan::new(vec![PlanStep::new("test")]).with_metadata("key", serde_json::json!("value"));
        assert_eq!(plan.metadata.get("key").unwrap(), "value");
    }

    #[test]
    fn test_validate_empty_plan() {
        let plan = Plan::new(vec![]);
        let issues = plan.validate();
        assert!(issues.iter().any(|i| i.message.contains("no steps")));
    }

    #[test]
    fn test_validate_self_dependency() {
        let plan = Plan::new(vec![
            PlanStep::new("step 0"),
            PlanStep::new("step 1").with_dependencies(vec!["step_1".to_string()]),
        ]);
        let issues = plan.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("depends on itself"))
        );
    }

    #[test]
    fn test_validate_invalid_index() {
        let plan = Plan::new(vec![
            PlanStep::new("step 0").with_dependencies(vec!["step_99".to_string()]),
        ]);
        let issues = plan.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("non-existent step index"))
        );
    }

    #[test]
    fn test_auto_fix_removes_self_dependency() {
        let mut plan = Plan::new(vec![
            PlanStep::new("step 0"),
            PlanStep::new("step 1")
                .with_dependencies(vec!["step_1".to_string(), "step_0".to_string()]),
        ]);
        let fixes = plan.auto_fix();
        assert!(fixes.iter().any(|f| f.contains("self-dependency")));
        assert_eq!(plan.steps[1].dependencies, vec!["step_0"]);
    }

    #[test]
    fn test_auto_fix_empty_description() {
        let mut plan = Plan::new(vec![PlanStep::new("")]);
        let fixes = plan.auto_fix();
        assert!(fixes.iter().any(|f| f.contains("empty description")));
        assert!(!plan.steps[0].description.is_empty());
    }

    #[test]
    fn test_downstream_steps() {
        let plan = Plan::new(vec![
            PlanStep::new("A"),
            PlanStep::new("B").with_dependencies(vec!["step_0".to_string()]),
            PlanStep::new("C").with_dependencies(vec!["step_0".to_string()]),
            PlanStep::new("D").with_dependencies(vec!["step_1".to_string()]),
        ]);
        let downstream = plan.downstream_steps(0);
        assert_eq!(downstream, vec![1, 2]);

        let recursive = plan.downstream_steps_recursive(0);
        assert!(recursive.contains(&1));
        assert!(recursive.contains(&2));
        assert!(recursive.contains(&3)); // 3 depends on 1 which depends on 0
    }

    #[test]
    fn test_plan_touch() {
        let mut plan = Plan::new(vec![PlanStep::new("test")]);
        let before = plan.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        plan.touch();
        assert!(plan.updated_at >= before);
    }
}
