---
schema_version: 1
id: asset.framework-docs
kind: asset
title: Framework 正式文档与 ADR
asset_type: document
status: active
risk: medium
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.workspace-architecture
code_refs:
- README.md
- README.zh.md
- docs/en/README.md
- docs/zh/README.md
- docs/en/architecture.md
- docs/zh/architecture.md
- docs/en/concepts.md
- docs/zh/concepts.md
- docs/en/lifecycles.md
- docs/zh/lifecycles.md
- docs/en/24-eval-system.md
- docs/zh/24-eval-system.md
- docs/en/29-long-running-tasks.md
- docs/zh/29-long-running-tasks.md
- docs/adr/0014-framework-capability-placement.md
- docs/adr/0033-subagent-factory-singleflight-publication.md
- docs/adr/0034-context-scoped-tool-result-cache.md
- docs/adr/0035-owned-tool-registry-handles.md
- docs/adr/0036-eval-workspace-generation-lifecycle.md
- docs/adr/0037-eval-timeout-turn-settlement.md
- docs/adr/0038-eval-trace-correlation-identity.md
- docs/adr/0039-background-task-terminal-authority.md
- docs/adr/0040-framework-concept-documentation-authority.md
- docs/adr/0041-semantic-governance-continuity.md
- docs/adr/0101-native-message-execution.md
- docs/adr/0103-current-storage-records.md
- docs/adr/0104-canonical-agent-runtime-kernel.md
- docs/adr/0105-current-config-and-plugin-inputs.md
consumer_refs:
- echo-agent-learning/tests/documentation_contract.rs
behavior_refs:
- behavior.workspace-composition
rule_refs:
- rule.framework-layer-ownership
evidence_refs:
- evidence.workspace-structure
- evidence.eval-timeout-turn-settlement-verification
- evidence.eval-trace-correlation-verification
- evidence.background-task-terminal-authority-verification
- evidence.workspace-topology-doc-repair
- evidence.workspace-topology-doc-verification
- evidence.feature-table-doc-repair
- evidence.feature-table-doc-verification
- evidence.readme-example-target-repair
- evidence.readme-example-target-verification
- evidence.framework-concept-navigation
- evidence.evolution-doc-namespace-verification
- evidence.source8-docs-current-source-review
finding_refs:
- finding.workspace-topology-doc-drift
- finding.public-feature-table-drift
- finding.readme-example-target-drift
- finding.mcp-version-doc-drift
- finding.evolution-doc-namespace
- finding.tool-pipeline-example-drift
candidate_refs: []
---

# 当前来源与有限父层接管

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

g02两README已准确修正Ollama兼容三wire入口与await/Option快照示例，未新增第四adapter；Git684查源与原文稿源审分开保留。当前all-targets编译示例、documentation_contract及独立全feature doctest通过，后者不调用真实模型。原24引用的当前菜单/历史职责和原bounded源审继承，SDK迁仓、旧执行原身份、网站clean-source、native、发行资格分列，不从结构测试推出所有自然语言或平台全验。

原父风险、完整code/consumer/behavior/rule/finding关系不缩减；以下准备段的needs_review/未取得等字样仅保原时点，当前可消费声明与上述具名修复按本节和机器observed_at。相邻Behavior/Rule、开放Finding、Discovery与真实未知不自动升级。

# 已冻结的r04准备记录原文

# Framework 正式文档与 ADR

## 资产身份

Framework当前公共概念、API、架构决策、examples路由和长期说明；保原24份code_refs，不以一条测试证明全文自然语言准确。

## 来源与消费者

Framework用户消费本仓正式中文文档与ADR；独立SDK拥有Host、纯Protocol及三语言accepted合同，完整Rust inventory只作非阻断漂移telemetry。网站从合格clean Git来源镜像，不拥有产品行为；历史英文/原SDK同仓/旧feature计数保原source，不能反向扩当前产品承诺。

## 生命周期

API/架构变化须与实现、examples和测试同步。当前原生thinking由provider/model配置，不再强注入工具前CoT；旧LSP用户文件仅保原件，当前prepare不probe/read/freeze/hash/cache、不进generation、不产LSP专属诊断。当前关键决策由原ADR0101/0103/0104/0105与所属chapter解释，新增引用只连现有源，不改历史ADR原阶段事实。

