//! 工具执行审批（人工介入）

use super::super::ReactAgent;
use crate::error::{ReactError, Result};
use serde_json::Value;
use tracing::{info, warn};

impl ReactAgent {
    #[cfg(feature = "human-loop")]
    /// 判断工具是否需要人工审批（供 process_steps 决定串行/并行执行）
    ///
    /// 优先使用 PermissionService（传递真实工具参数而非空 JSON），回退到旧的两阶段检查。
    pub(crate) async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        // 1. PermissionService 统一管线（快速路径：不触发 handler）
        if let Some(service) = &self.approval.permission_service {
            self.flush_pending_permission_rules(service).await;
            let mode = service.mode().await;
            // BypassPermissions / Auto / DontAsk 模式不需要串行等待审批
            if matches!(
                mode,
                crate::tools::permission::PermissionMode::BypassPermissions
                    | crate::tools::permission::PermissionMode::DontAsk
            ) {
                return false;
            }
            // Plan 模式下写操作直接拒绝（不需要串行审批）
            if mode == crate::tools::permission::PermissionMode::Plan {
                return false;
            }

            // Get actual tool permissions to pass to service.check_with_permissions
            let tool_perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            // Use check_with_permissions with real perms instead of empty JSON
            let decision = service
                .check_with_permissions(tool_name, &serde_json::json!({}), &tool_perms)
                .await
                .unwrap_or(crate::tools::permission::PermissionDecision::RequireApproval);

            return decision.requires_approval();
        }

        // 2. PermissionPolicy 回退
        if let Some(policy) = &self.guard.permission_policy {
            let tool_perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            if !tool_perms.is_empty() {
                let decision = policy.check(tool_name, &tool_perms).await;
                if matches!(
                    decision,
                    crate::tools::permission::PermissionDecision::RequireApproval
                        | crate::tools::permission::PermissionDecision::Ask { .. }
                ) {
                    return true;
                }
            }
        }

