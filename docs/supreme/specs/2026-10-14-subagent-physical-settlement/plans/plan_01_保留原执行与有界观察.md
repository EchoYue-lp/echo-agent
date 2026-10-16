---
schema_version: 4
slug: subagent-physical-settlement-b/retain-owner
outcome:
  summary: 取消观察有界而原Subagent执行owner保留到physical关闭及同claim CAS
  acceptance:
    - 当前源码held-cleanup红在B实现后绿；无取消的普通长执行没有grace
      timeout，root/exact重复请求不延长观察界限且exact A不影响sibling B。
    - 原owner关闭前claim/interruption/permit保留；释放后一次原终态和exact CAS；caller
      drop/unwind与unknown恢复均不可伪停止或重复effect。
    - ExecutionNotSettled只描述观察知识，不会转换为AgentFailure或Failed；框架六项与适用独feature门禁均真实通过，精确merged
      main CI通过后交付。
out_of_scope:
  - 修改SDK、Host wire、accepted seed/generated contract或语言API；另独立线程拥有这些路径。
  - 新TaskStatus/store/scheduler/receipt
    authority、应用产品策略、改变执行timeout预算或批量复制旧诊断raw。
design_ref: docs/supreme/specs/2026-10-14-subagent-physical-settlement/design.md
design_sections:
  - ref: design.md § Observation and settlement
    digest: sha256:a814268a6ea99e90df16da929ed721fd66ff2c58fa26289fa7d6e69476f77b5d
  - ref: design.md § Stream ownership and cause ordering
    digest: sha256:0afa91fc150b1c73804da4819b38aeb49ad1e4d20300ed2f6b09c3f0f4e5ed9f
  - ref: design.md § Recovery and limits
    digest: sha256:6ceb3e3ddf365401b051fd40eaa6ade66ed97649d786c8f1ce2e423e3e2b40a1
  - ref: design.md § Acceptance
    digest: sha256:d43603db30687a8fd283d1f60629956cd45f1dcf8dc0d6f7671363eada6d72be
delivery_ref: null
todos:
  - id: observe-without-terminal
    summary: 用原runtime service/exact control提供首次取消的观察deadline，保留真实join和CAS权威
    files:
      - echo-core/src/error.rs
      - echo-orchestration/src/tasks/runtime_service.rs
      - echo-orchestration/src/tasks/runtime_executor.rs
    acceptance:
      - held-cleanup超过grace时只observer返回typed
        notsettled、原claim和permits仍在；重复exact/root不reset、A与B分离、recovery无receipt不清claim。
  - id: retain-original-stream-owner
    summary: Sync/Fork/Teammate及managed producer在取消/drop/unwind中保留原stream drain并保首cause
    files:
      - src/agent/subagent/executor.rs
      - src/agent/subagent/team/mod.rs
      - src/agent/react/run/stream_channel.rs
      - echo-core/src/agent/event_envelope.rs
      - echo-execution/src/tools.rs
      - echo-core/src/agent/mod.rs
      - echo-orchestration/src/runtime/turn_driver.rs
    acceptance:
      - 三mode cancellation/deadline/raw
        failure均在原EOF后发布一次原cause和terminal，drop收尾不abort原producer或提前释放guard。
      - Canonical started Tool 的 timeout/Complete/receiver-close
        保原future、permit和资源；真实Task claim在原producer关闭前保留，普通非draining策略不变。
      - 同原run/execution/source的观察后真实terminal+EOF结算原receipt/input/exact
        claim一次，首cause保；no-terminalEOF/foreign
        generation/lateUnknown/sink观察均不伪消。
  - id: typed-observation-and-delivery
    summary: 收敛观察错误的非终态消费边界、维护ADR与语义并以独审和完整门禁交付
    files:
      - echo-core/src/error.rs
      - echo-core/src/agent/mod.rs
      - src/agent/subagent/team/mod.rs
      - docs/adr/0088-subagent-settlement-observation.md
      - .echo-semantic
      - echo-tools/src/code.rs
      - src/agent/snapshot.rs
      - src/agent/react/run/pipeline.rs
      - src/agent/react/run/phases/tools.rs
      - src/acp/session.rs
      - src/headless.rs
      - echo-agent-learning/tests/example_contracts/demo54_headless.rs
      - echo-orchestration/src/scheduler/runner.rs
      - echo-orchestration/src/workflow/graph.rs
      - echo-orchestration/src/workflow/checkpoint_store.rs
      - echo-state/src/delivery.rs
      - echo-orchestration/src/workflow/node.rs
    acceptance:
      - 所有实际terminal producers在观察错误分支不emit Failed、不retry
        mutation；源码引用和当前snapshot准确，原反例不被成功日志覆盖，独立复审行动0。
      - ACP close/close_all保原typedUnknown与同generation，repeat不重发；无身份owner
        loss保原Notify观察而不造identity/Failed，普通返回error重试与真成功关闭保持。
      - Headless便利返回显式携同原handle，内部result无owner环；真实fileproducer
        held时原guard/permit不释放，原5公开值/格式/普通close保，外部Rust literals迁移。
      - Scheduler原EffectStarted精claim事件可选payload原子保护current
        callback，包括legacy原首次replay的新attempt；wrongclaim不改payload/seq，原None字节和旧history控制保，Some/保护checkpoint拒旧reader；Handle借用保原parent及normal结果，未知不重投。
      - 用户已接受Workflow A原proposal23c5：同Store exactclaim保护先于Node
        effect，默认false能力在claim/prefork前拒；Memory实例与File持久保护/原OS锁/旧reader拒绝及CRUD全或无，heartbeat和Node取消/timeout/fanout/receiver关闭保原source与首cause，Unknown不重排；普通真关闭error与旧未保护恢复保持。
artifact_id: plan:19bdc455-30ff-4251-96cb-05d6615792b9
lifecycle: ready
---
