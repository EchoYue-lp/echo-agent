//! Plan-and-Execute 类型定义

use serde::{Deserialize, Serialize};

/// 执行计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// 计划中的步骤列表
    pub steps: Vec<PlanStep>,
    /// 计划的整体目标描述
    pub goal: Option<String>,
}

impl Plan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps, goal: None }
    }

    pub fn with_goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = Some(goal.into());
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
}

impl PlanStep {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            status: StepStatus::Pending,
            expected_input: None,
            expected_output: None,
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
