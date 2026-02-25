use std::collections::HashSet;

/// 工具执行前的人工审批管理器（guard 模式）
///
/// 通过 `ReactAgent::add_need_appeal_tool` 将工具标记为"危险"，
/// 执行前会在控制台弹出 y/n 确认。
pub struct HumanApprovalManager {
    need_approval_tools: HashSet<String>,
}

impl HumanApprovalManager {
    pub fn new() -> Self {
        HumanApprovalManager {
            need_approval_tools: HashSet::new(),
        }
    }

    pub fn mark_need_approval(&mut self, tool_name: String) {
        self.need_approval_tools.insert(tool_name);
    }

    pub fn needs_approval(&self, tool_name: &str) -> bool {
        self.need_approval_tools.contains(tool_name)
    }
}
