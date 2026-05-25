//! Tool execution approval (human-in-the-loop)

use super::super::ReactAgent;
use crate::error::{ReactError, Result};
use serde_json::Value;
use tracing::{info, warn};

impl ReactAgent {
    #[cfg(feature = "human-loop")]
    /// Determine whether a tool requires human approval (for process_steps to decide serial/parallel execution)
    ///
    /// Prioritize PermissionService (passing real tool params instead of empty JSON), fall back to legacy two-phase check.
    pub(crate) async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        // 1. PermissionService unified pipeline (fast path: no handler triggers)
        if let Some(service) = &self.approval.permission_service {
            self.flush_pending_permission_rules(service).await;
            let mode = service.mode().await;
            // BypassPermissions / Auto / DontAsk modes do not need serial approval wait
            if matches!(
                mode,
                crate::tools::permission::PermissionMode::BypassPermissions
                    | crate::tools::permission::PermissionMode::DontAsk
            ) {
                return false;
            }
            // Plan mode directly denies write operations (no serial approval needed)
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

        // 2. PermissionPolicy fallback
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

    /// Unified approval check entry point
    ///
    /// Prioritizes PermissionService (unified pipeline: mode → rules → cache → denial → classifier/handler),
    /// then PermissionPolicy check; if approval is required, requests human intervention.
    ///
    /// Returns `Ok(Some(modified_args))` if the user modified parameters during approval (caller should use modified params).
    /// Returns `Ok(None)` if approved, use original params.
    #[cfg(feature = "human-loop")]
    pub(crate) async fn check_tool_approval(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;

        // ── Phase -1: PermissionRequest hook ──
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_permission_request(
                tool_name,
                input,
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let perm_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            // Check block first: a hook may signal block via exit code 2 or
            // {"decision": "block"} without setting a PermissionDecision.
            if perm_result.block {
                let reason = perm_result
                    .block_reason
                    .unwrap_or_else(|| "blocked by hook".to_string());
                let hook_result = self.log_permission_denied(tool_name, &[], &reason).await;
                let retry_hint = if hook_result.retry {
                    " (retry allowed by hook)"
                } else {
                    ""
                };
                return Err(ReactError::Other(format!(
                    "Hook blocked permission for {tool_name}: {reason}{retry_hint}"
                )));
            }
            if let Some(decision) = perm_result.permission_decision {
                match decision {
                    crate::tools::permission::PermissionDecision::Allow => {
                        info!(agent = %agent, tool = %tool_name, "🔓 PermissionRequest hook auto-approved");
                        return Ok(None);
                    }
                    crate::tools::permission::PermissionDecision::Deny { reason } => {
                        let hook_result = self.log_permission_denied(tool_name, &[], &reason).await;
                        let retry_hint = if hook_result.retry {
                            " (retry allowed by hook)"
                        } else {
                            ""
                        };
                        return Err(ReactError::Other(format!(
                            "Hook denied permission for {tool_name}: {reason}{retry_hint}"
                        )));
                    }
                    _ => {} // Ask/RequireApproval: fall through to normal approval flow
                }
            }
        }

        // ── Phase 0: PermissionService unified pipeline ──
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
                .map_err(|e| ReactError::Other(format!("PermissionService error: {e}")))?;

            match decision {
                crate::tools::permission::PermissionDecision::Allow => {
                    let modified = service.take_modified_args().await;
                    return Ok(modified);
                }
                crate::tools::permission::PermissionDecision::Deny { reason } => {
                    let hook_result = self
                        .log_permission_denied(tool_name, &tool_perms, &reason)
                        .await;
                    let retry_hint = if hook_result.retry {
                        " (retry allowed by hook)"
                    } else {
                        ""
                    };
                    return Err(ReactError::Other(format!(
                        "Tool {tool_name} has insufficient permissions: {reason}{retry_hint}"
                    )));
                }
                crate::tools::permission::PermissionDecision::RequireApproval => {
                    info!(agent = %agent, tool = %tool_name, "🔐 PermissionService requires human approval");
                    return self.request_human_approval(tool_name, input).await;
                }
                crate::tools::permission::PermissionDecision::Ask { suggestions } => {
                    return self.handle_ask_decision(tool_name, &suggestions).await;
                }
            }
        }

        // ── Phase 1 (fallback): PermissionPolicy check ──
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
                        let hook_result = self
                            .log_permission_denied(tool_name, &tool_perms, &reason)
                            .await;
                        let retry_hint = if hook_result.retry {
                            " (retry allowed by hook)"
                        } else {
                            ""
                        };
                        return Err(ReactError::Other(format!(
                            "Tool {tool_name} has insufficient permissions: {reason}{retry_hint}"
                        )));
                    }
                    crate::tools::permission::PermissionDecision::RequireApproval => {
                        info!(agent = %agent, tool = %tool_name, "🔐 PermissionPolicy requires human approval");
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
        tool_name: &str,
        _input: &Value,
    ) -> Result<Option<Value>> {
        // Record PermissionDecision trace event (allowed — no human-loop)
        self.record_trace_event(crate::trace::RunEvent::PermissionDecision {
            tool: tool_name.to_string(),
            decision: "allow".into(),
            reason: "Permission check passed (no human-loop)".into(),
        })
        .await;
        Ok(None)
    }

