---
{
  "schema_version": 1,
  "id": "finding.subagent-physical-settlement-observation",
  "kind": "finding",
  "type": "implementation_bug",
  "status": "open",
  "severity": "high",
  "primary_focus": "time_lifecycle",
  "focus": [
    "state_authority",
    "failure_concurrency",
    "result_side_effect",
    "contract_evidence"
  ],
  "boundary_ref": "boundary.task-subagent-workflow",
  "behavior_refs": [
    "behavior.task-subagent-execution",
    "behavior.agent-turn-lifecycle"
  ],
  "rule_refs": [
    "rule.task-subagent-authority",
    "rule.turn-terminal-authority"
  ],
  "evidence_refs": [
    "evidence.subagent-physical-settlement-repair",
    "evidence.workflow-observation-negative-controls"
  ],
  "audit_refs": [
    "audit.subagent-physical-settlement-focused-review"
  ],
  "decision_refs": [],
  "discovered_at": "00782b8aeddefe2ea8c566922cbdee6aedc80d70",
  "repair_evidence_refs": [
    "evidence.subagent-physical-settlement-repair"
  ],
  "verification_evidence_refs": [
    "evidence.subagent-physical-settlement-repair",
    "evidence.workflow-observation-negative-controls"
  ],
  "rereview_audit_refs": [
    "audit.subagent-physical-settlement-focused-review"
  ]
}
---

# Cancellation observation cannot settle the original physical owner

## 问题

Original grace abort and logical stream terminal/EOF could precede physical
producer cleanup, releasing claim, permit, isolation or invocation guards and
misclassifying observation uncertainty as Failed. Canonical authority remains
the original RuntimeTaskService/RuntimeDagExecutor/SubagentExecutor, original
producer and exact-claim CAS.

## 触发条件与影响

The user selected B, recorded in ADR0088: first admitted cancellation starts only
an observer deadline, normal execution has no grace timer, exact A does not
interrupt sibling B, and unknown closure never authorizes repeated execution.

## 证据

Current source and focused results are bound by the Evidence and independent
Audit. Manager 335, React run 189, Subagent 69 and RunCode 16 passed; formatter and
input guards had no drift. Three Task claim cases retain the original producer,
complete claim and actual terminal outcome; they are not represented as separate
original-source before executions. Final source review is PASS C0/I0/M0 and
supports the original focused development checkpoint. Six local gates have since
passed as recorded below. The Finding stays open for the identified live-source observation-to-terminal
gap and consumer/main delivery.

## 处理记录

Closure requires current-source held-cleanup, normal-long-run, exact sibling,
repeat-request, caller-drop/unwind, unknown restart, late uncertainty, original
mode capacity/resources, input watch and post-CAS priority proofs with independent
review. Focused verification does not represent merged-main completion.

ACP close increment is independently reviewed C0/I0/M0 after genuine held-backend
identity/retry and unidentified owner-loss failures. All 18 original Session tests
pass after the repair, including ordinary retry/success. Remaining full merge,
feature and consumer/main acceptance keeps this Finding open.

最终本地六门禁已全部通过（3580主通过/0失败/3既有live opt-in忽略），Headless便利返回丢唯一owner的真实反例已按同handle投影修复并独审C0/I0/M0；原3fixture错误保持失败记录并以更强原EOF/Unknown断言纠正。此更新不宣独feature或消费者/main通过。Plan验收明确精确merged-main CI，故当前ready与本Finding open保留交付缺口；后续只按真实结果收口。

当前23适用本地门禁实际0（6原命令+17独feature），但已识别live source上的观察知识会在后续合法terminal+EOF后仍被无条件上送，原owner可能无法结算。后继尚未新增真实red/green或改源；无source Unknown规则不能覆盖有同owner/gen真实结算来源的场景。本checkpoint保留该明确缺口，未宣全B完成，未合main。

## Live-source后继的局部完成

