//! 适配器 (Adapter)
//!
//! 将旧的权限/审批接口适配到统一的 `PermissionService` 管线，
//! 保证向后兼容的同时允许渐进迁移。
//!
//! ## 适配方向
//!
//! - `PermissionPolicyAdapter`: `dyn PermissionPolicy` → `PermissionService` 规则
//! - `ProviderHandlerAdapter`:  `dyn HumanLoopProvider` → `PermissionRequestHandler`

use async_trait::async_trait;
use echo_core::tools::permission::PermissionDecision;
use std::sync::Arc;

use super::permission::{
    PermissionRequest, PermissionRequestHandler, PermissionResponse, PermissionResponseDecision,
};
use super::service::PermissionService;
use echo_core::error::Result;

// ── PermissionPolicy → PermissionService 适配器 ──────────────────────────────

/// 将已有的 `dyn PermissionPolicy` 适配到 `PermissionService`
///
/// 内部使用一个桥接 `PermissionRequestHandler`，在 handler 中调用原始 Policy 的
/// `check()` 方法，并将结果转换为 `PermissionResponse`。
pub struct PermissionPolicyAdapter {
    policy: Arc<dyn echo_core::tools::permission::PermissionPolicy>,
}

impl PermissionPolicyAdapter {
    /// 从已有 Policy 创建适配器
    pub fn new(policy: Arc<dyn echo_core::tools::permission::PermissionPolicy>) -> Self {
        Self { policy }
    }

    /// 将适配器包装为完整的 `PermissionService`
    ///
    /// 保留原始 Policy 的语义，同时获得 PermissionService 的管线能力。
    pub fn into_service(self) -> PermissionService {
        let handler: Arc<dyn PermissionRequestHandler> = Arc::new(PolicyBridgeHandler {
            policy: self.policy.clone(),
        });

        PermissionService::new().with_request_handler(handler)
    }
}

/// 桥接 handler：调用 `PermissionPolicy::check()` 并转换结果
struct PolicyBridgeHandler {
    policy: Arc<dyn echo_core::tools::permission::PermissionPolicy>,
}

#[async_trait]
impl PermissionRequestHandler for PolicyBridgeHandler {
    async fn handle(&self, request: PermissionRequest) -> Result<PermissionResponse> {
        let decision = self
            .policy
            .check(&request.tool_name, &request.required_permissions)
            .await;

        Ok(match decision {
            PermissionDecision::Allow => PermissionResponse::allowed(),
            PermissionDecision::Deny { reason } => PermissionResponse::denied(Some(reason)),
            PermissionDecision::RequireApproval => {
                // 需要人工审批 — 返回 denied 让调用者走人工流程
                PermissionResponse::denied(Some("需要人工审批".to_string()))
            }
            PermissionDecision::Ask { suggestions } => PermissionResponse {
                decision: PermissionResponseDecision::NeedMoreInfo {
                    question: suggestions.join(", "),
                },
                rule_updates: Vec::new(),
                feedback: None,
                updated_input: None,
            },
        })
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::permission::{PermissionDecision, ToolPermission};
    use futures::future::BoxFuture;

    /// 测试用的 Mock PermissionPolicy
    struct MockPolicy;

    impl echo_core::tools::permission::PermissionPolicy for MockPolicy {
        fn check<'a>(
            &'a self,
            _tool_name: &'a str,
            permissions: &'a [ToolPermission],
        ) -> BoxFuture<'a, PermissionDecision> {
            Box::pin(async move {
                if permissions.contains(&ToolPermission::Execute) {
                    PermissionDecision::RequireApproval
                } else {
                    PermissionDecision::Allow
                }
            })
        }
    }

    #[tokio::test]
    async fn test_policy_adapter_into_service() {
        let policy = Arc::new(MockPolicy);
        let adapter = PermissionPolicyAdapter::new(policy);
        let _service = adapter.into_service();
    }
}
