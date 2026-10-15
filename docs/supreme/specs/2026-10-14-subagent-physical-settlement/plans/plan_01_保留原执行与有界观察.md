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
    digest: sha256:6128c1e40638fdcdcb784a683485775b190a1912c2a81fc133d048de6aeb817b
  - ref: design.md § Recovery and limits
    digest: sha256:6ceb3e3ddf365401b051fd40eaa6ade66ed97649d786c8f1ce2e423e3e2b40a1
  - ref: design.md § Acceptance
    digest: sha256:175dbeb282e1c3244ca105ca2b0c8d2445f10c9b8c2206644108f86620f57591
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
    acceptance:
      - 三mode cancellation/deadline/raw
        failure均在原EOF后发布一次原cause和terminal，drop收尾不abort原producer或提前释放guard。
      - Canonical started Tool 的 timeout/Complete/receiver-close
        保原future、permit和资源；真实Task claim在原producer关闭前保留，普通非draining策略不变。
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
    acceptance:
      - 所有实际terminal producers在观察错误分支不emit Failed、不retry
        mutation；源码引用和当前snapshot准确，原反例不被成功日志覆盖，独立复审行动0。
artifact_id: plan:19bdc455-30ff-4251-96cb-05d6615792b9
lifecycle: ready
---
