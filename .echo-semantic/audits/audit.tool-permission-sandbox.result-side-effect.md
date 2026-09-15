---
schema_version: 1
id: audit.tool-permission-sandbox.result-side-effect
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: stale
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.effect-cleanup-owner, finding.k8s-sandbox-cleanup-settlement, finding.tool-terminal-observation-divergence, finding.trace-effect-event-producers, finding.tool-pipeline-example-drift]
challenges:
  artifact-sandbox-worktree-cleanup:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/tools/artifact.rs, echo-execution/src/sandbox/manager.rs, echo-tools/src/git_worktree.rs]
    evidence_refs: [evidence.effects-extensions]
  k8s-caller-drop:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-execution/src/sandbox/k8s.rs, echo-execution/src/sandbox/manager.rs]
    evidence_refs: [evidence.effects-extensions]
  tool-terminal-observation:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/react/run/pipeline.rs, echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs]
    evidence_refs: [evidence.effects-extensions, evidence.persistence-observation]
---

# Tool 外部副作用与资源清理审计

## 审查范围

审查 artifact scope、SandboxManager、Local/Docker/K8s、Git worktree、post Hook/Guard/trace/audit/callback 与已发生 effect 的终态表达。

## 已检查故障假设

验证 publish 后 artifact、sandbox/worktree/K8s resource 是否有 owner settlement，caller drop 是否遗留 Pod，以及 post-effect block/failure 是否让 caller、trace、audit 分叉。

## 实际实现路径与证据

已发布 artifact 只能由显式 cleanup API 回收，生产 React path 未调用；SandboxManager cleanup 无生产 owner；worktree marker 写失败无补偿且普通 remove 会拒绝。K8s future drop 只 kill 本地 kubectl，已被 API server 接纳的 Pod 无 detached owner/RAII receipt；delete_pod 又忽略 spawn/exit/确认。PostToolUse block 在 effect 后短路后续 Guard/artifact/trace/callback；失败结果仍调用 on_tool_end，on_tool_error 无调用，Audit 可记录为成功。

## 问题记录

确认 cleanup、trace producer 与 demo drift；新增 K8s cleanup settlement 与 tool terminal observation divergence。CommandCell cancel/finalizer 由 time-lifecycle Finding独立处理。

## 残余风险

K8s 遗留 Pod 尚未在真实集群观测，但源码已证明 owner/settlement 缺失；Local/Docker 已有 detached owner/caller-abort 测试。

## 未检查项

未连接 K8s、未使用 deterministic kubectl fake，未展开每个 domain tool 的 rollback 或 embedding app 自定义 cleanup。
