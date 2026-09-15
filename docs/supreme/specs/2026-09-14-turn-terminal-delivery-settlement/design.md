---
title: Turn 执行终态与 Delivery 结算分离设计
artifact: design
carrier: markdown
---

# Turn 执行终态与 Delivery 结算分离设计

## 1. 问题与目标

`AgentTurnDriver` 当前先从 terminal `AgentEvent` 推导 `TurnOutcome`，再把同一个
`EventEnvelope` 交给 `EventSink`。如果 sink 返回错误，driver 会把已经观察到的
`Completed`、`Cancelled` 或 producer `Failed` 覆盖为新的 `Failed`，并清除 final answer。

ACP 的 `SharedRunSink` 又把三种不同语义放进一次 `Result`：Journal/Ledger 事实提交、
`session/update` 投影和 extension observer 投递。因此 terminal event 已经让
ReactAgent trace 进入 `Completed` 后，任一后续步骤失败都可能让 `TurnReceipt` 变成
`Failed`，形成两个相互冲突的执行终态。

本设计关闭 `finding.turn-terminal-commit-projection-order`：

1. 一次 driven Turn 只有一个执行终态，由 producer 的 terminal event 决定；
2. sink 的接纳、关闭或失败是独立 delivery 结算，不得覆盖已观察到的执行终态；
3. `TurnReceipt` 同时携带执行结果与 delivery 结果，调用方不再用一个状态表达两种事实；
4. ACP 在返回最终 `StopReason` 前等待 delivery 结算，delivery 失败通过协议错误报告，
   但不得反向篡改已经完成的 Agent 执行事实。

## 2. 目标行为

### 2.1 两个正交结果

`TurnReceipt.outcome` 继续是一次 Agent 执行的唯一终态：

- `Completed`：producer 发出并结算 `FinalAnswer`；
- `Cancelled`：producer 发出取消 terminal，或 driver 因 consumer `Closed` 合同合成取消；
- `Failed`：producer 发出失败 terminal，或者在 terminal 之前发生使 driven invocation
  无法继续的框架错误。

`TurnReceipt.delivery` 新增为事件交付结果：

- `NotAttempted`：在事件流建立前就已结算，未发生 sink 调用；
- `Delivered`：所有已产生 envelope 都被 sink 接纳；
- `Closed`：sink 明确关闭，driver 不再继续投递；
- `Failed(AgentFailure)`：sink 拒绝或无法持久化、投影或观察某个 envelope。

`TurnReceipt::status()` 仍只返回执行状态，避免现有调用方把投递故障误认为模型、工具或
Agent 执行失败；新增的 delivery 查询只报告 delivery 状态。

`Closed` 是 driver 对 consumer 生命周期的合成取消：它只在 producer terminal 尚未出现时
写入 `TurnOutcome::Cancelled`；若 producer terminal 已经出现，则只写入
`TurnDeliveryOutcome::Closed`，不产生第二个 execution terminal。

### 2.2 terminal 优先级

driver 必须在调用 sink 前分类当前 envelope，但只有 producer terminal 能写入
`TurnReceipt.outcome` 的 terminal 值：

| 场景 | 执行 outcome | delivery |
| --- | --- | --- |
| `FinalAnswer` 被 sink 接纳 | `Completed` | `Delivered` |
| `FinalAnswer` 的 sink 调用失败 | `Completed` | `Failed` |
| `Cancelled` 的 sink 调用失败 | `Cancelled` | `Failed` |
| producer `Error` 的 sink 调用失败 | producer `Failed` | `Failed` |
| 非 terminal envelope 投递失败 | `Failed`，driver 取消并终止本次 driven invocation | `Failed` |
| terminal 之前 sink 主动关闭 | driver 合成 `Cancelled` | `Closed` |
| terminal envelope 后 sink 主动关闭 | producer terminal | `Closed` |
| stream 建立前失败且错误 envelope 已投递 | producer/framework `Failed` | `Delivered` |
| stream 建立前失败且错误 envelope 投递失败 | producer/framework `Failed` | `Failed` |

非 terminal delivery failure 仍会中止 driven invocation，因此执行无法成功；关键变化是
已经存在的 producer terminal 不再被随后发生的 delivery failure 覆盖。

