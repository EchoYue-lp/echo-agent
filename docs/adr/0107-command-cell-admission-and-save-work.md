# ADR 0107：分离后台准入与执行生命周期，复用同次保存工作

状态：实施中，验收证据单独记录。

## 背景

后台命令已持久 prepared，但产品在启动回执前等待全局 shell 许可，导致外层工具超时后命令仍继续。大检查点保存同时存在同一次操作内重复读取、解析和冻结的问题，不能仅凭 CAS 入口 deadline 错误认定物理 CAS 本身耗时十秒。

## 候选与决策

保留原 `BackgroundCommandManager`、Prepared/Queued/Running 和 TaskTracker。`CommandCellAdmission` 仅控制准备及 Start 交接前的取消与预算；`CommandCellRequest.cancel` 和 accepted_at 起算的绝对 deadline 控制已接纳 cell。`CommandCellResourceAdmission` 在原 Queued runner 中获取产品资源，许可保有至物理结算、产物终态及原观察 lease 释放。框架不定义产品资源上限。

不采用增加 timeout、统一免除工具限时、应用另建执行队列等方案。外层工具限时只对已返回结构化后台 cell 回执保留成功；前台晚到成功不能绕过原 timeout。

保存保留原格式、raw/approved 事实、CAS、owner-first 恢复及可重建索引。一次 owner 扫描直接验证已解码记录；CAS 重用本操作已解析的请求 payload；coordinator 使用 runtime snapshot 自带的 scope revision；投影和待保存 checkpoint 使用同一冻结 messages，Context 锁不跨 I/O。

`PersistenceStage` 只记录阶段、单调耗时及原共享预算，不包含正文。阶段结束不表示物理写成功，提交与恢复仍由原 Store 回执决定。没有持久缓存、新数据库、降低校验或增加保存预算。

## 依据与影响

延续设计已核对的 [Tokio shutdown](https://tokio.rs/tokio/topics/shutdown) 的通知、原任务清理、等待模型，以及 [Semaphore](https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html) 的异步容量与取消模型。本次浏览工具返回 404，不记为新增网络核验成功。

公共 `CommandCellRequest` 新增两个有 Default 的字段；使用完整 struct literal 的消费者应显式给出 admission/resource_admission，或采用 `..Default::default()`。SDK 独立 pin 不随此次应用集成更新。资源提供者不能启动进程或再拥有执行循环。

## 验收

受控覆盖容量满后的取消、prepared 交接撤回、排队回执、退出 0/101、终态观察持有许可、原前台 timeout；保存覆盖真实只读样本的隔离副本、合成多 scope、CAS/退休/损坏索引和 pending 恢复。定向测试、App、main CI 与发布分列，不相互替代。
