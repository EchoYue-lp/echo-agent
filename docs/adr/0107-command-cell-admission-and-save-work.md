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

## 2026-11-08 补充：原生操作资源与父任务分离

后台 cell 的 Queued 准入不能与其调用父任务同时争用同一单槽资源。父任务的逻辑并发由宿主保留；物理原生命令的许可由实际操作取得，并交给原 backend owner。框架不根据产品角色、任意 guard 类型或 available=0 豁免容量，也不增加配额或第二条命令队列。

可选 `NativeExecutionResourceAdmission` 由宿主提供取消／绝对期限下的异步准入，成功返回既有 `InvocationResourceGuard`。ToolManager 向每次 attempt 的有效 ToolContext 注入能力引用，默认 None；取得的 operation guard 只追加到该次 SandboxCommand／direct child，不写回父 context。前台 shell、run_code 与 run_skill_script 的解释器探测／执行在原校验后、dispatch 前使用该接缝。后台仍只在原 Queued runner 通过既有 cell admission 取得一次资源。

准入等待位于原 ToolManager attempt future 内；原 child cancellation、per-attempt timer 与各入口零值规则保持。原命令 timeout 建立 checked 绝对期限，准入消耗同一次预算，不在之后重启 wall-clock 计时，也不让 ResourceLimits 再覆盖剩余预算。

本地 Skill 探测采用异步进程观察；direct script 复用原 Local child task 的 kill／wait／drain，不把 timeout(output) 返回当成回收。该 crate-private direct 接缝不宣称 OS 隔离，不改变公开 LocalSandbox 平台限制或 Skill source executor／最小环境。清理未确认时，原 child task 保有 Child、PGID 和 guard，使用既有 keyed uncertainty fence 保留原资源；观察可报告 Unknown，同一个原 owner 仅在真实 wait 与原组消失后核销同代 fence，不重放或另起 supervisor。

取舍沿用原参考资料及固定 Codex process-owner、Tokio owned permit 模式：资源随实际工作，不随输出观察或父模型调用。App decorator 与全量 executor wrapper 因能力转发漂移、scoped Arc 身份变化、后台重复准入与控制 I/O 误管而不采用。原保存权威、格式、CAS、raw／approved 与预算保持。

验收包含真实父 Verification→background→wait、混合原生入口的单槽控制、迟到准入／取消／原预算、direct 进程组回收与 Unknown 原 owner 恢复。资源计数和模拟错误本身不代替真实物理终结证据。

### 可选合同的后端能力边界

`SandboxExecutor::supports_physical_resource_ownership` 默认 false；声明支持的叶子必须兑现原物理 owner 保有，路由器必须检查最终叶子。新 provider 为本次 `SandboxCommand` 设置 runtime-only `require_physical_resource_ownership`，不进入工具 wire／持久格式。唯一检查 `ensure_physical_resource_ownership(supported)` 的参数只取实际 executor 的能力，或已审核的原 direct owner 实现能力，不从名称、隔离、角色或 guard 数量推定。

Manager 在缓冲／limits／实际 isolation receipt 的共同分派点及独立 stream 分派点，检查实际 select（含 fallback）返回的原 Arc。stream 检查在原 spawn 前返回 typed Unavailable；原 availability 控制探测不因此受阻。“零创建”指 workload／容器／Pod 创建。Docker／K8s 的公开执行入口也在各自共同 workload 入口、create 前复用同一检查；本次不修其旧清理生命周期，不授其新合同资格。

后台使用同一个 native→CommandCellResourceAdmission 薄转换，在原 Queued 获取一次资源。原 lease 的要求标志默认 false，新转换才明确要求 physical ownership；原 runner 将要求传给原命令。没有资格的旧无 sandbox direct runner 在新要求下保留接纳回执并记录明确 LaunchFailed，不以另一条 direct owner 的 Unix 证明代替它。

generic 未实际使用新 provider／require=false 保留原行为。有效 Some 政策在未证明的平台／direct／custom backend 会新增 Unsupported，这属于明确的新合同边界，不是所有平台行为不变。当前 Unix direct 复用已补齐的原 process-group owner；Windows 等未证明平台不假声明。显式登记 local(None) 来源继续保留；不得悄悄换成 host default。嵌入方迁移自定义执行器，须同时提供 trait 声明与原 guard／Unknown／真实终结控制，不可只填 true。测试与 live 资格分别记录。

### 正常 root 退出与 owned group 的区别

父进程 exit／stdout-stderr EOF 不能证明原进程组已空，例如 `sh -c 'sleep 60 >/dev/null 2>&1 &'`。请求新合同的 Local/direct 原 owner 在 root 状态和管道读取结束后继续观察捕获的原 PGID；正常结束、取消和原 deadline 都沿同一个 owner／同一预算结算，不能在 root 退出后另开计时器。仅实际组结束才发布完成，清理未知保持原 child 与 guard。