f0a6公开checkpoint之后，同源真实terminal+physical EOF仍被旧观察覆盖的Driver反例已实际101并修后0；Core426/Orch477/Subagent70及适用三crate Clippy/fmt实际0，独立有限source增量C0/I0/M0。匹配源成功/取消结算、无terminalEOF、foreign/wrong-attempt、晚观察和sink独立强例保留。旧23资格仍固定f0a6，不借给当前新源码。

本Finding继续open：已确认Scheduler callback把typed观察变Failed/终态及owner-loss重投，Workflow resume无条件requeue且heartbeat失败会丢原future，File lease或远端checkpoint恢复尚无未知物理权威闭包。两者按原Plan同Outcome后继独立调查/真实counter/窄设计；未实施，不声称新恢复协议或普通重试合同全改。当前全6+17与跨仓消费、精确merged-main CI仍待最终代码验证。

## Scheduler局部闭合与后继边界

原Scheduler消费者缺口已有真实红绿：保护同occurrence/attempt、保原callback及Handle parent，不从typed观察、borrowed drop、shutdown或panic制造terminal/replay；legacy本次effect也在原EffectStarted中升级。最终whole53与原Delivery14、相关Clippy/fmt实际0，独立增量C0/I0/M0。先前“Scheduler未实施”仅属于ffff历史切点。

本Finding继续open。用户已接受Workflow保护合同A；其Graph/Store/heartbeat与持久化恢复尚待同任务正式增量实施、真实反例及验证。当前新Scheduler字节仍需最终6+17、SDK/CLI适配与精确merged-main CI，不借f0a历史23通过关闭本Finding。

## Workflow A局部闭合与交付边界

3ded之后已按用户接受A实现原claim effect-before保护、NativeStore原锁/私有格式与Graph唯一ownedloop/Node rawsource闭合；真实重claim、caller/stream Drop、marker降级、chat截流、fanout panic、identity/firstyield/heartbeat及typed Store错误反例均有保留原件与after。Core427/Orch508、相关Clippy/fmt实际0及独立有界审通过；原File reader真实双进程证明限定旧3ded模块与原依赖，不宣全部旧写者/掉电/跨平台保证。

Finding仍open：当前最终6+17尚未重跑、SDK注册Store/三语Host与CLI精确新main消费及merged-main CI未完成。此前“Workflow尚未实施”仅为3ded以前历史状态；无source未知仍无强制恢复/解锁承诺。

## 框架最终本地资格

当前663框架代码完整6+17实际全0，主测试3620/0/3既有opt-in忽略，逐581输入无漂移。中英文checkpoint指南随后同步已接受A，既有文档runtime合同新输入实际1pass，精PF纠正重放与独审均0；新Source文档摘要和旧23执行摘要通过不变581码桥关联，不伪造新epoch运行。

本Finding仍open，仅保跨仓最终SDK三语Host/CLI消费与精确merged-main CI交付缺口；不因本地23通过冒main已交付或所有remote Store已验证。既有Graph-specific foreign/lateUnknown专例未单独跑的覆盖限制不删除，无source/无receipt仍不承诺自动恢复。

## Graph 两项负例覆盖补证

6563基础上，原Graph live-source fixture新增foreign→matching→localterminal→EOF与matching→terminal→latestUnknown→EOF两具名case，生产Graph/node/store未改。新五控5/0/0、workflow子集93/0/0实际通过并保原源/claim/内存输出与同实例checkpoint断言；详情绑定evidence.workflow-observation-negative-controls。前文“专例未单独跑”继续描述原历史切点，不再代表本补证缺失。

本Finding仍open。新两case只补Memory同实例的Graph观察顺序覆盖，不新增File、跨重启、remoteStore、SDK/CLI或当前完整合并门禁资格，不以本次focused通过关闭整个跨仓交付Finding。

本Graph测试增量的当前六条完整workspace门禁已按冻结的1023 Rust/Cargo输入实际全部0，主3629/0/3、两个nested各1单列；新增两具名case在全套实际执行。此结果补齐此前本测试分支的完整本地门禁资格，不新增运行时行为、状态或恢复保证。本Finding继续open：最终独审/必要性与精main CI、独立SDK/CLI消费者及其它未闭边界仍按各自证据验收。
