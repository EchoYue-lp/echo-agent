---
{
  "schema_version": 1,
  "id": "audit.framework-combined-parent-drop-review",
  "kind": "audit",
  "boundary_ref": "boundary.task-subagent-workflow",
  "lens": "time_lifecycle",
  "freshness": "examined",
  "revision": "8843afd5029afba9be8a6551de4ab0a93951311f",
  "finding_refs": [
    "finding.task-dispatch-parent-drop-owner"
  ],
  "challenges": {
    "prejoin-parent-drop": {
      "revision": "8843afd5029afba9be8a6551de4ab0a93951311f",
      "source_refs": [
        "echo-orchestration/src/tasks/runtime_executor.rs",
        "echo-orchestration/src/tasks/runtime_service.rs",
        "src/agent/subagent/team/mod.rs",
        "docs/adr/0060-task-execution-store-cas-and-local-interruption.md"
      ],
      "finding_refs": [
        "finding.task-dispatch-parent-drop-owner"
      ],
      "evidence_refs": [
        "evidence.task-runtime-store-control-verification"
      ]
    }
  }
}
---

# Framework 组合独立复审阻断记录

## 审查范围

host_actor_delta_review 对当前 Task/ACP/LSP 组合、直接消费者、ADR0060/0061/0082与23真实门禁核对；plan/design none。来源 source8896e508，23logs全部SHA匹配，15runtime+9docs验证前后字节一致。

## 已检查故障假设

父execute在dispatch join前被drop，局部JoinSet abort物理工作后无人结算claim/watch；公有调用无禁止取消future合同。

## 实际实现路径与证据

已注册Active/watchNone位于spawn前，guard位于joined-wave之后；既有父取消回归在settlementCAS barrier之后。完整3447/0/3ignored/59examples不是该窗口反证。

## 问题记录

独立结论block：Important1，新Finding为finding.task-dispatch-parent-drop-owner。其它已通过切片不由此重写为错误，也不能用它们替代prejoin修复。

## 残余风险

新增修复需要actualabortedjoin/CAS结果和unknownowner保留，以及最终适用门禁重跑/复审。

## 未检查项

不证明SDK全部能力、正式pin、remoteCI/main，也未执行Cargo或Git交付。

历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
