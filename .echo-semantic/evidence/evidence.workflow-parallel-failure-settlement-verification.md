---
schema_version: 1
id: evidence.workflow-parallel-failure-settlement-verification
kind: evidence
observed_at: 8c8fdffc4a2dfb341d9b25e32a321c9fb41c7350
source_refs:
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/dag.rs
  - echo-orchestration/src/workflow/concurrent.rs
supports: [behavior.task-subagent-execution]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 测试使用in-process test Agent，不模拟外部系统不可取消的副作用
---

# Workflow sibling failure结算验证证据

## 支持的结论

59项Workflow定向测试全部通过。反例覆盖一个分支快速失败、另一个分支挂起60秒时在1秒内
返回错误并drain；streaming Graph先发`NodeError`再发terminal error。新增成功反例强制后
注册节点先完成，并验证Concurrent merge/steps与DAG steps/final leaf结果仍保持原顺序。

## 来源与范围

独立review先确认原始failure settlement闭合并发现成功顺序回归；修正后三文件增量再次
review为pass，Critical、Important、Minor均为0。Formatter和diff check通过。

## 已知缺口

不可中止的第三方effect仍需其Tool/Subagent owner提供自身cleanup receipt；Workflow只能保证
其持有的task handle完成取消与drain。
