---
schema_version: 1
id: audit.tool-read-cache-authority-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: failure_concurrency
freshness: examined
revision: 745a3f87fd51019aa3a96988e995dfd24bd1ff2f
finding_refs: [finding.tool-read-cache-scope, finding.tool-read-cache-inflight-invalidation-race, finding.tool-registry-mutation-active-call-deadlock]
challenges:
  context-and-artifact-scope:
    revision: 745a3f87fd51019aa3a96988e995dfd24bd1ff2f
    source_refs: [echo-execution/src/tools.rs, echo-core/src/tools/artifact.rs, echo-tools/src/files/artifact.rs]
    evidence_refs: [evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification]
  write-and-registration-generation:
    revision: 745a3f87fd51019aa3a96988e995dfd24bd1ff2f
    source_refs: [echo-execution/src/tools.rs]
    evidence_refs: [evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification]
  omitted-context-consumers:
    revision: 745a3f87fd51019aa3a96988e995dfd24bd1ff2f
    source_refs: [echo-core/src/tools/mod.rs, echo-execution/src/tools.rs, echo-execution/src/skills/external/activate_tool.rs, echo-tools/src/code.rs, src/tools/builtin/agent_dispatch.rs, src/tools/builtin/subagent_message.rs]
    evidence_refs: [evidence.tool-read-cache-authority-verification]
---

# Tool read cache authority 独立复审

## 审查范围

复审ToolManager stream/non-stream read result cache的workspace/invocation/artifact scope、Write overlap、Tool registry generation、测试独立性和当前ReadOnly context consumer。

## 已检查故障假设

验证artifact policy是否仍可跨scope命中，relative root解析是否偏离真实writer，registration replacement是否允许旧实现回存，workspace与lineage测试是否互相掩盖，以及未指纹化context是否影响当前内建ReadOnly结果。

## 实际实现路径与证据

Key独立覆盖working directory、conversation/run/turn/message/execution和完整artifact policy；artifact相对root按真实consumer相同的process cwd解析。Read在cache write lock内用observed epoch条件发布，所有非Read effect在进入和Drop时失效。调用在取得Tool guard前观察epoch；当前DashMap Ref又持续到旧结果发布后，replacement随后swap并clear。31个ToolManager测试分别覆盖workspace、lineage、artifact relative/absolute冲突、completed/in-flight replacement与Read/Write交错；Clippy、crate check、strict snapshot和SDK零diff均通过。

## 问题记录

独立review三轮后无目标blocker；`finding.tool-read-cache-scope`与`finding.tool-read-cache-inflight-invalidation-race`具备repair、verification与rereview证据，可标记resolved。复审另确认跨await持有DashMap Ref会使同步registry mutation等待，并形成独立`finding.tool-registry-mutation-active-call-deadlock`与Issue #115，保持open。

## 残余风险

第三方ReadOnly Tool仍可能依赖未指纹化context；AtomicU64存在理论2^64 wrap；replacement测试的started信号位于实际replace调用前，最终结论同时依赖源码guard生命周期和epoch-before-get，而不单靠该信号。

## 未检查项

未执行真实domain Tool、长时间stress、完整workspace合并门禁、远端CI或其它open Finding。