### 2.3 final answer 与计量

- `final_answer` 和 `final_message_id` 跟随 `Completed` 执行终态保留，即使 delivery 失败；
- delivery failure 不清除已经完成的 usage、compaction 和 elapsed accounting；
- `last_event_sequence` 表示 driver 已观察到的最后一个 envelope sequence，不谎称该
  sequence 已被某个具体 sink 持久化；ACP `RunEntry.ledger.last_sequence` 仍是 sink 已
  提交的 durable/in-memory watermark，两者允许在 Journal 或 projection failure 时不同；
- 调用方必须同时检查 `outcome` 和 `delivery`，才能宣称“执行成功且结果已交付”。

## 3. 范围与非目标

### 3.1 范围

- `echo-orchestration::runtime::AgentTurnDriver` 的 terminal/delivery 归约；
- `TurnReceipt` 的 typed delivery contract；
- Headless、Eval 和 ACP 对新 receipt 字段的消费；
- ACP Journal/Ledger、projector、observer 与最终 Prompt response 的结果映射；
- trace、runtime checkpoint、transcript projection 与 receipt 的语义说明；
- Rust facade inventory、SDK contract 分类、ADR、双语生命周期文档和测试。

### 3.2 非目标

- 不在本切片修复 Channel 绕过 TurnDriver（#107）或 A2A 第二终态（#35）；
- 不在本切片定义 transcript projection 的 retry/debt store（#106）；
- 不改变 `RuntimeStateStore`、`ConversationStore` 或 `RunStore` 的存储实现；
- 不新增 EKO 产品字段、数据库或应用层投影；
- 不让 projector、observer 或 Journal 成为第二套 Agent 执行状态机；
- 不把 raw `Agent::chat*` / `execute*` API 强制改造成 driven API。

## 4. 系统边界

```text
ReactAgent / other Agent
  │ terminal AgentEvent
  ▼
AgentTurnDriver
  ├─ classify producer terminal ───────────────► TurnOutcome
  ├─ sequence + accounting
  └─ EventSink.on_event
       ├─ Journal / Ledger fact commit
       ├─ protocol projection
       └─ extension observer
                    │
                    └──────────────────────────► TurnDeliveryOutcome

TurnReceipt = execution outcome + delivery outcome + accounting
```

框架层拥有 `TurnOutcome`、`TurnDeliveryOutcome` 和 `TurnReceipt`，因为所有 driven Agent
消费者都需要区分执行与交付。ACP 只负责把 Journal、projection、observer 和 Prompt
response 映射到该通用合同，不把 ACP 专属字段下沉到 driver。

## 5. 当前代码事实与复用结论

- `AgentTurnDriver` 已经是 envelope sequence、usage、final answer 和执行 terminal 的唯一
  通用归约器，可直接扩展，不新增第二个 driver 或 reducer。
- `EventSink` 已经把每个 envelope 的接纳结果返回 driver；标准库 `Result` 与现有
  `AgentFailure` 足够承载 failure，不需要新依赖。
- `EventLedger` 已经保证 Journal-first commit；本设计保留该顺序，不复制 Journal。
- `AcpEventProjector` 与 `RunEventObserver` 已经位于 committed fact 之后；它们继续返回
  typed error，由 receipt 的 delivery 字段承载，不再冒充 producer terminal。
- ReactAgent 的 trace `RunStatus` 描述 Agent execution。本设计通过保留已观察到的
  producer terminal 使其与 `TurnOutcome` 对齐，不新增 trace store。
- runtime checkpoint 是可恢复执行状态，transcript 是用户历史 projection；二者均不是
  Turn 执行 terminal authority。各自的 durability debt 由独立 Finding 处理。

最小自定义实现是一个 product-neutral `TurnDeliveryOutcome` 枚举和 driver 的双结果归约；
标准库、Tokio、现有 `EventSink`、`AgentFailure`、Ledger 和 receipt 已覆盖其余机制。

SDK wire 不直接序列化框架枚举，而是扩展 `RunReceiptWire`：新记录总是写入
`delivery` 状态字符串；状态为 `failed` 时同时写入 lossless `delivery_error`；为兼容已有
run index，读取时两个字段都可缺省并保留为 `legacy_unknown`，不得把旧记录推断成已交付。
新写入记录不会缺省。SDK 只通过 RunGet/RunWait 的 receipt 字段读取 delivery，不新增
平行的 `RunHandle` 状态 operation。

