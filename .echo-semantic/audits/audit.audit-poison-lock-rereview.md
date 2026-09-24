---
schema_version: 1
id: audit.audit-poison-lock-rereview
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: contract_evidence
freshness: examined
revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
finding_refs: [finding.in-memory-audit-successful-drop]
challenges:
  poisoned-write-admission:
    revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
    source_refs: [echo-state/src/audit/memory.rs]
    evidence_refs: [evidence.audit-poison-current-repair, evidence.audit-poison-current-verification]
---

# InMemoryAuditLogger poison recovery independent rereview

## 审查范围

独立 reviewer 在 `origin/main@f7c1fef7` 集成源码上复核 Finding #61 的原故障条件、
`InMemoryAuditLogger` 四条公开路径和 focused 回归收据；本审计不扩大为进程持久化合同。

## 已检查故障假设

先前 panic poison 锁后，第二次 `log` 可能不写事件却返回成功；`query`、`snapshot`、
`len` 或 `clear` 也可能丢失、隐藏或无法处理已有事件。

## 实际实现路径与证据

`log` 从 poisoned `RwLock` 取得内部 guard 后 push 事件才返回 `Ok(())`；其余路径同样
恢复 guard。当前主线的 injected-poison focused test 1/1 通过，第二次写入可见，
query/snapshot/clear 与 length 一致。

## 问题记录

独立 reviewer 对现有实现和本轮证据报告 pass、无阻塞发现；此审计支持本分支
Finding resolved，不代表 Issue 已按远端交付口径关闭。

## 残余风险

In-memory logger 不保证进程重启后的持久性；完整 workspace 门禁及远端交付仍待验收。

## 未检查项

独立 reviewer 未自行重跑 focused 测试；未重跑完整 workspace、远端 CI 或 CLI/SDK
消费者链路。
