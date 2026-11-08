---
schema_version: 1
id: map.protocol-surfaces
kind: capability_map
title: ACP、A2A、Channels、Headless 与 SDK Surfaces
risk: high
observed_at: source:3fc37deb9da5313ffb1d3e80bc29162f1969b12d88d7bf35a2d1e74706b635fc
boundary_refs:
- boundary.protocol-surfaces
behavior_refs:
- behavior.protocol-projection
rule_refs:
- rule.protocol-role-separation
- rule.sdk-rust-authority
evidence_refs:
- evidence.provider-protocol-quality
- evidence.sdk-contracts
- evidence.high-risk-audit-frontier
- evidence.framework-concept-navigation
- evidence.agent-adapter-close-settlement-repair
- evidence.agent-adapter-close-settlement-verification
- evidence.sdk-deferred-backlog-count-repair
- evidence.sdk-deferred-backlog-count-verification
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.lsp-response-value-fidelity-investigation
- evidence.lsp-diagnostic-notification-repair-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
finding_refs:
- finding.a2a-terminal-authority
- finding.a2a-stream-cleanup
- finding.a2a-task-id-admission-authority
- finding.a2a-advertised-capability-binding
- finding.channel-attachment-projection
- finding.channel-reset-stale-generation-delivery
- finding.turn-driver-entry-coverage
- finding.agent-adapter-close-settlement
- finding.turn-terminal-commit-projection-order
- finding.sdk-gap-generation-validation-parity
- finding.sdk-gap-ack-replay-watermark
- finding.sdk-deferred-backlog-count-drift
- finding.extension-invocation-cancellation-scope
- finding.lsp-response-value-fidelity
- finding.lsp-diagnostic-notification-fidelity
audit_refs:
- audit.protocol-surfaces.state-authority
- audit.protocol-surfaces.time-lifecycle
- audit.protocol-surfaces.contract-evidence
- audit.semantic-governance-final-rereview
- audit.agent-adapter-close-settlement-rereview
- audit.framework-final-34d5-gate-rereview
related_map_refs:
- map.agent-session-turn
- map.observation-persistence-delivery
- map.extension-lifecycle
- map.sdk-facade-parity
scenarios:
  weixin-durable-no-stream-admission:
    status: needs_review
    source_refs:
    - docs/en/15-im-channels.md
    - docs/zh/15-im-channels.md
    - echo-agent-learning/examples/demo38_im_channels.rs
    - echo-integration/src/channels/channels/mod.rs
    - echo-integration/src/channels/mod.rs
    - echo-integration/src/channels/session.rs
    - echo-integration/src/channels/types.rs
    - docs/adr/0099-personal-weixin-channel.md
    - echo-integration/src/channels/channels/weixin/api.rs
    - echo-integration/src/channels/channels/weixin/auth.rs
    - echo-integration/src/channels/channels/weixin/channel.rs
    - echo-integration/src/channels/channels/weixin/codec.rs
    - echo-integration/src/channels/channels/weixin/mod.rs
    - echo-integration/src/channels/channels/weixin/state.rs
    - echo-integration/src/channels/channels/weixin/wire.rs
    rule_refs: [rule.channel-invocation-generation]
    unknown: Framework source review and focused checks passed for retained Rust inputs; complete App/A2/native/main and full merge qualification remain unverified, with historical evidence scope preserved.
    next_step: Preserve actual focused raw/close receipts and complete downstream App/A2/native and task-main qualification; source/focused acceptance is not global closure.
  acp-session-run-projection:
    status: mapped
    source_refs:
    - src/acp/session.rs
    - src/acp/runtime.rs
    - src/acp/projection.rs
    behavior_refs:
    - behavior.protocol-projection
    rule_refs:
    - rule.turn-terminal-authority
    - rule.protocol-role-separation
  a2a-task-and-stream:
    status: needs_review
    source_refs:
    - docs/adr/0090-a2a-lsp-retirement-contract.md
    finding_refs:
    - finding.a2a-terminal-authority
    - finding.a2a-stream-cleanup
    - finding.a2a-task-id-admission-authority
    - finding.a2a-advertised-capability-binding
    unknown: A2A 自持 terminal、重复 ID 无 generation、stream cleanup 与宣告 capability 均未闭合
    next_step: 分别 repair admission/cleanup/terminal，并 semantic-decide text+SSE 或 file/push 合同
  channel-session-and-message:
    status: needs_review
    source_refs:
    - echo-integration/src/channels/session.rs
    - echo-integration/src/channels/types.rs
    - src/channels.rs
    finding_refs:
    - finding.channel-attachment-projection
    - finding.channel-reset-stale-generation-delivery
    - finding.turn-driver-entry-coverage
    evidence_refs:
    - evidence.provider-protocol-quality
    - evidence.channel-generation-delivery-fence-repair
    - evidence.channel-generation-delivery-fence-verification
    - evidence.agent-adapter-close-settlement-repair
    - evidence.agent-adapter-close-settlement-verification
    unknown: Channel session/incarnation、reset delivery fence与Agent close已映射，但handler仍丢弃attachments并直接raw chat
    next_step: 分别repair attachment投影与driven Turn入口；不重建close owner
  headless-turn:
    status: mapped
    source_refs:
    - src/headless.rs
    - echo-orchestration/src/runtime/turn_driver.rs
    behavior_refs:
    - behavior.protocol-projection
    rule_refs:
    - rule.turn-terminal-authority
    evidence_refs:
    - evidence.agent-adapter-close-settlement-repair
    - evidence.agent-adapter-close-settlement-verification
  sdk-host-and-language-clients:
    status: needs_review
    source_refs:
    - .echo-semantic/assets/asset.external-sdk-repository.md
    - docs/adr/0031-sdk-identity-governance-scope.md
    behavior_refs:
    - behavior.sdk-facade-routing
    rule_refs:
    - rule.sdk-rust-authority
    evidence_refs:
    - evidence.sdk-contracts
    finding_refs:
    - finding.sdk-gap-generation-validation-parity
    - finding.sdk-gap-ack-replay-watermark
    unknown: Gap generation校验已统一，但Client确认snapshot watermark后，Host resume watermark仍可能回退并重发旧事件
    next_step: Repair gap ACK后的单调resume watermark，不扩大到intrinsic identity门禁
  sdk-deferred-backlog:
    status: needs_review
    source_refs:
    - .echo-semantic/assets/asset.external-sdk-repository.md
    - docs/adr/0031-sdk-identity-governance-scope.md
    - docs/adr/0032-sdk-contract-scope-classification.md
    evidence_refs:
    - evidence.sdk-deferred-backlog-count-repair
    - evidence.sdk-deferred-backlog-count-verification
    finding_refs:
    - finding.sdk-deferred-backlog-count-drift
    audit_refs:
    - audit.semantic-governance-final-rereview
    unknown: 1448个deferred identity的capability分组、外部用户价值与逐组产品合同决策尚未闭合
    next_step: 按externally useful capability审查deferred；Host/Rust-only、language intrinsic与internal helper不是语言parity
      backlog
  product-backend-desktop-device:
    status: excluded
    source_refs:
    - docs/en/39-framework-application-boundary.md
    reason: Product Backend、Frontend/Desktop、Canvas、Electron/Tauri 与 Device sync 不属于 echo-agent framework 当前源码
    risk: high
    recheck_when: 出现 product-neutral protocol primitive 且不依赖应用 state/UI 时
  extension-invocation-cancellation-isolation:
    status: mapped
    source_refs:
    - src/acp/extension.rs
    evidence_refs:
    - evidence.task-runtime-store-control-repair
    - evidence.task-runtime-store-control-verification
    finding_refs:
    - finding.extension-invocation-cancellation-scope
