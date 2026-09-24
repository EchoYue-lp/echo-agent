---
schema_version: 1
id: audit.memory-provenance-authority-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: permission_external
freshness: examined
revision: 733d352fc719f922b21bab1cd46206139564367f
finding_refs: [finding.pre-compaction-memory-trust-provenance]
challenges:
  exact-origin-and-producer-admission:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [src/agent/react/run/context.rs, src/agent/react/run/phases/compact.rs, src/memory_promoter.rs, src/evolution/triggers.rs, echo-state/src/compression/mod.rs]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
  activation-and-recovery-authority:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [echo-core/src/memory/types.rs, src/evolution/layer.rs, src/evolution/review.rs, src/evolution/dreaming.rs]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
  agent-tool-and-hot-warm-recall:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [src/agent/react/mod.rs, src/agent/react/builder.rs, src/evolution/recall.rs, src/evolution/layer.rs, src/tools/builtin/memory.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
  public-contract-and-examples:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [README.md, README.zh.md, docs/en/03-memory.md, docs/zh/03-memory.md, docs/adr/0070-memory-provenance-and-recall-authority.md, echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs, echo-agent-learning/examples/demo18_semantic_memory.rs, echo-agent-learning/examples/demo27_sqlite_memory.rs, echo-agent-learning/examples/demo45_customer_service.rs]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
---

# Issue 76 memory provenance independent rereview

## 审查范围

独立 reviewer 在 `fix/Echoyue/memory-provenance-authority` worktree 对
`main@c45f83cbed1722f86d3d99f0f7e2edd2ff105016` 的 framework diff 只读复审。
核对 Plan 05 schema v4 与总设计的唯一权威、持久恢复、异常边界、公共合同和 Finding
验收章节；沿自动写入、Draft 激活、File/SQLite 重启、Hot/Warm 召回、Agent 工具、
文档与 examples 的真实入口追踪。

## 已检查故障假设

复审先后提出：默认 Store 工具绕过 Draft 准入、Assistant 偏好被激活、
长工具证据令 Horizon 失败后丢失消息、trigger 引文不等于原文、Hot 晋升后自动
召回消失、框架合成 User/Assistant 消息伪装来源、无 manager 写入死端 Draft，以及
Store setter 与 manager 指向不同后端或忙时发布半套配置。

## 实际实现路径与证据

上述路径当前均有生产修复和对应 focused 回归。`MemoryMeta` 的统一准入、manager
的 journaled 激活、Agent Store 工具的受审阅召回、Hot 常驻上下文与 Store 绑定，
以及合成消息来源过滤均在 repair/verification Evidence 中有源码和测试引用。

## 问题记录

初轮复审提出的反例已逐项修复；最终增量复审未发现新 Critical、Important 或 Minor，
行动项 0，结论 PASS。

## 验证边界

主任务执行 `./scripts/verify.sh`（session 60631，exit 0）、17 项独立 feature
matrix（session 82347，exit 0）及 strict semantic/change-evidence（session 88849，
exit 0）。独立 reviewer 未重跑完整门禁，也无可独立读取的完整门禁日志；其只读
核对包括当前 diff、Plan 合同与 `git diff --check`。结论绑定上列 source digest。

## 残余风险

`MemoryApproval` 的 reviewer 身份仍由 embedding host 负责；直接使用 raw Store
的外部代码不享有 manager 已结算读取保证，均为 ADR 0070 明确的边界。复审后
PR #152 的七项 CI 全绿，修复经签名 squash commit `5a0f2af2` 进入 framework
main；合并后 strict semantic 在相同 source digest 上通过。外部 Issue 关闭应在
本语义状态进入 main 后执行。

## 未检查项

SDK、CLI、website 与 A2A 不在本次 framework-only 复审范围内。
