---
schema_version: 1
id: evidence.provider-protocol-quality
kind: evidence
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
source_refs:
  - echo-core/src/llm/mod.rs
  - echo-core/src/llm/capabilities.rs
  - echo-core/src/budget.rs
  - echo-core/src/tokenizer.rs
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/anthropic.rs
  - echo-integration/src/providers/responses.rs
  - echo-state/src/compression/compressor/summary.rs
  - echo-state/src/compression/levels.rs
  - src/agent/react/builder.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/think.rs
  - src/agent/react/extract.rs
  - echo-agent-learning/examples/demo15_structured_output.rs
  - docs/en/11-structured-output.md
  - docs/en/38-factory-modes.md
  - tests/react_smoke.rs
  - src/acp/adapter.rs
  - src/acp/session.rs
  - src/acp/runtime.rs
  - src/a2a/server.rs
  - src/a2a/types.rs
  - src/a2a/serve.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/types.rs
  - echo-integration/src/channels/channels/mod.rs
  - src/channels.rs
  - src/headless.rs
  - echo-sdk-protocol/src/lib.rs
  - echo-sdk-host/src/lib.rs
  - echo-sdk-host/src/core_profile/handles.rs
  - echo-sdk-host/src/core_profile/handler.rs
  - echo-sdk-host/tests/core_profile_e2e.rs
  - sdks/typescript/src/client.ts
  - sdks/python/src/echo_agent_sdk/client.py
  - sdks/java/src/main/java/com/echoagent/sdk/EchoAgentClient.java
  - sdks/java/src/main/java/com/echoagent/sdk/BoundedPublisher.java
  - src/eval/runner.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/agent/react/run/stream_channel.rs
  - src/eval/replay.rs
  - src/improve/mod.rs
  - src/improve/loop.rs
  - src/improve/eval_improvement.rs
  - src/improve/trajectory.rs
  - src/evolution/mod.rs
  - src/evolution/background_review.rs
  - src/evolution/dreaming.rs
  - src/evolution/layer.rs
  - src/evolution/audit.rs
  - src/evolution/curator.rs
  - src/evolution/candidate.rs
  - src/evolution/draft.rs
  - src/evolution/merge.rs
  - src/evolution/patch.rs
  - src/evolution/review.rs
  - src/evolution/security.rs
  - src/evolution/recall.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/context.rs
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
  - docs/adr/0022-typed-llm-timeouts.md
  - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/adr/0037-eval-timeout-turn-settlement.md
  - docs/adr/0038-eval-trace-correlation-identity.md
supports: [behavior.llm-provider-execution, behavior.protocol-projection, behavior.eval-evolution, rule.provider-protocol-boundary, rule.protocol-role-separation, rule.quality-observation-boundary]
limitations:
  - Provider 模型事实会随外部服务变化；SDK intrinsic backlog 与产品 UI/backend 行为不是本 Evidence 的闭合目标
---

# Provider、Protocol 与 Quality 证据

## 支持的结论

LLM provider adapter、模型能力/预算/超时构成typed framework边界；ACP、A2A、Channels、Headless和SDK Host是不同入口投影；Trace/Eval/Improve的per-run workspace generation、AgentTurnDriver settlement、product/correlation/trace identity分离、未settled timeout retain与分层Evolution能力的已知行为和反例均可从本Evidence复核。

## 来源与范围

来源覆盖 LLM traits/capabilities/budget/tokenizer/provider 实现、ACP/A2A/Channel/Headless、SDK protocol/Host、Eval/Improve，以及 Background Review/Dreaming、memory、Skill lifecycle、安全检查等 Evolution 入口与相关 ADR/E2E。

## 已知缺口

外部 provider 的当前限制必须由消费方更新；全语言 intrinsic parity、发布和产品 surface 不纳入全 workspace baseline 完成条件。

## SDK watermark 的外仓 owner 结算（2026-10-20）

