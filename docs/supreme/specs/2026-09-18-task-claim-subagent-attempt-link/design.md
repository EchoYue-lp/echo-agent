---
title: TaskClaim 与 SubagentAttempt 单一身份链路设计
artifact: design
carrier: markdown
---

# TaskClaim 与 SubagentAttempt 单一身份链路设计

## 问题与目标

`RuntimeDagExecutor`已经持久化`TaskClaim`，但Team controller和SDK controller把claim丢弃，
随后使用普通Subagent dispatch。真实Subagent execution因此获得随机execution ID，event lineage中的
`task_id`、`attempt`、`plan_revision`为空。精确interrupt可能找不到尚未admit的attempt，或因所有并行
task共享TaskRun根CancellationToken而取消整个run。

目标是让一条权威身份链从`TaskRun -> TaskClaim -> SubagentAttempt`无损贯通，使claim CAS、事件、
控制命令、取消、重试和恢复都指向同一physical attempt；不新增task store、第二执行器或应用专属状态机。

## 目标行为

1. `TaskClaim`是attempt identity的唯一来源。framework按
   `claim.execution_id(run_id, task_id)`生成execution ID，并同时携带run、task、revision、attempt。
2. `RuntimeDagExecutor`在claim成功后构造完整`TaskSubagentContext`；controller只能消费该context，
   不能重新生成execution ID、attempt或revision。
3. 每个claimed task使用TaskRun cancellation的child token。取消TaskRun会传播到全部child；取消一个
   exact attempt只取消该child，不影响sibling task。
4. Team dispatch使用结构化`TeamDispatchRequest`和显式`TeamDispatchController`，调用
   `dispatch_attempt`而不是普通`dispatch`。`DispatchStarted`、tool/stream事件、terminal和artifact
   都保留同一lineage；TeamAgent通过同一controller暴露exact live control。
5. claim成功后、等待shared admission/semaphore之前，controller必须把exact identity与同一个child token
   注册为process-local reservation。exact interrupt可立即取消reserved或active attempt；若命令早于
   reservation，则原子登记pending intent，由reservation消费。
6. pending interrupt是live-control projection，不是durable command权威。SDK/应用若承诺跨进程恢复，
   必须持久化`SubagentCommandIdentity`并在恢复时重放；framework registry只负责当前进程的原子交接。
7. stale claim、错误attempt、错误execution ID、重复admission和冲突pending intent均fail closed。

## 范围与非目标

范围：

- framework `RuntimeDagExecutor`到`RuntimeDagController`的exact context；
- Team默认dispatch与React Team dispatch；
- `RuntimeTaskService`的durable-aware exact control与`SubagentControlRegistry`的
  pending/reserved/active/settled投影；
- TaskRun根取消与task child取消的传播关系；
- event/runtime context lineage与framework文档、测试和语义证据；
- 独立SDK仓库的后续thin adapter、framework pin、inventory分类与E2E。

非目标：

- 不新增Task/Plan/Subagent store、ready frontier、retry loop或第二DAG validator；
- 不改变TaskClaim CAS、retry attempt编号或TaskRun terminal规则；
- 不把EKO reviewer、worktree、文件权威、UI投影或approval策略放进framework；
- 不在framework中持久化SDK command ledger；
- 不顺带处理A2A；
- 不要求process-local helper形成三语言facade。

## 系统边界

### Framework

`echo-orchestration`继续拥有TaskRun graph、claim、wave、retry和settlement。`echo_agent::subagent`
继续拥有真实Subagent execution、event与live control。两者通过一个完整`TaskSubagentContext`相接，
没有反向依赖或重复身份算法。

### Team Adapter

Team只把task extension/dependency output编译为prompt，并把完整context交给
`TeamDispatchController`。默认和React adapter都调用同一个`dispatch_attempt`入口，并从该controller
取得同一个control registry；adapter不得再创建`team-member-*`随机身份。自定义dispatch必须实现
同一trait，不能只提供一个无法控制的裸closure。

执行期由`TeamRuntimeHandle`同时持有stable `run_id`、同一个`Arc<TeamRuntimeController>`，以及基于
该controller的`Arc<RuntimeTaskService<_>>`。它只引用既有Task store与control authority，不拥有
新状态机。`TeamAgent`在首次执行前确定run ID并保留handle；caller-supplied runtime直接构造同源handle；
React Team由`SubagentExecutor`按run ID登记active handle，供父级control转发。现有one-shot convenience
入口可以内部创建handle，但不宣称提供并发control；需要控制的调用方必须使用返回/持有handle的入口。
active执行结束后handle按有界retention保留以回答late stale/settled查询，淘汰后command replay仍可用
同一个controller/store重建handle并先做durable claim precondition。

