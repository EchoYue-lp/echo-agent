---
schema_version: 1
id: audit.effect-cleanup-owner-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
finding_refs: [finding.effect-cleanup-owner]
challenges:
  artifact-pending-and-path-identity:
    revision: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
    source_refs: [echo-core/src/tools/artifact.rs, src/agent/react/run/pipeline.rs, docs/adr/0072-resource-cleanup-ownership.md]
    evidence_refs: [evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
  backend-exact-owner-and-agent-close:
    revision: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
    source_refs: [echo-execution/src/sandbox/resource_owner.rs, echo-execution/src/sandbox/docker.rs, echo-execution/src/sandbox/k8s.rs, echo-execution/src/sandbox/manager.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
  worktree-marker-and-receipt-compensation:
    revision: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
    source_refs: [echo-tools/src/git_worktree.rs, docs/adr/0072-resource-cleanup-ownership.md]
    evidence_refs: [evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
---

# 精确资源 cleanup owner 独立复审

## 审查范围

独立 reviewer 只读复核 `#47` 最终 diff、源码、ADR、双语文档和 focused
结果，重点检查 Artifact、Docker/K8s、SandboxManager、Agent close 与
Git worktree 的资源身份、取消后 owner 延续及终态可见性。末次测试断言改动
后再次复核最终差异。

## 已检查故障假设

检查活动 artifact writer 是否被 scope/age cleanup 删除，deferred 失败是否
丢失；alias 或替换目录能否借旧身份删除新资源；Docker/K8s 创建前后失败及
caller drop 是否留下无 owner 的资源；实例 cleanup 是否误扫共享 label，
Agent close 是否因 MCP 错误跳过 sandbox；Git add 与 marker/ack 之间取消、
写入失败、替换 checkout 或脏状态是否触发错误补偿。

## 实际实现路径与证据

最终实现为每类资源保留精确身份与 pending/debt。Artifact writer release
驱动 deferred cleanup；backend registry 拒绝用实例 cleanup 代替全局 sweep；
Agent close 尝试两个独立结算；worktree owner 在 caller 离开后继续发布
marker 或按 guard、分支和干净状态补偿。对应 focused 测试结果记录在
verification Evidence，reviewer 没有自行执行测试。

## 问题记录

独立最终复审对当前源码 diff 返回 PASS、0 findings；测试断言修正后
all-feature 定向回归 1/1 与隔离 target 完整 `./scripts/verify.sh` 均 exit 0。
最终源码的独立 17-feature 条件矩阵亦全部 exit 0。该结论支持本分支
`finding.effect-cleanup-owner` resolved；远端 PR/CI 与 main 交付仍另行验收。

## 残余风险

ADR 0072 明确本地路径操作的身份检查是安全点，不是对不协调外部 OS actor
的原子 fence；进程或 runtime 崩溃还需要显式恢复。

## 未检查项

reviewer 未重跑测试；真实 Docker/K8s 集群、PR/CI 与远端 main 验收另行执行。
