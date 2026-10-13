---
{
  "schema_version": 1,
  "id": "audit.framework-parent-drop-rereview",
  "kind": "audit",
  "boundary_ref": "boundary.task-subagent-workflow",
  "lens": "time_lifecycle",
  "freshness": "examined",
  "revision": "68925f49814e50a7074e11cc329c40b00c44d2d1",
  "finding_refs": [
    "finding.task-dispatch-parent-drop-owner",
    "finding.task-runtime-store-control",
    "finding.extension-invocation-cancellation-scope",
    "finding.lsp-response-value-fidelity",
    "finding.lsp-diagnostic-notification-fidelity"
  ],
  "challenges": {
    "caller-drop-retained-driver": {
      "revision": "68925f49814e50a7074e11cc329c40b00c44d2d1",
      "source_refs": [
        "echo-core/src/lsp/types.rs",
        "echo-integration/src/lsp/client.rs",
        "echo-integration/src/lsp/jsonrpc.rs",
        "echo-integration/src/lsp/manager.rs",
        "echo-orchestration/src/planning/validator.rs",
        "echo-orchestration/src/tasks/mod.rs",
        "echo-orchestration/src/tasks/revisioned.rs",
        "echo-orchestration/src/tasks/runtime.rs",
        "echo-orchestration/src/tasks/runtime_executor.rs",
        "echo-orchestration/src/tasks/runtime_service.rs",
        "src/acp/extension.rs",
        "src/acp/session.rs",
        "src/agent/subagent/team/mod.rs",
        "src/agent/subagent/types.rs",
        "src/tools/lsp.rs",
        "docs/en/09-tasks.md",
        "docs/zh/09-tasks.md",
        "docs/en/lifecycles.md",
        "docs/zh/lifecycles.md",
        "docs/en/31-lsp-integration.md",
        "docs/zh/31-lsp-integration.md",
        "docs/adr/0060-task-execution-store-cas-and-local-interruption.md",
        "docs/adr/0061-acp-recovered-turn-identity.md",
        "docs/adr/0082-lsp-response-value-fidelity.md"
      ],
      "evidence_refs": [
        "evidence.task-dispatch-parent-drop-repair",
        "evidence.framework-parent-drop-final-verification"
      ],
      "finding_refs": [
        "finding.task-dispatch-parent-drop-owner",
        "finding.task-runtime-store-control"
      ]
    },
    "unchanged-protocol-slices": {
      "revision": "68925f49814e50a7074e11cc329c40b00c44d2d1",
      "source_refs": [
        "src/acp/extension.rs",
        "src/acp/session.rs",
        "echo-core/src/lsp/types.rs",
        "echo-integration/src/lsp/client.rs",
        "echo-integration/src/lsp/manager.rs"
      ],
      "evidence_refs": [
        "evidence.framework-parent-drop-final-verification",
        "evidence.lsp-diagnostic-notification-repair-verification",
        "evidence.lsp-response-value-fidelity-investigation"
      ],
      "finding_refs": [
        "finding.extension-invocation-cancellation-scope",
        "finding.lsp-response-value-fidelity",
        "finding.lsp-diagnostic-notification-fidelity"
      ]
    }
  }
}
---

# 当前框架组合修复独立复审

## 审查范围

host_actor_delta_review 对完整 Task/ACP/LSP 候选、实际直接消费者、三个 ADR 及最终 23 项门禁只读审查；源码摘要 b0d162b72f206d2a94de225d462eaeea3688affc41d8b1336a37562c26a10bb5。原历史 block 保持。

## 已检查故障假设

prejoin caller drop、claim response await、reservation await、driver unwind 后实际物理停止及各 exact claim CAS、已确认 Settled 不降级、未返回身份的 Unknown 不偷取或重试；ACP child token；LSP 原生形状、单缓存权威及 fatal/cache admission。

## 实际实现路径与证据

Task180/Subagent169 focused、三个真实红绿、完整新 23 门禁与全部当前源哈希一致。单一 canonical driver 和 registered Store 保持权威；没有第二 adapter 调度器。ACP/LSP 源码与各独立 scoped pass 逐字节相同，完整组合门禁重新验证。

## 问题记录

复审结论 pass，action items 0；旧父取消阻断已由真实反例、修复、当前门禁与本复审闭合。

## 残余风险

Backend 未返回 exact identity 前 commit 后 unwind 仍需原 backend lease/fencing，不能猜终态。三项外部环境 opt-in ignored 明列。

## 未检查项

公开 framework revision、SDK 最终无 path patch 构建、accepted contract 生成、CLI/官网及精确 main 均未由本审查证明；不关闭远端 Issue、不宣称全 SDK 能力完成。

快照保全：原 b0d162 完整846文件源码已保存为内部 recoverable Git snapshot 68925f49814e50a7074e11cc329c40b00c44d2d1；此历史审查/门禁不证明后续工具校验 public 入口改动，也不是已公开 pin。