### SDK Adapter

独立`echo-agent-sdk`仓库把Host持久化的claim/command转换到framework exact context和控制API。
Host负责command durable replay与wire receipt，framework `RuntimeTaskService`负责current-claim校验并将
控制投影到live reservation/attempt。SDK阶段必须基于已合入的framework SHA，不能在两个仓库各实现
一套attempt identity，也不能绕过TaskClaim precondition直接调用process registry。

## 核心结构与数据流

### Exact Task Context

`TaskSubagentContext`表达一个已claim的physical attempt，私有保存exact `TaskClaim`与以下字段：

- `run_id`
- `task_id`
- `execution_id`
- `plan_revision`
- `attempt`
- task-scoped `cancel`
- delegation policy与waived dependencies

framework只在`RuntimeDagExecutor`内部通过`TaskClaim + run_id + task_id + child token`构造context。
identity、claim和cancel字段保持private，只提供只读accessor；delegation/waived dependency同样通过
受控builder形成。现有public `new`删除，不能构造unbound task context。`child_delegation_context`
保留同一claim/run/task/execution/revision/attempt，只派生child cancellation和delegation policy。

`RuntimeDagController::dispatch_task`只接收`TaskSubagentContext`与`Task`，不再接收第二份独立claim。
executor内部保留原claim用于`resolve_dispatch`、CAS settlement和abandonment；controller如需claim只读取
`context.claim()`。这样adapter无法收到两组可冲突事实。

### Team Dispatch Contract

原`TeamDispatchFn`改为`TeamDispatchController` public trait：

- `dispatch(TeamDispatchRequest { member, prompt, context })`执行exact attempt；
- `reserve_attempt(context)`在claim后、执行许可前注册exact child token；
- crate内部`request_live_interrupt(identity)`投影已经通过Task authority验证的请求；
- `retire_attempt_control(identity)`在durable claim CAS之后清理reservation/pending/settled projection。
  若identity仍active，它先取消child并等待canonical terminal，不能直接删除binding。

这是有意的Rust公共合同迁移；所有in-tree closure、示例、doctest和consumer contract必须同步，
不保留旧三参数closure adapter。默认Team controller与React controller都持有同一个
`SubagentExecutor`/control registry；`TeamAgent`将exact control转发给该controller。自定义controller
必须实现完整trait，不能声明支持exact control却只执行dispatch。

### Live Interrupt Admission

`SubagentControlRegistry`在同一mutex下维护bounded pending interrupt、reserved、active与settled：

1. pending早于reservation：登记exact identity；
2. reservation：claim已被runtime接纳并绑定child token，但尚未获得执行许可；
3. active：`dispatch_attempt`把同一reservation推进到Starting/Running，不重复admit；
4. settled：保留bounded terminal projection供同进程幂等查询；
5. 同task/attempt但不同execution ID：identity conflict；
6. pending容量达到上限：显式拒绝，durable caller保留命令并稍后重试；不静默evict取消意图。

`reserve_attempt`原子消费pending intent并在注册reservation时取消传入child token。
`RuntimeDagExecutor`在child已取消时绕过shared admission/semaphore，但仍调用canonical Team dispatch，
使`dispatch_attempt`升级同一reservation并通过现有pre-cancel path发出Started/Cancelled；它不得取得
昂贵执行许可或触发Agent/model/tool side effect。正常路径取得许可后也只升级同一reservation。

公开control入口位于Task authority：

- `RuntimeTaskService::request_attempt_interrupt(run_id, task_id, claim)`先调用
  `claim_is_current`，再投影live interrupt，随后再次检查claim；第一次检查失败时绝不写pending，
  第二次检查发现竞态时精确retire刚写入的projection并返回`StaleOrSettledClaim`。
- 成功返回`RuntimeTaskAttemptInterruptReceipt`，回显run/task/claim/execution/revision/attempt，
  disposition为`QueuedBeforeReservation`、`ReservedRequested`、`ActiveRequested`、
  `ActiveAlreadyRequested`或`AlreadySettled { status }`。
- `StaleOrSettledClaim`是typed outcome而非live-registry猜测。process settled tombstone被淘汰或进程重启后，
  旧durable command仍先经TaskClaim CAS，因此不能变成未来pending。
- `PendingCapacityExceeded { limit }`和
  `IdentityConflict { task_id, attempt, expected_execution_id, actual_execution_id }`是typed error。
- 现有`interrupt_subagent(execution_id, attempt)`保留为active-only、wait-until-settled兼容入口，
  TaskRuntime和SDK不得用它填补pre-admission窗口。