## 6. 核心数据流

### 6.1 正常完成

1. Agent 产生 `FinalAnswer`；
2. driver 记录 producer `Completed`、final answer、message identity 与 sequence；
3. sink 完成 Journal/Ledger、projection 和 observer；
4. driver 返回 `outcome=Completed, delivery=Delivered`；
5. ACP 持久化完整 receipt，再返回 `stopReason=end_turn`。

### 6.2 terminal delivery failure

1. Agent 产生 terminal event，ReactAgent execution 已到达对应终态；
2. driver 在 sink 前记录该 producer terminal；
3. sink 的 Journal、projector 或 observer 返回错误；
4. driver 保留 producer terminal 和 terminal-only fields，记录 `delivery=Failed`；
5. ACP 持久化该双结果 receipt，并以协议错误结束当前 Prompt response；
6. 后续诊断可明确区分“执行已完成”和“输出未完成交付”。

### 6.3 pre-terminal delivery failure

1. sink 在非 terminal envelope 上失败；
2. driver 记录 delivery failure、取消 invocation，并停止继续投递；
3. 由于没有已提交 producer terminal，本次 driven invocation 以 framework `Failed`
   结算，terminal-only fields 为空；
4. receipt 同时保留 delivery failure，避免把失败来源误归到 provider 或 tool。

## 7. ACP 映射

ACP v1 把 `session/update` 定义为进度通知，把原始 `session/prompt` response 中的
`StopReason` 定义为 Turn 收口。所有 pending update 必须在最终 response 前发送。

因此 ACP adapter 使用以下规则：

- 只有 `outcome=Completed` 且 `delivery=Delivered` 才返回 `end_turn`；旧记录的
  `legacy_unknown` 不能产生成功 StopReason；
- `outcome=Cancelled` 且 delivery 未失败时返回 `cancelled`；
- producer/framework `Failed` 返回现有内部错误；
- 任意 `delivery=Failed` 返回明确的 delivery/persistence/projection 错误，但 RunEntry 和
  profile 持久化的 receipt 保留真实 execution outcome；
- `Closed` 是通用 `EventSink` 的显式 consumer lifecycle。ACP `SharedRunSink` 不返回
  `Closed`：notification transport失败归为delivery `Failed`，request/connection取消经
  cancellation authority归约；ACP不得为满足通用枚举而合成不存在的Closed状态。

Journal、standard projection 和 extension observer 仍按当前顺序执行。此切片不引入
异步 outbox；任一阶段失败都同步结束 delivery，并由 receipt 暴露失败。

## 8. 异常与边界场景

- **错误 envelope 自身投递失败**：execution failure 与 delivery failure 同时保留，
  delivery failure 不替换原始 execution failure。
- **terminal 后出现额外事件**：沿用 first-terminal execution authority；额外事件不能
  修改 outcome，delivery 仍按实际 sink 结果结算。
- **多个 sink failure**：当前同步链在首个错误停止，只记录首个 failure；不制造虚假的
  后续投递结果。
- **sink Closed**：视为明确 consumer lifecycle，不编码成 persistence failure。
  当前只有明确返回 `SinkControl::Closed` 的通用sink产生该状态，ACP不合成它。
- **receipt 外部构造**：`TurnReceipt::failed` / `cancelled` 使用 `NotAttempted`，只有 driver
  实际调用 sink 后才能产生 `Delivered`、`Closed` 或 `Failed`。
- **旧版 receipt 恢复**：缺少 delivery 字段的历史 JSON 进入 `legacy_unknown`，只用于诊断和
  显式恢复流程，不被当成成功交付。
- **持久化 receipt 失败**：这是 adapter/profile 的后置 persistence failure，不能回写
  receipt 的 execution outcome；调用方收到协议错误。
- **进程在 receipt 持久化前退出**：仍属于后续 Journal/checkpoint/recovery Finding，
  本设计不以成功日志掩盖该缺口。

## 9. 关键取舍

### 9.1 采用 execution/delivery 双结果

