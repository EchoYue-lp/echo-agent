---
schema_version: 4
slug: 2026-09-14-turn-terminal-delivery-settlement/plan
outcome:
  summary: TurnReceipt 同时携带唯一 Agent execution outcome 与 typed delivery
    outcome；terminal delivery failure 不再把已完成执行改写为 Failed，ACP/SDK 能准确报告交付失败。
  acceptance:
    - driver测试证明通用sink的terminal/pre-terminal
      failure与Closed；Headless、Eval、ACP和SDK
      host分别证明其真实normal、failed、取消与历史恢复路径保留execution与delivery结果。
    - cargo fmt、SDK contract 检查、完整 clippy、workspace tests 和 no-default-features
      check 全部退出 0。
out_of_scope:
  - "不在本切片修复 Channel 绕过 TurnDriver 的 Finding #107 或 A2A 第二套终态的 Finding #35。"
  - 不在本切片建立 transcript retry/debt store、异步 outbox 或修改 EKO/echo-agent-cli。
  - 不通过新增 RunHandle operation 读取 delivery；SDK 只使用 RunGet/RunWait receipt 字段。
design_ref: docs/supreme/specs/2026-09-14-turn-terminal-delivery-settlement/design.md
design_sections:
  - ref: design.md § 2. 目标行为
    digest: sha256:bce534858b42d3c19184b18e44baa78d7462c4b1336f1ed56f191b9521e39d63
  - ref: design.md § 4. 系统边界
    digest: sha256:92fb8a558940e57206ff3877097e57c33306c488b977eadbc0eb0013791c77cc
  - ref: design.md § 5. 当前代码事实与复用结论
    digest: sha256:1ef7b5db832bc89c6632d788c17b5190b7f8585b0f90f979c3575f0e4b31150d
  - ref: design.md § 6. 核心数据流
    digest: sha256:b54f6846476597861dd9f9a3db3141bb88b586d05914fc2f510dff0c6795405f
  - ref: design.md § 7. ACP 映射
    digest: sha256:7e51d4fd549b088b21a0f0f3c240d21a5de8f20b9c58c07f45311b0011f0de4d
  - ref: design.md § 8. 异常与边界场景
    digest: sha256:f14f47d431d1a485ed558ad2d8fb703d7c572a5dd5a234530b7270a36ff28549
  - ref: design.md § 9. 关键取舍
    digest: sha256:233ae73d2f4dd747372fb2f353f703764c8d53d88e7a8a4ac484553fb12e5d98
  - ref: design.md § 11. 文档、示例与 SDK 合同
    digest: sha256:1a9591b79af805cd75b37a4a76f769ca30911bfa09e6aff44343e1615f5d20b4
  - ref: design.md § 12. 验收标准
    digest: sha256:9978d0886ee7348a575510c8c1b6dacf952a86ab13079aa50f957c431411346e
delivery_ref: null
todos:
  - id: driver-execution-delivery-reduction
    summary: 扩展 TurnReceipt 并让 driver 独立归约 execution 与 delivery 结果
    files:
      - echo-orchestration/src/runtime/turn_driver.rs
      - echo-orchestration/src/runtime/mod.rs
      - echo-orchestration/src/lib.rs
    acceptance:
      - FinalAnswer、Cancelled、producer Error 的 terminal sink failure 保留
        execution terminal/final facts 并返回 delivery Failed；非 terminal failure
        仍取消并返回 Failed + delivery Failed；Closed 只在 terminal 前合成 Cancelled。
      - driver 定向测试覆盖正常 Delivered、NotAttempted、Closed、Failed、缺失 terminal、错误
        envelope 和 input lifecycle。
  - id: acp-receipt-and-wire-mapping
    summary: 让 ACP Ledger、projector、observer、Prompt response 和 SDK wire 保留双结果
    files:
      - src/acp/runtime.rs
      - src/acp/projection.rs
      - src/acp/adapter.rs
      - echo-sdk-host/src/core_profile/wire.rs
      - echo-sdk-host/src/core_profile/persistence.rs
      - echo-sdk-host/src/core_profile/mod.rs
      - echo-sdk-host/src/core_profile/handler.rs
      - echo-sdk-host/src/core_profile/handles.rs
      - echo-sdk-protocol/src/methods.rs
      - tests/acp_agent_adapter.rs
      - tests/acp_extension_runtime.rs
    acceptance:
      - Journal/projector/observer failure 可观察为 delivery Failed，Run
        status/terminal 仍来自 execution outcome。
      - RunReceiptWire 无损保存新 delivery 状态和 AgentFailureWire，并将旧 JSON 缺字段保留为
        legacy_unknown；ACP 只有 Completed + Delivered 返回 end_turn。
  - id: contract-documentation-synchronization
    summary: 同步 ADR、生命周期文档、SDK inventory 和扩展合同
    files:
      - docs/adr/0010-canonical-turn-receipt-accounting.md
      - docs/adr/0046-turn-execution-delivery-settlement.md
      - docs/en/10-streaming.md
      - docs/zh/10-streaming.md
      - docs/en/lifecycles.md
      - docs/zh/lifecycles.md
      - docs/sdk/acp-agent-adapter.md
      - docs/sdk/protocol.md
      - echo-sdk-protocol/src/inventory.rs
      - echo-sdk-host/src/core_profile/facade/source_operations.rs
      - contracts/sdk/schema/echo-agent-extension-v1.schema.json
      - contracts/sdk/fixtures/extension/v1/run-receipt-completed-valid.json
      - contracts/sdk/fixtures/extension/v1/run-receipt-delivery-failed.json
    acceptance:
      - 文档明确双终态、双 watermark、legacy_unknown 和 ACP 对外顺序；生成的
        schema、inventory、catalog、digest 与源码一致。
  - id: terminal-delivery-regression-coverage
    summary: 补齐跨层回归场景并验证调用方同时读取 execution 与 delivery
    files:
      - src/headless.rs
      - src/eval/runner.rs
      - tests/agent_handle_turn_driver.rs
      - tests/acp_agent_adapter.rs
      - echo-sdk-host/tests/core_profile_e2e.rs
      - echo-sdk-host/src/core_profile/wire.rs
    acceptance:
      - Headless、Eval、ACP、SDK
        host的真实normal/failed/legacy场景以及driver通用Closed场景均通过；所有
        TurnReceipt/RunReceiptWire literal都提供新字段或使用安全构造器。
artifact_id: plan:0e050978-89ae-42b1-954e-26c62b1220b9
lifecycle: completed
---
## Notes

本 Plan 只覆盖一个可独立合并的 #108 repair slice。执行顺序固定为：先修改 driver 的双结果归约，再接入 ACP/SDK wire 与 persistence，随后同步 ADR、文档和生成合同，最后运行跨层测试和仓库完整门禁。