live lifecycle是
`Pending -> Reserved -> Active -> Settled`，也允许`Pending/Reserved -> RetiredAfterExactClaimSettlement`。
当durable claim已非current而live attempt仍active时，转换为
`Active -> InterruptRequested -> Settled -> RetiredAfterExactClaimSettlement`：registry在同一mutex下
标记并取消child，保留agent binding、settled watch与control入口直到真实Agent/tool effect结算；
不允许删除active后放任旧execution继续运行。
任何durable settlement/abandonment error或unknown outcome都保留projection供lookup/retry；禁止TTL、
无条件eviction或reclaim时沿用旧intent。

`RuntimeDagController`新增三个有界hook：`reserve_attempt_control`、`request_live_interrupt`与
`retire_attempt_control`。前两个可返回typed error并发生在durable commit前；retirement只在
`settle_resolution`/`abandon_claim`返回durable `Settled`或`Superseded`后由executor独立调用，不能藏在
CAS方法内部。

post-CAS retirement返回`RuntimeAttemptControlCleanupReceipt`：`Retired`、`NotFound`、
`AlreadyConsumed`或`RetryableFailure { error }`。该receipt通过稳定tracing target与可选
`RuntimeAttemptControlObserver`报告；`RetryableFailure`绝不改变已提交`RuntimeTaskResolution`、不触发
abandonment或第二次CAS。内置registry对poisoned mutex恢复后重试清理，observer只报告仍未清理的投影。

`RuntimeTaskService::reconcile_attempt_control(run_id)`从durable snapshot派生当前claim execution ID集合，
并调用controller hook处理所有不再current的projection：pending、reserved与settled可精确retire；
active必须先cancel-and-drain直到canonical terminal，不能提前释放binding/watch。它在execute/resume
接纳前、revision reload safe point、command replay和public interrupt的stale precheck上运行。
active不配合取消时由下述同一个`RuntimeDagExecutor` wave supervisor执行定向grace/forced-abort；
cleanup failure沿用上述receipt/observer，不改变durable task状态。

### Cancellation Tree

TaskRun token只作为parent。每个claim在进入shared admission/semaphore之前获得child token，将同一
child放入exact context并通过controller reservation注册。shared admission与semaphore两条等待分支都
select child cancellation；取消获胜时跳过许可并走pre-cancel dispatch。run cancel取消所有child；
exact interrupt只取消目标child。retry/reclaim产生新TaskClaim、execution ID和新child，不复用已取消token。

`SubagentAttemptAdmission`的异常Drop按token事实结算：child已取消则live terminal为Cancelled，
未取消的异常drop才是Failed。TaskRuntime的Paused表示task可恢复，但本次physical Subagent execution
仍是Cancelled；两者是不同层级，不互相改写。

### Exact Attempt Supervisor

唯一`RuntimeDagExecutor`继续拥有整个wave的`JoinSet`。每个claim spawn时保存由同一JoinSet返回的
`AbortHandle`、exact execution ID与settled通知到一个有界的process-local supervisor index；index只
追踪已经由TaskClaim接纳的future，不维护ready frontier、task status或第二terminal。registry仍只拥有
control projection，不接触JoinSet/AbortHandle。

exact interrupt或durable supersede取消child后，supervisor给目标attempt启动
`RuntimeTaskServiceConfig.cancellation_grace_period`。若该attempt在grace内join，按真实结果结算；
否则只调用目标AbortHandle（不调用`abort_all`），继续由原JoinSet drain该join，然后让
`SubagentAttemptAdmission::Drop`以已取消token投影Cancelled。sibling handle/token均不受影响。
registry binding/watch与supervisor entry只在join/abort已经结算且durable claim CAS/reconciliation完成后释放。
run-root cancellation仍复用现有wave级grace/`abort_all`；二者并发时terminal只由canonical join和
TaskClaim CAS产生一次，exact timer不另行提交terminal。

## 异常和边界场景

- claim后、reservation前interrupt：Task service的current-claim precondition通过后写pending；reservation
  消费并取消child，零Agent side effect，claim按Cancelled结算。
- interrupt与reservation/admit并发：同一registry mutex给出唯一顺序；结果只能是pending后消费、
  reserved后取消或active后取消。
- reserved child等待shared admission/semaphore时被interrupt：等待立即结束，不占用许可，仍走同一
  execution ID的pre-cancel Started/Cancelled路径。
- interrupt与settlement并发：返回真实terminal或等待同一settled watch，不合成第二terminal。
- claim在admission前因validation/shared admission/controller错误被settle或abandon：durable CAS成功或
  superseded后精确retire live projection；CAS outcome unknown时保留。
