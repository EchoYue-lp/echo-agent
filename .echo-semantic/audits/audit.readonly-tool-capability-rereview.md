---
schema_version: 1
id: audit.readonly-tool-capability-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
finding_refs: [finding.readonly-tools-custom-registration-bypass]
challenges:
  custom-registration-bypass:
    revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
    source_refs: [src/agent/react/builder.rs, src/agent/react/mod.rs, src/agent/react/capabilities.rs]
    evidence_refs: [evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification]
  late-injection-visibility-execution:
    revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/pipeline.rs]
    evidence_refs: [evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification]
  observation-side-effects:
    revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
    source_refs: [echo-orchestration/src/tasks/task_tools.rs, src/tools/builtin/cell_tools.rs, src/tools/builtin/subagent_message.rs]
    evidence_refs: [evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification]
---

# Read-only Tool capability 独立复审

## 审查范围

独立 reviewer 复核 `#60/#81` 最终源码、ADR、双语文档、focused 结果与
Tool capability 边界；本对象只关闭 `#81` 的 custom registration 绕过反例。

## 已检查故障假设

检查构造期 custom Write/Execute 注册、后续 Agent API/trait 注册、直接
ToolManager 注入后的 LLM 可见性与执行，以及观察工具是否因 capability
错配而被隐藏或将持久写入错误标为只读。

## 实际实现路径与证据

Builder 与 Agent 注册、snapshot LLM view 和 PlanModeStage 都读取工具自身
capability。focused 测试覆盖构造、后续注入与执行拒绝；task/cell/subagent
list 的 read-only 合同保持可用，memory recall telemetry 写入工具保持 mutating。

## 问题记录

独立只读 reviewer 对当前源码和提供的 focused 结果返回 pass、0 action items；
reviewer 未自行运行测试。`#81` 已具备 repair、verification 与 rereview 证据，
仅表示当前任务分支 Finding resolved。

## 残余风险

完整 workspace 门禁与 17 项独立 feature 检查已通过；PR/CI 与远端 main 交付
尚待完成。第三方 Tool 的
capability 声明正确性仍由 Tool 作者负责。

## 未检查项

未执行第三方自定义工具生态抽样、远端 CI 或生产长时间运行。
