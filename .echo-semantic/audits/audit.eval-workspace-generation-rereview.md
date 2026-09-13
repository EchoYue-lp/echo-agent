---
schema_version: 1
id: audit.eval-workspace-generation-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: failure_concurrency
freshness: examined
revision: 29a00f66843263f27503f83247ee8a770b89e913
finding_refs: [finding.eval-workspace-generation-isolation]
challenges:
  per-run-generation-isolation:
    revision: 29a00f66843263f27503f83247ee8a770b89e913
    source_refs: [src/eval/runner.rs]
    evidence_refs: [evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
  settled-and-unsettled-cleanup:
    revision: 29a00f66843263f27503f83247ee8a770b89e913
    source_refs: [src/eval/runner.rs]
    evidence_refs: [evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
  improve-consumer-convergence:
    revision: 29a00f66843263f27503f83247ee8a770b89e913
    source_refs: [src/improve/loop.rs, src/eval/comparator.rs, docs/adr/0036-eval-workspace-generation-lifecycle.md]
    evidence_refs: [evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
---

# Eval workspace generation 独立复审

## 审查范围

复审同ID与无fixture workspace隔离、fixture setup、success/typed error cleanup、timeout/caller-drop retain、cleanup error投影、Improve early-stop并发与Comparator consumer。

## 已检查故障假设

验证两个run是否仍共享cwd或互删fixture，settled error是否遗留generation，timeout/caller-drop是否误删，early-stop是否绕过cleanup，以及cleanup failure是否错误覆盖已有criteria score。

## 实际实现路径与证据

每次run在父目录下创建唯一`eval-` TempDir，fixture与无fixture都只使用该generation。Guard默认禁用Drop cleanup；success、typed Agent error和setup failure显式close，timeout keep并记录路径，真实run future被abort后Drop保留Agent已观察到的cwd。Cleanup failure使success=false并增加violation，但0.75 criteria score保持。

同IDfixture与无fixture旧实现red均exit 101；当前Runner 9 tests覆盖隔离与所有cleanup disposition。两个并发ImprovementLoop在iteration 0 early-stop，各自只调用一次Agent、cwd不同且完成后目录不存在。Improve/Comparator不再构造固定或无人拥有的parent；双语文档与ADR 0036保持同一合同。

## 问题记录

独立review两轮后无blocker；`finding.eval-workspace-generation-isolation`具备repair、verification与rereview证据，可标记resolved。Issue #50保持open，等待本地提交进入远端main后关闭。`finding.eval-timeout-settlement`与Issue #48保持open。

## 残余风险

Timeout下真实detached producer仍未证明terminal，retained目录需要Issue #48后续回收；真实跨平台cleanup failure尚未用文件系统故障注入验证。

## 未检查项

未执行长时间并发stress、完整workspace合并门禁、远端CI、website后续文档投影或其它77个open Finding。