---





# ACP、A2A、Channels、Headless 与 SDK Surfaces
## 当前 Task 的微信业务回执依据

`echo-integration/docs/weixin-business-acceptance.md` 承接当前
`echo-integration/src/channels/channels/weixin/wire.rs`／`api.rs`／`channel.rs` 的业务接受
边界及真实验证限制。getupdates 在无拒绝码、无 null 业务码时，使用明确 ret／errcode
业务0或当前 typed msgs／cursor 建立接受；非零／-14、raw null 与坏类型保持各自结果。
HTTP 成功本身不是业务证据。当前来源独立记录于 `map.canonical-output-consumers`。
sendmessage 新增的缺 ret ACK 还要求原 typed 非空 ID 且无错误码／null 码；原明确 ret=0
及 String/u64 decoder 保留。Unknown 不回填、不授权自动重发，Accepted 不等于手机 Received。

此依据复用原请求、发送回执、cursor／ACK 与 Stop owner；一次有界脱敏诊断不输出
正文、token、cursor 或未知键名。本文仅补当前项目关联，不改历史 `observed_at`、
原场景状态／unknown、旧观察与资格；不引入 getConfig／typing、第二 poller 或新执行器。


## 能力范围

覆盖外部 Client/Agent/peer/channel/headless/source SDK 入口及其 Session/Run/event/handle projection。