- run cancel与exact interrupt并发：child token幂等取消；TaskRun settlement仍由runtime authority决定。
- run cancellation grace到期并force-abort：已取消admission Drop结算Cancelled，不得误报Failed；Task
  可按产品策略持久化Cancelled或Paused。
- sibling并行：目标attempt取消不改变sibling child token，wave继续收集其结果。
- retry：新claim identity不得匹配旧pending/settled attempt；旧命令不会自动作用于新attempt。
- physical reclaim：新claim即使revision/attempt相同也有新claim/execution ID；旧pending只在旧claim
  被durable CAS证明不再current后retire，绝不迁移到新claim。
- settled tombstone淘汰/进程重启后的旧command：公开Task service在写live pending前以exact TaskClaim
  precondition返回StaleOrSettledClaim，并对同一exact identity执行reconciliation retirement。
- durable settle响应丢失/unknown：不得立即abandon。executor先reload或调用`claim_is_current`；若exact
  claim已非current，视为durable authority已经前进、retire projection并reload graph，不做第二次CAS；
  若claim仍current才允许走既有abandon policy；若lookup本身失败则保留projection，由下一次run recovery/
  command replay reconciliation处理。
- durable CAS成功、live retirement失败：真实Task terminal保持已提交；独立cleanup receipt/observer
  报告retryable projection debt，不返回wave failure、不abandon、不做第二次CAS。
- 外部CAS supersede正在运行的attempt：reconciliation取消该child，保留live control/watch直到
  canonical terminal或定向grace后force-abort+join结算；不能只删registry entry后放任旧execution继续运行。
- 不配合取消的exact target：唯一wave supervisor定向abort该handle、等待join并settle；sibling
  正常完成。已经提交给外部系统的in-flight effect仍需由原effect合同判断，不能声称token能撤回。
- crash：process-local pending intent会丢失；durable Host必须依据command ledger重放，framework不得声称恢复完成。
- stale graph revision/spec hash：TaskClaim CAS继续拒绝，live registry不能覆盖持久化结果。
- adapter传入不一致lineage：dispatch前拒绝，不回退随机identity。
- public callback迁移：编译失败作为显式迁移信号，不保留旧三参数callback与新request的双实现。

## 关键取舍

### 选择claim-derived context，而不是adapter自行拼字段

ADR 0008已经确认claim是physical lease。让每个adapter拼execution ID会重建本Finding；因此映射只在
runtime executor完成一次，controller只能传递。

### 选择child cancellation，而不是共享根token

共享根token无法表达exact task interrupt。child token同时保留parent传播与sibling隔离，是Tokio原生
能力，无需新增取消状态机。OpenAI Codex也把running task持有的CancellationToken继续派生给turn，
并在interrupt时取消active task，而不是用无身份的全局boolean。

### 选择pending live intent，而不是轮询admission

sleep/poll无法闭合claim与registry admission窗口。pending intent与active admission在同一mutex中原子
交接，复用现有queued guidance模式，但取消intent有独立bounded map且不静默丢弃。

### 选择claim后的live reservation，而不是等到dispatch

只在`dispatch_attempt`注册会让child在shared admission/semaphore等待期不可寻址。reservation紧跟claim，
绑定同一child token但不拥有durable status；TaskClaim CAS仍是能否登记/保留该projection的唯一依据。

### 不把pending intent当durable authority

process registry无法跨crash。Kubernetes resourceVersion/If-Match式precondition和ADR 0008的claim CAS
继续保护持久化状态；公开interrupt必须经`RuntimeTaskService`前后两次current-claim检查。SDK command
ledger负责恢复重放，framework live registry只执行当前进程控制。

### 选择post-CAS诊断，而不是反转terminal

live registry只是projection。durable CAS成功是commit point；其后cleanup failure必须以独立receipt/
observer可见，但不能把已提交terminal改成wave failure或触发abandonment。

### 选择同源TeamRuntimeHandle，而不是裸control registry

current-claim检查需要同一个Task store，仅有control registry无法判断durable stale。handle把run ID、
controller/store与RuntimeTaskService绑定为执行期能力；TeamAgent、React Team和外部runtime只传递该引用，
不复制claim或terminal状态。

## 业界依据

