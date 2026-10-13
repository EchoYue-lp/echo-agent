---
schema_version: 1
id: finding.channel-handler-invocation-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.channel-handler-invocation]
rule_refs: [rule.channel-invocation-generation]
evidence_refs: [evidence.channel-handler-forward-full-gates, evidence.channel-handler-forward-repair, evidence.channel-handler-forward-focused]
audit_refs: [audit.channel-handler-invocation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.channel-handler-forward-repair]
verification_evidence_refs: [evidence.channel-handler-forward-full-gates, evidence.channel-handler-forward-focused]
rereview_audit_refs: [audit.channel-handler-invocation-rereview]
discovered_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
---

# Channel 前向调用准入与真实结算

## 问题

新的handler前向consumer若裸Arc调用或依据handler存在接消息，会绕过starting/failed
rollback/stop边界。实现独立审查发现queuedreset取消仍可替换session、正常EOF误取消
child信号以及retire ready=false→cancel解锁窗口可被open重开；这些必须闭合。

## 触发条件与影响

同entry start/stop并发、排队reset取消与stream满缓冲/自然EOF会影响准入与关闭结算。

## 唯一权威与修复

原entry owned API+exactgenerationchildtoken/receipt，Session原childdrain，原Agent
驱动；cancel不退休兄弟generation。Mutex覆盖retire两个步骤。闭合不增加SDKwirecode
或第二stateauthority。完整代码、先前reviewanchors/diff、真实失败命令见档案。

## 证据

见focused13原日志与两份Evidence，历史源和本轮待验明确分开。

## 处理记录

源码独立审查确认已知实现问题已修；原22/79/12/2docs/双lint真实通过。后继mutex修复
使这些工程证据对新manager失效，新增23/80/lint、全部23与独立最终复核尚待。
Finding保持open，不能只靠准备metadata或mapped宣称resolved/交付。


当前完整23已补齐：evidence.channel-handler-forward-full-gates绑定同source d056，23/80具名回归、warning0、全部feature门禁通过；上文待验为历史准备记录。独立最终receipt复核与SDK/官网/exactmain仍分开，不借本地全量门禁关闭无关问题。


当前本地关闭：完整当前source d056之23门禁与23新增/Channel80具名回归通过，
audit.channel-handler-invocation-rereview记录独立源码/52原字节档案/939manifest及完整
收据PASS action0；三链完整，故仅本Finding resolved。SDK/官网/精确main仍另验。
