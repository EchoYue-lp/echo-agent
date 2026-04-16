//! 任务定义

use serde::{Deserialize, Serialize};

/// 任务状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// 待处理
    Pending,
    /// 进行中
    InProgress,
    /// 已完成
    Completed,
    /// 已取消
    Cancelled,
    /// 失败
    Failed(String),
    /// 阻塞
    Blocked(String),
    /// 超时
    TimedOut { error: String },
    /// 重试中
    Retrying { attempt: u32, last_error: String },
}

impl TaskStatus {
    /// 是否为终态（不会再变化）
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed
                | TaskStatus::Cancelled
                | TaskStatus::Failed(_)
                | TaskStatus::TimedOut { .. }
        )
    }

    /// 状态转换是否合法
    pub fn can_transition_to(&self, target: &TaskStatus) -> bool {
        match self {
            TaskStatus::Pending => {
                matches!(target, TaskStatus::InProgress | TaskStatus::Cancelled)
            }
            TaskStatus::InProgress => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
            ),
            TaskStatus::Retrying { .. } => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
            ),
            TaskStatus::Blocked(_) => matches!(target, TaskStatus::Pending | TaskStatus::Cancelled),
            _ => false,
        }
    }

    /// 执行状态转换，校验合法性后返回新状态
    ///
    /// 如果转换不合法，返回包含详细错误信息的 `Err`。
    pub fn transition_to(&self, target: TaskStatus) -> Result<TaskStatus, String> {
        if !self.can_transition_to(&target) {
            return Err(format!(
                "Invalid task state transition: {:?} → {:?}",
                self, target
            ));
        }
        Ok(target)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// 任务 ID
    pub id: String,
    /// 任务描述
    pub description: String,
    /// 任务状态
    pub status: TaskStatus,
    /// 依赖的任务 ID 列表
    pub dependencies: Vec<String>,
    /// 优先级 (0-10, 10 最高)
    pub priority: u8,
    /// 任务结果
    pub result: Option<String>,
    /// 执行理由或备注
    pub reasoning: Option<String>,
    /// 分配给执行的 Agent 名称
    pub assigned_agent: Option<String>,
    /// 标签（用于分类和过滤）
    pub tags: Vec<String>,
    pub parent_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// 任务主题/标题（用于日志和事件）
    pub subject: String,
    /// 超时时间（秒），0 表示不超时
    pub timeout_secs: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 当前已重试次数
    pub retry_count: u32,
}

impl Task {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        let description = description.into();
        Self {
            id: id.into(),
            subject: description.clone(),
            description,
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
            priority: 5,
            result: None,
            reasoning: None,
            assigned_agent: None,
            tags: Vec::new(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            timeout_secs: 0,
            max_retries: 0,
            retry_count: 0,
        }
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn add_dependency(&mut self, dep: String) {
        self.dependencies.push(dep);
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(10);
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// 指定执行的 Agent
    pub fn with_assigned_agent(mut self, agent: impl Into<String>) -> Self {
        self.assigned_agent = Some(agent.into());
        self
    }

    /// 添加标签
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// 添加单个标签
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        self.tags.push(tag.into());
    }

    /// 是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.status == TaskStatus::Cancelled
    }

    /// 取消任务（使用状态机校验）
    ///
    /// 仅当当前状态允许转换为 `Cancelled` 时才成功。
    /// 返回 `true` 表示取消成功，`false` 表示当前状态不允许取消。
    pub fn cancel(&mut self) -> bool {
        match self.status.transition_to(TaskStatus::Cancelled) {
            Ok(new_status) => {
                self.status = new_status;
                true
            }
            Err(_) => false,
        }
    }

    /// 记录一次执行结果
    pub fn record_execution(
        &mut self,
        attempt: u32,
        error: Option<String>,
        duration_secs: Option<u64>,
        result: Option<String>,
    ) {
        self.retry_count = attempt.saturating_sub(1);
        self.updated_at = super::time::now_secs();
        if let Some(r) = result {
            self.result = Some(r);
        }
        if let Some(dur) = duration_secs {
            let _ = dur; // 记录执行时长（可用于未来统计）
        }
        if let Some(err) = error {
            self.reasoning = Some(format!("Attempt {} failed: {}", attempt, err));
        }
    }
}
