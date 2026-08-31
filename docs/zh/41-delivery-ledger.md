# Delivery Ledger

`echo_agent::delivery` 提供产品无关的 durable delivery 原语。调用方保留自己的 route
与 payload 类型，framework 负责顺序、attempt、生命周期事实、保留和恢复。

## 公共 API

稳定 SDK facade 位于 `echo_agent::delivery`：

```rust
use echo_agent::delivery::{
    DeliveryEnvelope, DeliveryLedger, DeliveryLedgerConfig, DeliveryPayload,
    DeliveryRoute, DeliverySettlement, DeliveryTransition,
};
```

`DeliveryEnvelope<Route, Payload>` 是泛型类型。简单调用方默认使用 `String` 与
`serde_json::Value`；有领域地址和消息类型的应用直接使用自己的类型：

完整的可编译 typed lifecycle 示例位于 `DeliveryLedger` rustdoc，并演示了包含 `f64` 字段的
payload。

```rust
type Ledger<J> = DeliveryLedger<J, MyAddress, MyMessage>;

fn drive<J>(ledger: &Ledger<J>, address: MyAddress, message: MyMessage) -> echo_agent::error::Result<()> {
    ledger.enqueue(DeliveryEnvelope::new("message-1", address, message))?;
    if let Some(claim) = ledger.claim_next()? {
        ledger.transition(&claim, DeliveryTransition::effect_started("turn-1"))?;
        ledger.transition(
            &claim,
            DeliveryTransition::settled(DeliverySettlement::terminal(
                Some("turn-1".to_string()),
                echo_agent::delivery::DeliveryOutcome::Completed,
                Some(true),
                None,
                None,
            )),
        )?;
    }
    Ok(())
}
```

`DeliveryRoute` 校验调用方拥有的 route。`DeliveryPayload` 保证 payload 可由 serde 拥有、保留和
重放，并只要求可做部分相等比较而不是 `Eq`，因此包含 `f64` 等值的 payload 仍可使用。projection
直接返回带类型的 `DeliveryRecord<Route, Payload>`，调用方不需要按来源命名的转换层。

## 生命周期

```text
Persisted -> Claimed -> EffectStarted -> MailboxAccepted -> Drained -> TurnSettled
                         \-> Deferred -> Claimed（新 attempt）
```

每个 claim 都带严格递增的 attempt 和 opaque attempt ID。`EffectStarted` 必须携带实际 turn
identity。`DeliveryClaim.payload` 保留从 frontier 选出的调用方 payload，route 与生命周期 identity
仍保持 typed。`MailboxAccepted` 与 `Drained` 必须匹配同一 attempt 和 turn。可重试 settlement 必须
带 next-attempt 时间；terminal settlement 会离开 FIFO frontier。

Owner 丢失使用 `OutcomeUnknown` 表示；产品明确 retirement delivery 时可以使用 `Dropped`。
framework 不从输出文本或 transport EOF 推断成功，没有新的显式 claim 就不能重放 effect。

## 保留与恢复

Journal sequence、durability、checkpoint recovery 和物理 segment pruning 仍由
`echo_agent::state::journal` 负责。delivery logical terminal retention 同时受
`DeliveryLedgerConfig` 的数量和字节上限约束。projection 可丢弃；恢复时从 durable typed journal
重放并重新应用相同边界。checkpoint 恢复还会在状态交给调用方之前校验 order/frontier 成员关系、
route identity、attempt identity 以及各 phase 的 terminal 字段一致性。

`DeliveryLedger` 组合现有 `EventJournal` 与 `CheckpointedReducer` authority。
`PreparedJournalBatch` 保留 append identity 以支持 retry/reconciliation；unknown outcome 后必须
reopen 并 lookup，再决定是否重试。

`JournalDurabilityStatus` 是 canonical tagged durability 值（`unconfirmed`、`confirmed` 或带
error 的 `degraded`）。应用可以直接暴露它，不需要再定义平行的 receipt enum。

需要自定义物理 durability 或 reopen 处理的宿主可以使用
`DeliveryLedger::apply_prepared_with`。callback 只能为同一个 prepared batch 提供 journal receipt；
framework 会在 fold 前校验 batch identity 与 payload digest，生命周期 preflight、checkpoint、
retention 和提交后校验仍由 framework 负责。

`DeliveryTransition` 是唯一的 lifecycle command 类型。普通调用方可以把它传给
`transition`；需要通过自定义物理 journal 提交的宿主则把同一个值传给
`prepare_transition`。原有的命名方法（`begin_effect`、`accept_mailbox`、`mark_drained`、
`defer`、`settle`）仍作为同一 command 路径上的便捷入口。

`prepare_claim_next` 返回 `DeliveryClaimDraft`，`prepare_transition` 返回已校验的
`DeliveryEvent`，不再需要应用侧自己定义转换类型。

EKO 直接使用 `DeliveryLedger<Journal, AgentAddress, AgentMessage>`。workspace policy 与
GUI/TUI/CLI 展示仍由应用负责，但 durable delivery 只有一个 framework record 和 reducer。这个
typed API 取代临时 legacy wire bridge；schema 仍在开发期，需重新创建本地 data root。详见
[ADR 0019](../adr/0019-typed-delivery-ledger-api.md)。