- [Cursor Plan Mode](https://cursor.com/docs/agent/plan-mode)与
  [Cursor Subagents](https://cursor.com/docs/subagents)：plan artifact与执行角色分离，支持单一执行图权威。
- [OpenAI Codex active turn state](https://github.com/openai/codex/blob/3d3ae4965ab370217e871b3a7f0d15589557ee4b/codex-rs/core/src/state/turn.rs)：running task显式持有CancellationToken与完成通知。
- [OpenAI Codex regular task](https://github.com/openai/codex/blob/3d3ae4965ab370217e871b3a7f0d15589557ee4b/codex-rs/core/src/tasks/regular.rs)：task token向实际turn派生child token。
- [OpenAI Codex interrupt](https://github.com/openai/codex/blob/3d3ae4965ab370217e871b3a7f0d15589557ee4b/codex-rs/core/src/session/mod.rs)：interrupt针对当前active task并等待统一abort路径。
- [Kubernetes resource versions](https://kubernetes.io/docs/reference/using-api/api-concepts/#resource-versions)与
  [HTTP If-Match](https://www.rfc-editor.org/rfc/rfc9110.html#name-if-match)：stale revision必须显式失败，
  不能由live控制投影覆盖持久化authority。

## 复用与实现约束

- 复用`TaskClaim::execution_id`、`TaskSubagentContext`、`SubagentAttemptIdentity`、`dispatch_attempt`、
`SubagentControlRegistry`、queued guidance与CancellationToken；不新增依赖。pending interrupt使用独立
bounded map/order，因为guidance可累积而cancel intent是单个幂等事实。
- 标准库mutex与Tokio child token覆盖原子交接和取消传播；不新增自定义scheduler。
- Team adapter只做prompt/metadata转换，不拥有claim、control registry或settlement。
- default Team、React Team和custom Team都实现同一`TeamDispatchController`；没有隐藏的无control路径。
- public exact interrupt只通过`RuntimeTaskService`；registry pending API保持crate-internal。
- `settle_resolution`/`abandon_claim`只做durable CAS；post-CAS live cleanup由executor单独结算并诊断。
- JoinSet/AbortHandle只由canonical RuntimeDagExecutor持有；supervisor index不成为新调度器或terminal权威。
- Team control必须经同源`TeamRuntimeHandle`；不能只暴露registry或临时构造另一个in-memory store。
- ambiguous CAS后的reload/recovery/replay统一调用`reconcile_attempt_control`，没有只留在内存的永久debt。
- 迁移阶段每个提交必须切换真实Team主路径，不保留长期平行callback。
- 所有新增文本截断必须UTF-8安全；不得使用panic API或越界索引。
- 框架先合入；SDK在独立worktree pin该SHA后迁移。SDK未合入前Finding与Issue保持open。

## 验收标准

1. Team runtime的每个Subagent event/runtime context都等于对应TaskClaim派生的run/task/execution/revision/attempt。
2. 同一physical claim在claim CAS、Subagent control、event和terminal中使用同一execution ID。
3. exact interrupt只取消目标task child，至少一个并行sibling正常完成。
4. claim后、reservation/admission前的interrupt被原子消费；等待许可立即结束且不占permit，目标
   Agent/model/tool调用次数为零，task结算Cancelled。
5. pending/reserved projection在admission、durable settle/abandon、supersede与reclaim各路径精确consume/retire；
   unknown CAS不删除，容量不会被已结束claim永久占用。
6. public interrupt在settled tombstone淘汰与进程重启后仍用TaskClaim precondition拒绝旧command；
   duplicate/stale/conflicting/capacity-full返回完整typed receipt/error，不静默作用于新attempt。
7. run cancel仍取消全部reserved/active child；grace后forced abort把已取消Subagent结算Cancelled而非Failed。
8. durable CAS成功后注入live retirement failure，真实Task terminal不变、无abandon/第二次CAS，
   observer收到retryable cleanup receipt。
9. Team默认、React Team、自定义TeamDispatchController及framework consumer tests完成结构化request迁移，
   并通过同源TeamRuntimeHandle共享Task store、RuntimeTaskService与control registry。
10. 注入“durable CAS已提交但响应丢失”：首次不删除projection、不abandon；随后reload/run recovery/
    command replay证明claim非current并清理projection，cleanup失败只进入observer。
11. 外部CAS supersede live active attempt时，reconciliation先cancel-and-drain并保留binding/watch，
    定向grace到期后只abort目标并等待JoinSet结算；非协作目标join/abort完成后无新的本地model/tool
    effect，sibling仍正常完成，同一attempt observer可见Cancelled。远端已接纳effect单独报告。
12. SDK Host在后续阶段使用相同context/control API，并通过command replay E2E；不产生第二identity算法。
13. framework focused tests、公共API 17-feature矩阵、完整合并门禁、独立review与strict semantic gate全绿。
14. SDK阶段完成前#99保持open，并在外部Issue记录已交付framework SHA与剩余边界。