原header、observed_at、source_refs、supports、limitations和正文保留。F main1d的
ADR0051是ownership依据；SDK revision不充作F revision，外仓链接不进入F source_refs。

### repair 角色

固定SDKmain `09f670fbbde98bc44a7c710c6419a6ea0d7d469c` 的[Host ACK实现](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/echo-sdk-host/src/core_profile/events.rs#L70)、
[repair Evidence](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/.echo-semantic/evidence/evidence.sdk-gap-watermark-host-repair.md)
对应原缺陷：仅删除ACK推进一行，真实Host在snapshot2后重发live2，原严格断言fail；
恢复后live3/gapfrom4。是单变量机制反例，不称完整旧30d构建或旧Host三语言全量red。
不复制SDK实现或原始归档到framework，当前SDK tests分支不作本修复来源。

### verification 角色

[verification Evidence](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/.echo-semantic/evidence/evidence.sdk-gap-watermark-host-verification.md)、
[独立rereview](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/.echo-semantic/audits/audit.sdk-gap-watermark-host-rereview.md)及
[最终全语言验证段](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/.echo-semantic/evidence/evidence.sdk-media-accepted-route-repair-verification.md)
保各自review/runtime epoch。最终本地唯一完整入口含TS/Python/Java各12个真实Host
snapshot ACK/replay/live场景，不把阶段重跑相加。metadata-only后继main09f工程输入不变。

精main09f/tree80dd80d87b97a193a7c0b455d70a7c1c50df4739，
source9c1cee934443d0cef5c33b1a4e3d6e800e0d7eed6cdc6719b7ac760faeebc201，
[原始push CI39405806697](https://github.com/EchoYue-lp/echo-agent-sdk/actions/runs/39405806697)
九项SUCCESS。Linux完整语言入口0；Temurin17 Java quiet整体成功、无上传逐case XML，
本地详细12场景不改称CI逐case观察。Host/语言/依赖/冷恢复原件经独立reader按精来源复核。

09f独立冷仓仅GitHub main+35必要annotated refs，无alternates/shallow/本地对象或task/PR
导入；53个旧source epoch及1e1祖先恢复，共11819对象无缺失。SDK固定c0bc按真实base30d7
strict/change-evidence实际0，只证明SDK自身来源可恢复，不是F候选canonical PASS。
[SDK持久来源规则](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/docs/sdk/governance/semantic-source-provenance.md)
保各旧观察切点；09f这次具体读回由协调者原件补充，不把旧1e1段重标为新执行。

[Issue120闭合评论](https://github.com/EchoYue-lp/echo-agent/issues/120#issuecomment-6289991562)
与协调者issue120-external-close-proof.json（2578B，SHA256
`de4fad0da50b3a7238ec21e25009dbf7bbbd11492ef0bae003c392a90b7eacf4`）记录
2026-10-20T11:38:41Z closed/completed。该原件仅协调者ignored task local可用，
不作source_ref；公开评论和不可变SDK Git链接提供外部入口，local不是唯一事实源。

### 限制与采用门

此两个角色供历史F Finding的本仓typed引用；新Audit仅核owner/缺陷/三链/来源对应，
不宣布当前所有protocol surface无缺陷，不关闭四个A2A或managed未知，不接受风险。
本次重建以 F checkpoint `9c5b7043f040bfc12f326008dce2285502b5c62c` 的实际四文件为before；
ADR0051与F main1d字节相同，owner依据仍是该可恢复F源码，不使用SDK SHA作F revision。
本metadata-only候选只补历史账本。2026-10-21候选编制时，F9c5完整六门禁尚未
取得最终资格；这是编制时点记录，不是采用后状态。采用须Root依据届时实际资格
另行授权，并完成自身F引用/canonical/独立复审；后继实际资格由其受控证据/ref
承接，不把本准备重标为执行。最终main CI/cold须覆盖采用后快照。#120的历史
关闭依据仍是SDK09f原三链、九CI、cold及三语言各12场景，不归因于未来Fmain。
