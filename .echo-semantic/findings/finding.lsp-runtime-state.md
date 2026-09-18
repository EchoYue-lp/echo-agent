---
schema_version: 1
id: finding.lsp-runtime-state
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.time-lifecycle, audit.lsp-runtime-state-rereview]
decision_refs: []
repair_evidence_refs: [evidence.lsp-runtime-state-repair]
verification_evidence_refs: [evidence.lsp-runtime-state-verification]
rereview_audit_refs: [audit.lsp-runtime-state-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# LSP runtime status 与 restart 字段未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/64

## 问题

Public max_restarts/restart_count/last_error 没有更新路径；reader EOF 只清 pending，不重置 running/initialized；load_config 与重复 start 也不撤销旧状态。

## 触发条件与影响

Language server 异常退出、配置 reload 或重复启动时，查询状态与真实 child process 可能不一致，pending caller 和资源清理也可能漂移。

## 证据

`echo-core/src/lsp/types.rs`、`echo-integration/src/lsp/client.rs` 与 `lsp/manager.rs` 提供源码反例。

## 处理记录

Framework 修复候选位于 `c04ab97fdbdc7712af36360de5db10cfeeccfec1`：
EOF/restart/reload/shutdown 合同测试和局部编译、lint 已通过。Finding 保持 open，
等待独立复审、SDK/EKO 消费端对齐、完整合并门禁、网站文档同步与远端 main 交付。
