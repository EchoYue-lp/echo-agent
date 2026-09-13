---
schema_version: 1
id: evidence.background-task-terminal-authority-repair
kind: evidence
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
source_refs:
  - echo-orchestration/src/tasks/background_task.rs
  - docs/en/29-long-running-tasks.md
  - docs/zh/29-long-running-tasks.md
  - docs/adr/0039-background-task-terminal-authority.md
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/public-api.txt
  - echo-sdk-protocol/tests/facade_inventory.rs
  - scripts/check-language-sdks.sh
supports: [behavior.task-subagent-execution, rule.task-subagent-authority, behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - BackgroundTask仍是process-local且Result T只由一个waiter消费
  - 本修复不增加TaskSpawner shutdown、持久恢复或Task DAG关系
---

# BackgroundTask terminal authority 修复证据

## 支持的结论

基准`81e2756cee9127fa23a9bb1023bd56aa8f954964`把一个task拆为Tokio RwLock status、Mutex result、Notify、terminal bool和未监督user future。三个有效red分别证明双waiter中一个永久阻塞、queued cancellation不结算、`max_concurrent=0`永久Pending。

当前`BackgroundTaskHandleState<T>`在一个短同步mutex内原子保存status、单消费者result与typed panic provenance；type-erased list/prune读取同一state，terminal bool和合成fallback退出。Handle实现无`T: Clone`约束的Clone；wait先enable Notified再检查state，使用call-local absolute deadline；首个waiter取得T，后续waiter立即获得永久terminal反馈。`is_panicked()`不解析错误文案，只读取JoinError分类后与terminal同步发布的布尔出处。

TaskSpawner从spawn接纳时计算absolute deadline，cancel/deadline覆盖admission和execution。User future在受监督child execution task中运行；cancel/timeout先abort并await child，再发布terminal；JoinError panic归约为Failed。零并发直接Failed，不静默提升容量。

## 来源与范围

修复只改变process-local BackgroundTask/TaskSpawner机制，不进入revisioned Task graph。ADR0039记录Tokio Notify/JoinHandle与Codex background request依据、result/observer合同、优先级、兼容和回滚。新增Clone public identity由SDK生成器分类为Rust language_intrinsic。

## 已知缺口

本Evidence不证明跨runtime shutdown或进程退出后的terminal，也不让多个waiter复制任意T。CommandCell、BackgroundReview、scheduler和durable Task由独立Finding继续跟踪。