拒绝继续让 sink error 覆盖 producer terminal。单状态看似简单，却无法同时表达“模型与
工具已经完成”和“结果没有成功持久化/投影”这两个都真实的事实。

也拒绝把 projector/observer 变成 best-effort warning。delivery failure 必须可观察，
否则只是把冲突终态改成静默丢失。

### 9.2 不移动产品 projection 到框架

driver 只定义 product-neutral delivery contract。ACP notification、SDK observer、EKO UI
和未来 channel delivery 继续由各 adapter 所有，避免通用框架被产品字段污染。

### 9.3 不为本切片建立 outbox

现有 ACP 投递是同步、有界的；新增通用 outbox 会扩大到 retry、retention、ACK 和清理
策略。当前只要求结果可区分和不可互相覆盖，后续需要可恢复重投时再复用 Delivery Ledger
能力并由独立 Finding 决定。

## 10. 业界参考

- [OpenAI Codex app-server Turn schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/turn.rs)
  使用完整 `Turn` 对象发布 `TurnCompletedNotification`，`TurnStatus` 明确区分 completed、
  interrupted、failed、inProgress；item completion 是单独 notification。EKO 采用同样的
  typed Turn terminal，而不从单个渲染事件推断完成。
- [Claude Code programmatic usage](https://code.claude.com/docs/en/headless#stream-responses)
  把 progress JSON event 与最后的 `result` message 分开，并在退出前有界等待排队输出
  drain；SIGTERM 则明确留下未完成 Turn，不伪造结果。EKO 同样区分 execution result 与
  output delivery settlement。
- [ACP v1 Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)
  要求 Agent 在所有 pending update 发出后，才响应原始 `session/prompt` 的 StopReason；
  cancellation 也必须先停止操作、发送 pending update，再返回 cancelled。EKO 保留该
  顺序，同时把 update delivery failure 与 Agent execution terminal 分开记录。

三者的共同模式是：进度/投影事件不能代替完整 Turn 结果；最终响应是显式、typed、可
结算的边界；取消和输出排空发生在终态对外发布之前。

## 11. 文档、示例与 SDK 合同

- 更新 ADR 0010，明确原先“任何 sink failure 覆盖 execution terminal”的规则被本设计
  取代；新增 ADR 0046 记录双结果取舍。
- 更新中英文生命周期文档，解释 execution terminal 与 delivery settlement。
- `EventSink` 示例必须展示同时检查 `receipt.outcome` 与 `receipt.delivery`；无关示例只需
  通过编译合同，不批量改写。
- Rust public inventory 必须纳入 `TurnDeliveryOutcome` 及其公开方法；按现有 scope
  classifier 判断为 Rust/Host contract 或 external SDK contract，不以 identity 数量
  推动额外 facade 切片。
- 本切片不修改 `echo-website`：该站点没有 TurnReceipt/EventSink 公共合同说明；交付时
  明确记录不适用。

## 12. 验收标准

1. terminal `FinalAnswer`、`Cancelled`、producer `Error` 遇到 sink failure 时，receipt
   保留 producer execution outcome，并单独返回 typed delivery failure。
2. 非 terminal sink failure 不能留下 `Completed` 或 final-answer fields，同时 delivery
   failure 可观察。
3. stream 建立失败的原始 execution failure 不再被错误 envelope 的 sink failure覆盖。
4. Input lifecycle receipt 跟随 execution outcome，不把 delivery failure当成第二终态。
5. ACP Journal、projector、observer 的 terminal failure 产生 execution/delivery 双结果，
   adapter 不返回成功 StopReason。
6. `persist_run_settled` 接收完整、未被 adapter 重算的 receipt；其自身失败不改写 receipt。
7. ReactAgent trace `RunStatus` 与 receipt execution outcome 不再因 terminal delivery failure
   互相冲突。
8. Headless、Eval、ACP、根 facade 和 SDK contract 编译通过；现有 raw Agent API 不变。
9. 更新 ADR、双语生命周期文档、rustdoc 与相关示例，且文档合同通过。
10. semantic-preflight、首个 diff 后 semantic-diff、semantic-verify 和仓库完整合并门禁全部
    通过后，Finding #108 才能关闭。