    /// Log permission denied audit event and fire PermissionDenied hook
    #[cfg(feature = "human-loop")]
    async fn log_permission_denied(
        &self,
        tool_name: &str,
        tool_perms: &[crate::tools::permission::ToolPermission],
        reason: &str,
    ) -> crate::skills::hooks::HookResult {
        let agent = &self.config.agent_name;
        let session_id = self.config.session_id.clone().unwrap_or_default();
        warn!(agent = %agent, tool = %tool_name, reason = %reason, "🔒 Permission denied");
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

        // Fire PermissionDenied hook
        let hook_ctx = crate::skills::hooks::HookContext::for_permission_denied(
            tool_name,
            &serde_json::json!({}),
            reason,
            true, // retry_allowed: model can retry with different params
            &session_id,
            agent,
        );
        let registry = self.tools.hook_registry.read().await.clone();
        registry.run_lifecycle_hooks(&hook_ctx).await
    }

    /// Handle Ask decision — confirm tool execution with the user
    #[cfg(feature = "human-loop")]
    async fn handle_ask_decision(
        &self,
        tool_name: &str,
        suggestions: &[String],
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        info!(agent = %agent, tool = %tool_name, "❓ Permission requires user confirmation");
        let prompt = format!(
            "Tool '{}' requires confirmation. Suggested options: {}",
            tool_name,
            suggestions.join(", ")
        );
        let req = crate::human_loop::HumanLoopRequest::input(prompt);
        match self.approval.approval_provider.request(req).await? {
            crate::human_loop::HumanLoopResponse::Text(response) => {
                if response.to_lowercase().contains("reject")
                    || response.to_lowercase().contains("deny")
                {
                    return Err(ReactError::Other(format!(
                        "Tool {tool_name} user chose to reject: {response}"
                    )));
                }
                info!(agent = %agent, tool = %tool_name, "✅ User confirmed execution");
            }
            crate::human_loop::HumanLoopResponse::Approved => {
                info!(agent = %agent, tool = %tool_name, "✅ User confirmed execution");
            }
            crate::human_loop::HumanLoopResponse::Rejected { reason } => {
                return Err(ReactError::Other(format!(
                    "Tool {tool_name} user rejected{}",
                    reason.map(|r| format!(", reason: {r}")).unwrap_or_default()
                )));
            }
            _ => {
                return Err(ReactError::Other(format!(
                    "Tool {tool_name} user confirmation timed out"
                )));
            }
        }
        Ok(None)
    }

    /// Request human approval and record audit log
    ///
    /// Handles all `HumanLoopResponse` variants, consistently recording approval request/completion audit events.
    /// Risk level is dynamically computed based on tool permissions rather than hardcoded.
    ///
    /// Returns `Ok(Some(modified_args))` if the user modified parameters and approved.
    /// Returns `Ok(None)` if the user directly approved.
    #[cfg(feature = "human-loop")]
    async fn request_human_approval(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        let approval_start = std::time::Instant::now();

        // Fire Notification hook (permission_prompt) before requesting human approval
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_notification(
                "permission_prompt",
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let notif_result = registry.run_lifecycle_hooks(&hook_ctx).await;
            // If a hook provides a permission decision, short-circuit the approval flow
            if let Some(decision) = notif_result.permission_decision {
                match decision {
                    crate::tools::permission::PermissionDecision::Allow => {
                        info!(agent = %agent, tool = %tool_name, "🔓 Notification hook auto-approved");
                        return Ok(None);
                    }
                    crate::tools::permission::PermissionDecision::Deny { reason } => {
                        return Err(ReactError::Other(format!(
                            "Hook auto-denied {tool_name}: {reason}"
                        )));
                    }
                    _ => {} // Ask/RequireApproval: continue to human approval
                }
            }
        }

        // Dynamically compute risk level based on tool permissions
        let tool_perms = self
            .tools
            .tool_manager
            .get_tool(tool_name)
            .map(|t| t.permissions())
            .unwrap_or_default();
        let risk_level = crate::human_loop::RiskLevel::from_permissions(&tool_perms);
        let risk_level_str = format!("{:?}", risk_level).to_lowercase();

        // Audit: approval request
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

        // Audit: approval completed
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
                info!(agent = %agent, tool = %tool_name, "✅ User approved tool execution");
                Ok(None)
            }
            crate::human_loop::HumanLoopResponse::ApprovedWithScope { scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ User approved tool execution with scope");
                Ok(None)
            }
            crate::human_loop::HumanLoopResponse::ModifiedArgs { args, scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ User modified parameters and approved tool execution");
                Ok(Some(args))
            }
            crate::human_loop::HumanLoopResponse::Rejected { reason } => {
                warn!(agent = %agent, tool = %tool_name, "❌ User rejected tool execution");
                Err(ReactError::Other(format!(
                    "User rejected tool execution {}{}",
                    tool_name,
                    reason.map(|r| format!(", reason: {r}")).unwrap_or_default()
                )))
            }
            crate::human_loop::HumanLoopResponse::Timeout => {
                warn!(agent = %agent, tool = %tool_name, "⏰ Approval timed out");
                Err(ReactError::Other(format!(
                    "Tool {tool_name} approval timed out, execution skipped"
                )))
            }
            crate::human_loop::HumanLoopResponse::Deferred => {
                warn!(agent = %agent, tool = %tool_name, "⏸️ User deferred approval");
                Err(ReactError::Other(format!(
                    "Tool {tool_name} approval deferred, execution skipped"
                )))
            }
            crate::human_loop::HumanLoopResponse::Text(_) => {
                warn!(agent = %agent, tool = %tool_name, "⚠️ Approval request received unexpected Text response");
                Err(ReactError::Other(format!(
                    "Tool {tool_name} approval abnormal, execution skipped"
                )))
            }
        }
    }
}