## 入口与输出

ACP stdio、A2A HTTP/SSE、IM WebSocket/webhook、Headless prompt 和 SDK process client 进入；输出标准或 namespaced wire 与外部 delivery。

## 行为关系

ACP、MCP、A2A 角色分离；Headless/SDK Host 复用 driven Turn；Channel 当前只复用 raw Agent execution，A2A 当前自持 task/terminal。

## 状态与数据流

Session registry、A2A task map、Channel incarnation、SDK handle/generation 分别提供寻址；通用 terminal 应来自 framework runtime。

## 策略来源与优先级

Protocol capability negotiation、typed config、JWT/channel credentials、SDK contract 与 application adapter 决定可用 surface。

## 生命周期与失败路径

Initialize/create/start/stream/cancel/replay/close/recover；unknown method、stale handle、gap、disconnect 和 backpressure 显式处理。

## 权限与敏感信息

ACP permission 与 Agent policy 只在协商后调用；channel/provider credentials 不进入 event；本地协议不套多租户假设。

## 用户侧投影

各 surface 可有不同 wire/UI，但功能与通用生命周期事实不得分叉。

## 场景处置清单

五类入口均有路由；非A2A adapter close已闭合，A2A/Channel/SDK其它gap保持needs_review并进入
各自Finding，SDK deferred capability backlog独立needs_review，产品层excluded。

## 未展开项

现有 `map.sdk-facade-parity` 保留完整 SDK 子边界，不在本 map 重复 identity。

组合候选补充范围：ACP 恢复身份预留仍由 Session TurnSlot 唯一分配；LSP response/diagnostic 由 framework 解析，URI 单缓存区分有效结果与明确错误，fatal admission 后不返回假空。当前只有指定切片的局部复审和反例证据，组合 23 项门禁运行中；SDK 已发布 pin、其它协议缺口和 main 仍各自验收。


当前工程补充：同source34d5的新23门禁真实全通过，3471 passed/0 failed/3外部optin ignored、86targets/59examples；见 evidence.framework-final-34d5-full-verification 与原始 evidence.framework-final-34d5-gate-archive。原历史门禁和局部Audit不改写，mapped不表示所有lens/消费者/main已验收。


当前候选最终复审：audit.framework-final-34d5-gate-rereview 绑定新23真实工程收据与完整已审源码，implementation review PASS/action0；不替代独立SDK/main验收，不升级原broad状态。


## Weixin NoStream current source increment

Native Weixin poll uses original fallible bind/no-stream admission and original sender; exact context presence validation supports host recovery, with no EKO binding fields in Framework. Initial NoStream source observation: `source:4bc6e5fb3d60dd9ce705013f006f8eec0279cbbc1fd1414320b0574058187374`; prior map observation `77de6ca90ad842eb4b7b2921a4baa954f41cccfe` remains historical, with all old scenario statuses/evidence unchanged. The added scenario is needs_review. It does not re-date old engineering results or claim A2 application implementation, SDK accepted changes, real QR/messages, complete qualification or main delivery. The external formal Design c791eea4 / Plan fb4fafcd is the authorized cross-repository input, not a local Asset.

