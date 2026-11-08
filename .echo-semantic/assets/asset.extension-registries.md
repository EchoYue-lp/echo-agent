---
schema_version: 1
id: asset.extension-registries
kind: asset
title: MCP、Hook、Skill 与 Plugin 当前注册权威
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.extension-lifecycle
code_refs:
- echo-integration/src/mcp/mod.rs
- echo-execution/src/skills/hooks.rs
- echo-execution/src/skills/registry.rs
- echo-core/src/plugin/registry.rs
- src/plugin/coordinator.rs
- src/plugin/prepared.rs
- src/agent/react/mod.rs
- echo-core/src/plugin/lifecycle.rs
consumer_refs:
- src/agent/react/capabilities.rs
behavior_refs:
- behavior.extension-publication
rule_refs:
- rule.extension-generation-authority
evidence_refs:
- evidence.effects-extensions
- evidence.plugin-generation-publication-authority-repair
- evidence.plugin-generation-publication-authority-verification
- evidence.plugin-lifecycle-coordinator-repair
- evidence.plugin-lifecycle-coordinator-verification
- evidence.extension-cleanup-settlement-verification
- evidence.source8-extension-current-source-review
- evidence.extension-lsp-manager-historical-source
finding_refs:
- finding.skill-activation-authority
- finding.hook-permission-precedence
- finding.mcp-client-capability-advertisement
- finding.plugin-mcp-owner-isolation
- finding.plugin-failure-isolation-contract
- finding.plugin-lifecycle-coordination
- finding.lsp-runtime-state
- finding.extension-cleanup-settlement
- finding.mcp-version-doc-drift
candidate_refs: []
---

# 当前来源与有限父层接管

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

Command Hook四路I/O以原Child和同绝对动作deadline驱动，try_join fail-fast而stdin writeErr仍按旧合同忽略；超时/错误走原可await cleanup。before5为3通过2失败，修后5在focused和当前whole通过；旧1秒超时约4014ms晚成功和8秒duplex观察guard失败分别保，不把drop救援当原cleanup。当前原helper child.wait无独立硬cleanup期限，不能宣称总耗时硬界。调用方补已有ECHO_HOOK_DEADLINE_TEST_ROOT至本轮W，旧2c4污染5PID和source_drift/101原件留原，Hook源和断言未改。四类registry/owner菜单继续原权威，LSP独立历史Evidence保原Git来源。

原父风险、完整code/consumer/behavior/rule/finding关系不缩减；以下准备段的needs_review/未取得等字样仅保原时点，当前可消费声明与上述具名修复按本节和机器observed_at。相邻Behavior/Rule、开放Finding、Discovery与真实未知不自动升级。

# 已冻结的r04准备记录原文

# MCP、Hook、Skill 与 Plugin 的当前注册权威

## 资产身份

四类存续extension各自负责发现/命名、activation/publication、owner/generation和撤销；LSP仅保具名批准退役的历史来源，不再作为当前第五个registry。

## 来源与消费者

ReactAgent capabilities、PluginIntegrator、独立SDK Host与public API按原边界消费。每个ReactAgent持有自身target-scoped plugin publication和cleanup receipt；MCP owner快照只证明连接归属，不是工具执行许可。旧跨仓生产消费者表完整保历史，当前SDK关系不自动取得新pin/Host资格。新增当前限定审查及历史LSP来源分别由evidence.source8-extension-current-source-review和evidence.extension-lsp-manager-historical-source承接。

## 生命周期

Skill主registry/adapter/snapshot/checkpoint共用原activation handle及epoch/generation，single-flight与取消unknown poison不另建状态。Hook按原source/event reducer合成deny/ask/require-approval/allow，source executor/cwd不回退host；非permission stop与permission优先级分开。

MCP以typed owner identity索引active/preparing/prepared/debt，Direct与不同Plugin同名隔离；连接前登记可等待preparation owner，失败close保debt/retry。PluginRegistry、PreparedPluginSet/Integrator、LifecycleManager及Coordinator各自持有原desired/actual/callback/wiring权威；prepare不可变、target绑定与exact receipt，失败ActualPending不伪rollback成功。关闭先停准入并等原in-flight/MCP/sandbox owner，不把Drop日志当已结算。

