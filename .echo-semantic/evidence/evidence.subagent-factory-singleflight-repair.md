---
schema_version: 1
id: evidence.subagent-factory-singleflight-repair
kind: evidence
observed_at: 6d66479fd520da9cbbb66723faa35ce69a8963a8
source_refs:
  - src/agent/subagent/registry.rs
  - docs/adr/0033-subagent-factory-singleflight-publication.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - factory在旧registration被替换前已产生的外部副作用不由Registry自动补偿
  - factory不得递归resolve自己的registration，调用方仍须为其业务场景设置deadline或cancellation
---

# Subagent factory single-flight 修复证据

## 支持的结论

基准`78b9f06b4320531fd8f41260887cd69c1343e995`使用registry-wide `instantiating`集合、50ms polling和waiter-only 30秒timeout。确定性red证明初始化future被abort后名称永久残留；另一个test-only publication boundary证明成功路径删标记后、写agent前可启动同revision第二次factory。

当前实现把cached Agent放入每个`RegistryEntry`独有的Tokio `OnceCell`。`get_or_try_init`在同一个cell内串行构造并原子发布成功值；取消、错误或panic不初始化cell，后续等待者可重试。完成后同时比较registration revision与cell identity，旧代结果不得进入新entry。

## 来源与范围

`src/agent/subagent/registry.rs`包含所有异步/同步注册入口、lazy resolve、remove、list和测试；ADR 0033记录Tokio官方合同、候选方案、caller-owned deadline、同名递归限制、failure retry与rollback。

## 已知缺口

本修复不改变`create_fresh_agent`、SubagentExecutor attempt lifecycle、TaskClaim关联或definition catalog。Factory自身若在返回Agent前产生不可回滚副作用，仍需factory实现负责清理。