Historical Clippy-only source increment: `source:bf1789e2c47fc26e7e331858887f567067068e077aae6feb3c1a7ca24eda074b`. Two equivalent let-chain rewrites and one test identity-result discard resolve actual lint diagnostics; no scenario status, contract, assertion, API, manifest/lock or historical result is changed. Source/build acceptance still requires the actual repeated affected checks.

Historical documentation-only observation: `source:a40fa344d8df5fcccba273828d7f086dd1100f182e875cdae3a9a6bd04506557`; all Rust inputs remain identical to executed v4 `source:bf1789e2c47fc26e7e331858887f567067068e077aae6feb3c1a7ca24eda074b`. Actual focused batch selected135 channel tests (including NoStream10), root12 and doc2compile with0failed/0ignored, example/scoped lints/featureless/fmt0 and exactclean0(targetabsent). Original02warning/07lint101/sourceSemantic1 remain retained. Source reviews8cf8+777eac and batch `df4e7cf05d1ea805c37bcdcdbd157a6c5667c6d65588513b495928f972c8c2d5` / cleanup `49d90a0f95c77725411a5df48d63273eeef0443000faeebaa769f88fba3ef4d1` are local authoring-worktree evidence under `.echo-semantic/local/weixin-no-stream-build/focused-actual/`, not distributed assets or a new scan. No historical object status or full/global/native/consumer/main claim is changed.

Durable-reset current observation: `source:3fc37deb9da5313ffb1d3e80bc29162f1969b12d88d7bf35a2d1e74706b635fc`. The exact existing Session→inner reply regression genuinely selected1 and failed None versusSome(reset-1), then passed after only durable outbound `.with_reply_to(original message_id)`; original scalar/stream/owner/API/lock unchanged. Current Rust inputs match executed green v7 `source:c3fbb9242dcf81f0e05e6ce9e074c1396707d343447db08350e42ce62cfbc032`; exactgreen1, all135 channels (including NoStream10), scoped warnings/strictpanic and fmt/check0, exactclean0 targetabsent. Batch `59070d12b5afec08224894070f20f2d31d4aa07d7d2c3fdff0706397412ba6af` and cleanup `4f69bd9889e90f8612696fdd9863e2ded7ef8d55acbde42767e46e58da9ebecb` are preserved local evidence under `.echo-semantic/local/weixin-no-stream-build/reset-reply-actual/`. Prior root12/doc2/demo/features retain their actual oldepoch and unchanged-API/legacy branch applicability, not a new run. Scenario status/oldFindings stay unchanged; App/A2/native/platform/fullmain remain separate.


## 2026-10-28 已批准专用协议退出后的引用定位

本对象原observed_at、原scenario status、unknown、next_step与全部Finding/Evidence
身份保留其历史范围；不关闭旧行为问题，不冒当前运行时通过。具名A2A/LSP专用实现
已在当前WIP删除，故这项场景的当前可读sourceRefs指向所属ADR0090的退役合同。
原source:3fc37deb9da5313ffb1d3e80bc29162f1969b12d88d7bf35a2d1e74706b635fc
下的旧路径为：
- `src/a2a/server.rs`：历史实现；可恢复Git0e404185c8f252da3e335c35547e52249c74dc65，精确Blob/SHA见新增历史asset。
- `src/a2a/types.rs`：历史实现；可恢复Git0e404185c8f252da3e335c35547e52249c74dc65，精确Blob/SHA见新增历史asset。

这不是从闭包排除未知义务：其它动态/needs_review以及90个原loaded关联保原，
也不重新注册旧协议或引入兼容shim。原native执行/ACP/MCP/channel/Subagent/Store
与runtimeFinal通知另按各自当前合同维护。
