//! Mock 工具，用于在不依赖外部服务的情况下测试 Agent 的工具调用行为。
//!
//! # 典型用途
//! - 测试工具参数解析逻辑
//! - 在集成测试中替换真实工具（数据库、HTTP 等）
//! - 测试工具执行失败时 Agent 的容错行为
//!
//! # 示例
//!
//! ```rust
//! use echo_agent::testing::MockTool;
//! use echo_agent::tools::Tool;
//! use std::collections::HashMap;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let tool = MockTool::new("calculator")
//!     .with_description("计算两数之和")
//!     .with_response("结果是 42");
//!
//! let params = HashMap::new();
//! let result = tool.execute(params).await.unwrap();
//! assert!(result.success);
//! assert_eq!(result.output, "结果是 42");
//! assert_eq!(tool.call_count(), 1);
//! # }
//! ```

use crate::error::Result;
use crate::tools::{Tool, ToolParameters, ToolResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// 预设执行结果枚举
enum MockToolResponse {
    Success(String),
    Failure(String),
}

/// 可脚本化的 Mock Tool。
///
/// 按顺序返回预设的执行结果；队列耗尽后返回最后一个响应（若有），
/// 否则返回默认成功响应 `"mock response"`。
pub struct MockTool {
    name: String,
    description: String,
    parameters: Value,
    responses: Arc<Mutex<VecDeque<MockToolResponse>>>,
    /// 每次调用时收到的参数，按顺序记录
    calls: Arc<Mutex<Vec<HashMap<String, Value>>>>,
}

impl MockTool {
    /// 创建具名 Mock Tool（描述和参数 schema 均使用默认值）
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: "A mock tool for testing".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::<HashMap<String, Value>>::new())),
        }
    }

    /// 设置工具描述
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// 设置参数 JSON Schema
    pub fn with_parameters(mut self, schema: Value) -> Self {
        self.parameters = schema;
        self
    }

    /// 追加一条成功响应文本
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(MockToolResponse::Success(text.into()));
        self
    }

    /// 批量追加多条成功响应
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap();
            for t in texts {
                q.push_back(MockToolResponse::Success(t.into()));
            }
        }
        self
    }

    /// 追加一条失败响应（用于测试工具失败时 Agent 的行为）
    pub fn with_failure(self, msg: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(MockToolResponse::Failure(msg.into()));
        self
    }

    /// 已执行的调用总次数
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// 最后一次调用时传入的参数（若从未调用则返回 `None`）
    pub fn last_args(&self) -> Option<HashMap<String, Value>> {
        self.calls.lock().unwrap().last().cloned()
    }

    /// 所有历史调用的参数（按时序排列）
    pub fn all_calls(&self) -> Vec<HashMap<String, Value>> {
        self.calls.lock().unwrap().clone()
    }

    /// 清空已记录的调用历史
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap().clear();
    }
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
        // 记录本次调用参数
        self.calls.lock().unwrap().push(params.clone());

        let response = self.responses.lock().unwrap().pop_front();
        match response {
            Some(MockToolResponse::Success(text)) => Ok(ToolResult::success(text)),
            Some(MockToolResponse::Failure(msg)) => Ok(ToolResult::error(msg)),
            // 队列耗尽时返回默认成功
            None => Ok(ToolResult::success("mock response".to_string())),
        }
    }
}
