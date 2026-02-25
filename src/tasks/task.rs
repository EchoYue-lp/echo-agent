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
    /// 任务理由理由
    pub reasoning: Option<String>,
    /// 父任务ID
    pub parent_id: Option<String>,
    /// 创建时间戳
    pub created_at: u64,
    /// 更新时间戳
    pub updated_at: u64,
}

impl Task {
    /// 创建新任务
    pub fn new(id: String, description: String) -> Self {
        Self {
            id,
            description,
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
            priority: 5,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// 设置依赖
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// 添加依赖
    pub fn add_dependency(&mut self, dep: String) {
        self.dependencies.push(dep);
    }

    /// 设置优先级
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(10);
        self
    }
}
