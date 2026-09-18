---
schema_version: 1
id: evidence.channel-driven-turn-repair
kind: evidence
observed_at: source:8d956c10941594a85bb7c3d79c385fbb7e6ea4c25abc4a67f46bf8a99dc078b5
source_refs:
  - src/channels.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - echo-integration/src/channels/session.rs
  - docs/adr/0046-turn-execution-delivery-settlement.md
  - docs/adr/0057-channel-generation-delivery-fence.md
supports: [behavior.agent-turn-lifecycle, rule.turn-terminal-authority]
limitations:
  - Raw Rust Agent API remains a valid lower-level contract and does not automatically create a TurnReceipt
  - Channel receipt delivery records EventSink acceptance, not remote provider delivery ACK
---

# Channel driven Turn repair

## 支持的结论

`AgentChannelHandler::drive_turn` now constructs one fresh Turn identity for each
inbound message and awaits `AgentTurnDriver.drive`. The existing Turn driver
owns envelope sequence, execution outcome, delivery outcome, usage, and final
answer. `MessageHandler::handle` projects a reply only from the completed and
delivered receipt with a final answer; producer failure and cancellation remain
non-success outcomes. Conversation identity encodes channel, conversation, and
sender as a structured tuple, without introducing another store or lifecycle
state machine. `SessionHandler` continues to own generation and transport
delivery fencing.

## 来源与范围

EKO's separate `AppChannelMessageHandler` enters the same framework driver
through `drive_foreground_pooled_chat_turn`; it does not construct this framework
handler or call `ReactAgent::chat` on its production channel path.

## 已知缺口

This does not change raw Agent APIs, A2A adapter authority, or remote provider
delivery settlement. Independent rereview and main delivery remain outstanding.
