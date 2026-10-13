---
schema_version: 1
id: evidence.process-group-kill-delimiter
kind: evidence
observed_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
source_refs: [echo-orchestration/src/tasks/command_cell.rs, echo-tools/src/shell.rs, docs/en/02-tools.md, docs/zh/02-tools.md]
supports: [map.tool-permission-sandbox, rule.permission-effect-order]
limitations: [Historical runner cancellation causality is unproven; full final and remote checks recorded at delivery]
---

# Exact process-group cleanup

## 支持的结论

The Ubuntu procps-ng 4.0.4 syscall-injection probe in run 37962255127, job
113929766582 observed `kill(-2, SIGKILL)` for original arguments targeting -2443.
The `--` delimiter produced `kill(-2443, SIGKILL)`. Both signals were intercepted
with EPERM; this establishes the parser defect without delivering signals.

CommandCell and direct-shell cleanup now both delimit negative group IDs.
Existing timeout/cancellation/reap owners remain unchanged. No public API,
permission gate, persistence authority or dependency is added.

## 来源与范围

The production-command regression failed before the delimiter (group-kill-before
log), asserting the positional target contract without sending a signal. Final
argv and live cancellation tests, complete workspace checks and independent
review are recorded at delivery.

After the delimiter repair, the two affected all-feature library suites passed:
436 orchestration tests and 203 tool tests (two existing ignored tool tests).
Independent review returned pass with zero findings for both cleanup paths.
The complete workspace gate and Linux/Windows CI are required before merge.

## 已知缺口

Application run 38018370296 was cancelled and its Linux job log is unavailable
(BlobNotFound). The parser defect is established independently; this evidence
does not claim it was the exact cause of that historical cancellation.

历史快照定位说明：原 `source:cd5860e15353dbaf0c833ba1c224671c8b990e521a70f884e40ff2760884d5d0` 的非提交引用在本次整体源码变化后不可恢复。逐条核对该对象实际源码引用与 `95ec66c6083a8feb17a1c28de0d01e1a24ca8a28` 的 blob 完全相同，现仅将历史观察定位到该可恢复 Git revision；未提升状态或声称本轮重新审查该边界。
