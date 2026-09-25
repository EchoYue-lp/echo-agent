---
schema_version: 1
id: audit.tool-pipeline-example-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: contract_evidence
freshness: examined
revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
finding_refs: [finding.tool-pipeline-example-drift]
challenges:
  production-stage-source:
    revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
    source_refs: [src/agent/react/run/pipeline.rs, echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs]
    evidence_refs: [evidence.tool-pipeline-example-repair, evidence.tool-pipeline-example-verification]
  pre-execution-order:
    revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
    source_refs: [src/agent/react/run/pipeline.rs, echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs]
    evidence_refs: [evidence.tool-pipeline-example-verification]
---

# demo64 Tool pipeline executable contract independent rereview

## 审查范围

独立 reviewer 对 `origin/main@9abdf9de` 集成后的 #101 候选复审，
并对随后合入 `origin/main@93ee33d9` 的快照增量复审。
锚点为任务分支 HEAD `3a09a8bd13fa2a3a56bf8c8e5db9f5a879284247`，
加审查时未提交 `git diff` 的 SHA-256
`7a0ff047dfb32a500b4b0a88fe5484aa6058e7595280d2e15355bf1c8061d5de`。
审查覆盖生产 Pipeline 注册、真实工具调用的 tracing 采集、demo64 合同、
双语工具文档及语义修复和验证证据；无绑定 Plan。

## 已检查故障假设

- demo64 可能仍展示自己维护的阶段顺序，而不是生产运行顺序；新增或重复阶段
  也可能被静态展示或首次位置查找掩盖。
- 仅要求 17 个阶段齐全仍可能让可见性、计划模式先于干预，权限先于
  PreToolUse，或先读后改先于权限，改变调用重写和权限判定语义。
- 执行后的 OutputGuard、输出预算、Trace、Audit 与终态回调可能被文档写成
  与生产相反的顺序。

## 实际实现路径与证据

demo64 的 `StageTraceLayer` 消费 `ToolExecutionPipeline::run` 实际发出的
`stage.name()` 事件，按观测顺序展示；说明目录仅按名称索引，不持有第二份
生产顺序。合同要求每个已说明阶段恰好出现一次、无未知阶段，并检查
`intervention < tool_visibility < plan_mode < pre_tool_use_hook < permission
< read_before_edit`，再检查最终输入守卫、canonical invocation、执行和
结算观察的关键部分序。基于真实阶段采集的七组坏序交换和重复阶段回归均
被拒绝；最新定向 demo64 测试 4/4、Clippy 零警告、格式与 diff 检查通过。

## 问题记录

首次集成复审发现 Important 阻断：旧部分序允许上述执行前重排。实施方先
取得坏序回归 exit 101 红证据，随后补充关键依赖，4/4 转绿。最终独立
复审返回 PASS，0 findings。此审计仅记录 reviewed 快照；随后新增本审计、
Finding 引用及证据状态更正属于非源码增量。
在 `f55856125b08e3c9fc2e06545bb66d707b8c6eba` 上的最终整合增量复审
再次返回 PASS、0 findings，确认 #53 内容保留且 #101 合同未变。

## 残余风险

回归覆盖默认管线的一次成功工具调用，不证明自定义管线或每个执行前
阻断分支的行为。整合 `origin/main@93ee33d9` 后的完整 workspace 门禁
已通过；远端 CI、PR 与 main 交付尚未完成，不能仅凭复审 PASS 关闭 Issue #101。

## 未检查项

独立 reviewer 未替代完整 workspace 门禁或远端 CI。最终整合快照的
文档合同通过 15/15，完整门禁日志确认 86 条测试汇总均为 0 failed。
