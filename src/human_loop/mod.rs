use std::collections::HashSet;
use crate::tools::Tool;

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalResult {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    /// 引起触发的原因类型：LLM 触发，tool 触发
    pub approval_type: String,
    /// 人工批准触发原因，即为什么会触发 human_in_loop ？
    pub reasoning: String,
    /// 引起触发的工具名称
    pub tool: String,
    /// 人工批准结果
    pub approval_result: ApprovalResult,
}

/// 人工确认接口
pub trait HumanApproval {
    /// 请求确认操作
    fn request_approval(&self, action: &str, details: &str) -> ApprovalResult;
}

/// 人工确认管理，两种情况下需要人工介入：
///     1、危险操作，例如：删除文件、修改文件
///     2、不确定的操作，例如：用户问天气怎么样，但没有确定哪个城市的天气？用户问价格，但没指定物品。

pub struct HumanApprovalManager {
    need_approval_tools: HashSet<String>,
}

impl HumanApprovalManager {
    pub fn new() -> Self {
        HumanApprovalManager {
            need_approval_tools: HashSet::new(),
        }
    }

    /// 标记某个工具为危险工具
    pub fn mark_need_approval(&mut self, tool_name: String) {
        self.need_approval_tools.insert(tool_name);
    }

    /// 检查工具是否需要人工审批
    pub fn needs_approval(&self, tool_name: &str) -> bool {
        self.need_approval_tools.contains(tool_name)
    }
}
