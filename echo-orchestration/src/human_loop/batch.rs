//! 批量审批支持
//!
//! 当多个工具同时需要审批时，将它们打包成批量请求，
//! 让用户一次性处理，避免逐个审批的交互疲劳。
//!
//! `BatchApprovalProvider` 的默认 `batch_request` 实现会逐个调用 `request()`。
//! 自定义 Provider 可以覆写此方法，提供更高效的批量交互体验。

use std::time::Duration;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse, RiskLevel};
use echo_core::error::Result;

// ── 批量审批请求 ─────────────────────────────────────────────────────────────

/// 批量审批请求项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchApprovalItem {
    /// 工具名称
    pub tool_name: String,
    /// 工具参数
    pub args: Value,
    /// 风险等级
    pub risk_level: RiskLevel,
    /// 给用户的提示
    pub prompt: String,
}

impl BatchApprovalItem {
    /// 创建新的批量审批项
    pub fn new(tool_name: impl Into<String>, args: Value, risk_level: RiskLevel) -> Self {
        let tool_name = tool_name.into();
        let prompt = format!("工具 [{}] 需要人工审批（{}风险）", tool_name, risk_level);
        Self {
            tool_name,
            args,
            risk_level,
            prompt,
        }
    }
}

/// 批量审批请求
#[derive(Debug, Clone)]
pub struct BatchApprovalRequest {
    /// 待审批的工具列表
    pub items: Vec<BatchApprovalItem>,
    /// 给用户的全局提示
    pub prompt: String,
    /// 超时设置
    pub timeout: Option<Duration>,
}

impl BatchApprovalRequest {
    /// 创建新的批量审批请求
    pub fn new(items: Vec<BatchApprovalItem>) -> Self {
        let prompt = format!(
            "共 {} 个工具调用需要审批：\n{}",
            items.len(),
            items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    format!("  {}. {} [{:?}]", i + 1, item.tool_name, item.risk_level)
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
        Self {
            items,
            prompt,
            timeout: None,
        }
    }

    /// 设置超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

// ── 批量审批响应 ─────────────────────────────────────────────────────────────

/// 单个工具的审批决策
#[derive(Debug, Clone)]
pub enum BatchItemDecision {
    /// 批准
    Approved,
    /// 拒绝
    Rejected { reason: Option<String> },
    /// 跳过（推迟）
    Skipped,
}

/// 批量审批响应
#[derive(Debug, Clone)]
pub enum BatchApprovalResponse {
    /// 全部批准
    AllApproved,
    /// 全部拒绝
    AllRejected { reason: Option<String> },
    /// 逐项决策（索引与请求 items 对应）
    Individual(Vec<BatchItemDecision>),
    /// 超时
    Timeout,
}

// ── 批量审批 trait ────────────────────────────────────────────────────────────

/// 批量审批扩展 trait
///
/// 为 [`HumanLoopProvider`] 添加批量审批能力。
/// 默认实现逐个调用 `request()`。
pub trait BatchApprovalProvider: HumanLoopProvider {
    /// 批量审批请求
    ///
    /// 默认实现：逐个调用 `request()`。
    fn batch_request(
        &self,
        batch: BatchApprovalRequest,
    ) -> BoxFuture<'_, Result<BatchApprovalResponse>> {
        Box::pin(async move {
            let mut decisions = Vec::with_capacity(batch.items.len());

            for item in &batch.items {
                let req = HumanLoopRequest {
                    kind: HumanLoopKind::Approval,
                    prompt: item.prompt.clone(),
                    tool_name: Some(item.tool_name.clone()),
                    args: Some(item.args.clone()),
                    risk_level: Some(item.risk_level),
                    timeout: batch.timeout,
                };

                match self.request(req).await? {
                    HumanLoopResponse::Approved => {
                        decisions.push(BatchItemDecision::Approved);
                    }
                    HumanLoopResponse::ApprovedWithScope { scope: _ } => {
                        decisions.push(BatchItemDecision::Approved);
                    }
                    HumanLoopResponse::ModifiedArgs { args: _, scope: _ } => {
                        // 批量审批中暂不支持修改参数，按批准处理
                        decisions.push(BatchItemDecision::Approved);
                    }
                    HumanLoopResponse::Rejected { reason } => {
                        decisions.push(BatchItemDecision::Rejected { reason });
                    }
                    HumanLoopResponse::Timeout => {
                        return Ok(BatchApprovalResponse::Timeout);
                    }
                    HumanLoopResponse::Deferred => {
                        decisions.push(BatchItemDecision::Skipped);
                    }
                    HumanLoopResponse::Text(_) => {
                        decisions.push(BatchItemDecision::Skipped);
                    }
                }
            }

            // 检查是否全部一致
            let all_approved = decisions
                .iter()
                .all(|d| matches!(d, BatchItemDecision::Approved));
            let all_rejected = decisions
                .iter()
                .all(|d| matches!(d, BatchItemDecision::Rejected { .. }));

            if all_approved {
                Ok(BatchApprovalResponse::AllApproved)
            } else if all_rejected {
                Ok(BatchApprovalResponse::AllRejected { reason: None })
            } else {
                Ok(BatchApprovalResponse::Individual(decisions))
            }
        })
    }
}

/// 为所有实现了 `HumanLoopProvider` 的类型自动实现 `BatchApprovalProvider`
impl<T: HumanLoopProvider> BatchApprovalProvider for T {}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_batch_approval_item_new() {
        let item = BatchApprovalItem::new("Bash", json!({"cmd": "ls"}), RiskLevel::High);
        assert_eq!(item.tool_name, "Bash");
    }

    #[test]
    fn test_batch_approval_request_new() {
        let items = vec![
            BatchApprovalItem::new("Bash", json!({"cmd": "ls"}), RiskLevel::High),
            BatchApprovalItem::new("Write", json!({"path": "/tmp"}), RiskLevel::Medium),
        ];

        let request = BatchApprovalRequest::new(items);
        assert_eq!(request.items.len(), 2);
        assert!(request.prompt.contains("2"));
        assert!(request.prompt.contains("Bash"));
        assert!(request.prompt.contains("Write"));
    }

    #[test]
    fn test_batch_approval_response_variants() {
        let all_approved = BatchApprovalResponse::AllApproved;
        assert!(matches!(all_approved, BatchApprovalResponse::AllApproved));

        let all_rejected = BatchApprovalResponse::AllRejected { reason: None };
        assert!(matches!(
            all_rejected,
            BatchApprovalResponse::AllRejected { .. }
        ));

        let individual = BatchApprovalResponse::Individual(vec![
            BatchItemDecision::Approved,
            BatchItemDecision::Rejected {
                reason: Some("test".to_string()),
            },
            BatchItemDecision::Skipped,
        ]);
        assert!(matches!(individual, BatchApprovalResponse::Individual(_)));

        let timeout = BatchApprovalResponse::Timeout;
        assert!(matches!(timeout, BatchApprovalResponse::Timeout));
    }
}