## 候选关系

顶层工程计划/历史审计不是本仓长期行为authority；docs描述Framework机制，EKO/SDK各管产品/协议接受面。不能为文案一致复活LSP/旧控制工具、删合理pub API或新建另一执行路径。

## 未知与限制

原observed_at44b2ed68772c7c016d09af6c2e1adac9fe4fea70及“已形成文档Finding”是旧观察；原六Finding按各自verification/rereview完成的范围可接管，不拿六resolved或documentation_contract15作24份全文语义通过。

本候选继续needs_review。R15后继独审已确认原LSP/CoT指定冲突闭合，中文索引“退役／历史恢复”中的旧诊断词只导航历史章，不再认领当前prepare诊断；047a22对索引的待确认问题只保为原阶段观察，不再作为当前独立阻断。下节保留两项R15公开菜单/示例反例；它们已经g02两README正式采用在文稿source层闭合，不能继续写成未修。此有限闭合不授原24refs父层Source0或运行资格。

最小后继是在最终稳定源对原24份的当前/历史角色和修后增量定向复审，保下列g02已采用文稿与各代码owner修复合同的精确对应，仍保raw Store中间态、caller-owned background、独立SDK/newpin、真实native/网站clean-source/发行未验。完整工程原票和源审/正式source/full24/fresh依赖、mainCI分别验收；observed_at仍待F正常caller合法来源，不预填manifest/task，也不关闭其它父Asset/Discovery。

## R15两项文稿反例与g02限域source闭合

独立报告framework-docs-parent-postfix-R15.md（SHA256 e99a79f80348f9a18d1268411c8f0a4204b89c0e27c64cfe2911182889a67f70）结论C0/I2/M0 block。两项均是当时可复制/可依赖说明的错误，不是新增provider功能请求，也不因未测全部native而泛阻：

- R15-FD-I1：README.zh.md:775/783与README.md:1052/1060仍承诺内置“原生Ollama”；实际LlmApiProtocol/config/client只实现ChatCompletions、Responses、Anthropic三种wire，裸root不会自动切出Ollama-native adapter。应由原文稿writer准确描述按实际兼容协议/端点接入，保合理自定义LlmClient；本包不新增第四adapter或替任何服务端URL签支持资格。
- R15-FD-I2：README.zh.md:674–679把snapshot()/rollback()/rollback_to()写为同步Result加?，实际react/mod.rs:1855/1870/1889为async Option。原英文现有正确await/Option控制流可作最小文稿参考，不能用15个文档结构测试或4段learning no_run声称该中文fence已编译。

这两项原先存在于R23/R15未改正文，旧block报告保原时点。后继两README冻结文稿已由独立postfix审查f0866112限定source pass，g02正式采用记录65f74e25确认仅两README准确落地，正常pre-record、两个path检查与post-record均为原自然0；本线只回读原件，未重跑。g02明确未绑定本轮当前source/final身份，示例/文档focused实际验证尚待统一窗口；不能据此称原24refs整父Source0、原Finding转态或新运行通过。本增量不热写197、不改README/API/配置，也不造Audit/resolved收据。

## 本稿固定后继的资格分列

本稿初始取件基底为R15 `197cd657…`，当前四组修复固定组合为after manifest `d702396280f50ecc8138ccfbd53ae251061a6593451e91af90d337de2b98525f`，assembly `734021fa…`、执行计划 `1fba9fb4…`及其独立静态组合复核只证明准确准备与限定source范围。它们都是普通取件身份，不能填observed_at或冒task/source身份。

本稿准备时，原c01-before-01已进入编译的状态来自Root受控交接；本线不轮询/重启原operation，未取得可用于本后继绑定的实际NoRun自然0、实际test list或新反例RED/GREEN。函数清单是prepared selectors，编译/setup失败不能算行为RED；R15原24与fresh限定工程资格保原来源，不追给after d702。后续真实观察、稳定源工程与正常source/final绑定均交F sole，父status、风险、真实unknown、旧Eq/Finding/Discovery不由本材料改判。