## 候选关系

本父资产只组织跨extension合同，不合并多个独立authority，也不把channel等相邻Rule范围一起关尽。LSP runtime实现、feature和旧配置probe/read/freeze/hash/cache已按批准退出；不以旧阶段承诺复活reader或服务。

## 未知与限制

原observed_at733d352fc719f922b21bab1cd46206139564367f及“#55等待完整门禁”的原阶段记录保留；已完成的原verification/rereview可按其实际本地范围接管，PR/远端main本次未新核。原候选拟active的建议已由当前Hook超时源码反例撤回；保持needs_review；Hook r03已获source-only pass并固定进入after d702，仍待实际具名验证、稳定源正式接管及合法observed_at绑定。

原LSP code_ref已移入独立历史Evidence，保原733d Git/blob、原Finding/verification/rereview和退出依据；它不被当前machine code_refs假装存在。durable Hook delivery #58、channel/host整合与其它开放义务不由本资产关闭。外部MCP互操作、强杀完成清理、第三方transport/factory兑现、当前SDK Host/三语言、EKO fan-out/native与真实网络未验；这些是范围边界，任何新发现当前owner/dynamic缺口仍须needs_review处理。

## 原跨仓生产消费者承接（历史原文保全）



ADR0051提取后的SDK生产消费者由独立SDK拥有；原F观察、状态、全部业务限制和既有正文保持。consumer_refs仅列原F消费者，不把外仓路径误作本F revision中的文件。

| 原F定位与角色 | 实际SDK承接 | 不可变源码 | Git blob |
|---|---|---|---|
| `echo-sdk-host/src/core_profile/facade/integrations.rs`，F观察 `733d352fc719f922b21bab1cd46206139564367f`，production_source | SDK `map.sdk-integrations`；boundary.sdk-integrations; behavior.sdk-family-mcp / behavior.sdk-family-lsp | [SDK生产源码](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/echo-sdk-host/src/core_profile/facade/integrations.rs)；[既有Map](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/.echo-semantic/maps/map.sdk-integrations.md) | `b38cae06325cffc20b3d9d883070f957d5d20cf8` |

上述生产文件在既有Map观察、公开SDK `7f14a1b9c751b0612ceb42224f83f29d09aea46c`、SDK `019e700279677a2eb77e265b5038d9d581bc22da`及纠正时工作树均为相同字节。该表保留原生产角色和外仓Owner，属于跨仓归属与原件定位说明；当前单仓合同没有跨仓强引用解析器，不宣称正文链接构成新增机器强边，也不宣称519项连续性已通过。

## 当前Hook超时阻断

Tools父层最终审查I2适用于本父Asset现存Hook边：无sandbox命令先await完整stdin写入，随后才建立wait timeout。大合法工具输入与不读stdin的命令可以越过配置timeout，故属于现成Hook生命周期合同缺陷；不属于第三方能力或平台unknown。原四类机制有限源码pass保持历史范围，不能消除这个后发现义务；由IM指定作者处理的Hook r03已有源码修正与source-only pass，after d702固定准备；本稿未取得新行为验证或物理结算证据，不保持active提案。

## 本稿固定后继的资格分列

本稿初始取件基底为R15 `197cd657…`，当前四组修复固定组合为after manifest `d702396280f50ecc8138ccfbd53ae251061a6593451e91af90d337de2b98525f`，assembly `734021fa…`、执行计划 `1fba9fb4…`及其独立静态组合复核只证明准确准备与限定source范围。它们都是普通取件身份，不能填observed_at或冒task/source身份。

本稿准备时，原c01-before-01已进入编译的状态来自Root受控交接；本线不轮询/重启原operation，未取得可用于本后继绑定的实际NoRun自然0、实际test list或新反例RED/GREEN。函数清单是prepared selectors，编译/setup失败不能算行为RED；R15原24与fresh限定工程资格保原来源，不追给after d702。后续真实观察、稳定源工程与正常source/final绑定均交F sole，父status、风险、真实unknown、旧Eq/Finding/Discovery不由本材料改判。