        false
    }

    #[cfg(not(feature = "human-loop"))]
    pub(crate) async fn tool_needs_approval(&self, _tool_name: &str) -> bool {
        false
    }

    /// 统一审批检查入口
    ///
    /// 优先使用 PermissionService（统一管线: mode → rules → cache → denial → classifier/handler），
    /// PermissionPolicy 检查，需审批则请求人工介入。
    ///
    /// 返回 `Ok(Some(modified_args))` 表示用户在审批时修改了参数，调用方应使用修改后的参数。
    /// 返回 `Ok(None)` 表示审批通过，使用原始参数。
    #[cfg(feature = "human-loop")]
    pub(crate) async fn check_tool_approval(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;

        // ── Phase 0: PermissionService 统一管线 ──
        if let Some(service) = &self.approval.permission_service {
            self.flush_pending_permission_rules(service).await;
            let tool_perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            let decision = service
                .check_with_permissions(tool_name, input, &tool_perms)
                .await
                .map_err(|e| ReactError::Other(format!("PermissionService 错误: {e}")))?;

            match decision {
                crate::tools::permission::PermissionDecision::Allow => {
                    let modified = service.take_modified_args().await;
                    return Ok(modified);
                }
                crate::tools::permission::PermissionDecision::Deny { reason } => {
                    self.log_permission_denied(tool_name, &tool_perms, &reason)
                        .await;
                    return Err(ReactError::Other(format!(
                        "工具 {tool_name} 权限不足: {reason}"
                    )));
                }
                crate::tools::permission::PermissionDecision::RequireApproval => {
                    info!(agent = %agent, tool = %tool_name, "🔐 权限服务要求人工审批");
                    return self.request_human_approval(tool_name, input).await;
                }
                crate::tools::permission::PermissionDecision::Ask { suggestions } => {
                    return self.handle_ask_decision(tool_name, &suggestions).await;
                }
            }
        }

        // ── Phase 1 (回退): PermissionPolicy 检查 ──
        if let Some(policy) = &self.guard.permission_policy {
            let tool_perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            if !tool_perms.is_empty() {
                let decision = policy.check(tool_name, &tool_perms).await;
                match decision {
                    crate::tools::permission::PermissionDecision::Allow => {}
                    crate::tools::permission::PermissionDecision::Deny { reason } => {
                        self.log_permission_denied(tool_name, &tool_perms, &reason)
                            .await;
                        return Err(ReactError::Other(format!(
                            "工具 {tool_name} 权限不足: {reason}"
                        )));
                    }
                    crate::tools::permission::PermissionDecision::RequireApproval => {
                        info!(agent = %agent, tool = %tool_name, "🔐 权限策略要求人工审批");
                        return self.request_human_approval(tool_name, input).await;
                    }
                    crate::tools::permission::PermissionDecision::Ask { suggestions } => {
                        return self.handle_ask_decision(tool_name, &suggestions).await;
                    }
                }
            }
        }

        Ok(None)
    }

    #[cfg(not(feature = "human-loop"))]
    pub(crate) async fn check_tool_approval(
        &self,
        _tool_name: &str,
        _input: &Value,
    ) -> Result<Option<Value>> {
        Ok(None)
    }

    /// 记录权限拒绝审计日志
    #[cfg(feature = "human-loop")]
    async fn log_permission_denied(
        &self,
        tool_name: &str,
        tool_perms: &[crate::tools::permission::ToolPermission],
        reason: &str,
    ) {
        let agent = &self.config.agent_name;
        warn!(agent = %agent, tool = %tool_name, reason = %reason, "🔒 权限拒绝");
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.to_string(),
                crate::audit::AuditEventType::PermissionDenied {
                    tool: tool_name.to_string(),
                    required: tool_perms.to_vec(),
                    reason: reason.to_string(),
                },
            );
            let _ = al.log(event).await;
        }
    }

    /// 处理 Ask 决策 — 向用户确认工具执行
    #[cfg(feature = "human-loop")]
    async fn handle_ask_decision(
        &self,
        tool_name: &str,
        suggestions: &[String],
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        info!(agent = %agent, tool = %tool_name, "❓ 权限需要用户确认");
        let prompt = format!(
            "工具 '{}' 需要确认。建议选项：{}",
            tool_name,
            suggestions.join(", ")
        );
        let req = crate::human_loop::HumanLoopRequest::input(prompt);
        match self.approval.approval_provider.request(req).await? {
            crate::human_loop::HumanLoopResponse::Text(response) => {
                if response.to_lowercase().contains("拒绝")
                    || response.to_lowercase().contains("deny")
                {
                    return Err(ReactError::Other(format!(
                        "工具 {tool_name} 用户选择拒绝：{response}"
                    )));
                }
                info!(agent = %agent, tool = %tool_name, "✅ 用户确认执行");
            }
            crate::human_loop::HumanLoopResponse::Approved => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户确认执行");
            }
            crate::human_loop::HumanLoopResponse::Rejected { reason } => {
                return Err(ReactError::Other(format!(
                    "工具 {tool_name} 用户拒绝{}",
                    reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                )));
            }
            _ => {
                return Err(ReactError::Other(format!("工具 {tool_name} 用户确认超时")));
            }
        }
        Ok(None)
    }

    /// 请求人工审批并记录审计日志
    ///
    /// 处理所有 `HumanLoopResponse` 变体，统一记录审批请求/完成审计事件。
    /// 风险等级根据工具权限动态计算，而非硬编码。
    ///
    /// 返回 `Ok(Some(modified_args))` 表示用户修改了参数后批准。
    /// 返回 `Ok(None)` 表示用户直接批准。
    #[cfg(feature = "human-loop")]
    async fn request_human_approval(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        let approval_start = std::time::Instant::now();

        // 根据工具权限动态计算风险等级
        let tool_perms = self
            .tools
            .tool_manager
            .get_tool(tool_name)
            .map(|t| t.permissions())
            .unwrap_or_default();
        let risk_level = crate::human_loop::RiskLevel::from_permissions(&tool_perms);
        let risk_level_str = format!("{:?}", risk_level).to_lowercase();

        // 审计：审批请求
        if let Some(al) = &self.guard.audit_logger {
            let args_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                format!("{input}").hash(&mut hasher);
                format!("{:016x}", hasher.finish())
            };
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.clone(),
                crate::audit::AuditEventType::ApprovalRequested {
                    tool: tool_name.to_string(),
                    args_hash,
                    risk_level: risk_level_str,
                },
            );
            let _ = al.log(event).await;
        }

        let req = crate::human_loop::HumanLoopRequest::approval(tool_name, input.clone());
        let response = self.approval.approval_provider.request(req).await?;

        // 审计：审批完成
        if let Some(al) = &self.guard.audit_logger {
            let duration_ms = approval_start.elapsed().as_millis() as u64;
            let (decision, scope, reason) = match &response {
                crate::human_loop::HumanLoopResponse::Approved => {
                    ("approved".into(), "once".into(), None)
                }
                crate::human_loop::HumanLoopResponse::ApprovedWithScope { scope: s } => {
                    ("approved".into(), format!("{s:?}"), None)
                }
                crate::human_loop::HumanLoopResponse::ModifiedArgs { args: _, scope: s } => {
                    ("modified".into(), format!("{s:?}"), None)
                }
                crate::human_loop::HumanLoopResponse::Rejected { reason: r } => {
                    ("rejected".into(), "once".into(), r.clone())
                }
                crate::human_loop::HumanLoopResponse::Timeout => {
                    ("timeout".into(), "once".into(), None)
                }
                crate::human_loop::HumanLoopResponse::Deferred => {
                    ("deferred".into(), "once".into(), None)
                }
                crate::human_loop::HumanLoopResponse::Text(_) => {
                    ("unexpected".into(), "once".into(), None)
                }
            };
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.clone(),
                crate::audit::AuditEventType::ApprovalCompleted {
                    tool: tool_name.to_string(),
                    decision,
                    scope,
                    reason,
                    duration_ms,
                },
            );
            let _ = al.log(event).await;
        }

        match response {
            crate::human_loop::HumanLoopResponse::Approved => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户批准执行工具");
                Ok(None)
            }
            crate::human_loop::HumanLoopResponse::ApprovedWithScope { scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户带范围批准执行工具");
                Ok(None)
            }
            crate::human_loop::HumanLoopResponse::ModifiedArgs { args, scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户修改参数后批准执行工具");
                Ok(Some(args))
            }
            crate::human_loop::HumanLoopResponse::Rejected { reason } => {
                warn!(agent = %agent, tool = %tool_name, "❌ 用户拒绝执行工具");
                Err(ReactError::Other(format!(
                    "用户已拒绝执行工具 {}{}",
                    tool_name,
                    reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                )))
            }
            crate::human_loop::HumanLoopResponse::Timeout => {
                warn!(agent = %agent, tool = %tool_name, "⏰ 审批超时");
                Err(ReactError::Other(format!(
                    "工具 {tool_name} 审批超时，已跳过执行"
                )))
            }
            crate::human_loop::HumanLoopResponse::Deferred => {
                warn!(agent = %agent, tool = %tool_name, "⏸️ 用户推迟审批");
                Err(ReactError::Other(format!(
                    "工具 {tool_name} 审批被推迟，已跳过执行"
                )))
            }
            crate::human_loop::HumanLoopResponse::Text(_) => {
                warn!(agent = %agent, tool = %tool_name, "⚠️ 审批请求收到意外的 Text 响应");
                Err(ReactError::Other(format!(
                    "工具 {tool_name} 审批异常，已跳过执行"
                )))
            }
        }
    }
}
