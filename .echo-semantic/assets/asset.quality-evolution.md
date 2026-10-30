---
schema_version: 1
id: asset.quality-evolution
kind: asset
title: Trace、Eval、Improve 与 Evolution
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.eval-evolution
code_refs:
- src/trace/mod.rs
- src/eval/runner.rs
- src/eval/comparator.rs
- echo-orchestration/src/runtime/turn_driver.rs
- src/agent/react/run/stream_channel.rs
- src/improve/mod.rs
- src/improve/loop.rs
- src/evolution/mod.rs
- src/evolution/background_review.rs
- src/evolution/dreaming.rs
- src/evolution/layer.rs
- src/evolution/mutation.rs
- src/evolution/runtime_integration.rs
- src/evolution/candidate.rs
- src/evolution/curator.rs
- src/evolution/draft.rs
- src/evolution/merge.rs
- src/evolution/patch.rs
- src/evolution/review.rs
- src/evolution/security.rs
- echo-state/src/skill_telemetry.rs
- docs/adr/0036-eval-workspace-generation-lifecycle.md
- docs/adr/0037-eval-timeout-turn-settlement.md
- docs/adr/0038-eval-trace-correlation-identity.md
- docs/adr/0065-evolution-memory-audit-reconciliation.md
- docs/adr/0068-skill-candidate-mutation-audit-reconciliation.md
consumer_refs:
- echo-agent-learning/tests/example_contracts/demo50_eval.rs
- echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
behavior_refs:
- behavior.eval-evolution
rule_refs:
- rule.quality-observation-boundary
evidence_refs:
- evidence.provider-protocol-quality
- evidence.persistence-observation
- evidence.improve-singleton-split-repair
- evidence.improve-singleton-split-verification
- evidence.improve-iteration-config-repair
- evidence.improve-iteration-config-verification
- evidence.eval-workspace-generation-repair
- evidence.eval-workspace-generation-verification
- evidence.eval-timeout-turn-settlement-repair
- evidence.eval-timeout-turn-settlement-verification
- evidence.eval-trace-correlation-repair
- evidence.eval-trace-correlation-verification
- evidence.evolution-memory-audit-repair
- evidence.evolution-memory-audit-verification
- evidence.memory-provenance-authority-repair
- evidence.memory-provenance-authority-verification
- evidence.skill-candidate-audit-repair
- evidence.skill-candidate-audit-verification
- evidence.skill-lifecycle-authority-verification
- evidence.evolution-doc-namespace-verification
- evidence.source8-quality-current-source-review
finding_refs:
- finding.eval-trace-identity
- finding.eval-timeout-settlement
- finding.eval-workspace-generation-isolation
- finding.improve-iteration-config
- finding.improve-single-case-panic
- finding.evolution-audit-atomicity
- finding.evolution-skill-promotion-audit
- finding.skill-candidate-reinforcement-audit-gap
- finding.evolution-doc-namespace
- finding.pre-compaction-memory-trust-provenance
candidate_refs: []
---

# 当前来源与有限父层接管

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

公开set_store/install_store现沿ADR0070原manager后端fence与context锁，在无await发布段同步Store/promoter；两个入口返回既有Result，raw与safe工具注册语义仍不同。原API before6为2通过4失败；修后原6、新6和原保留4共16在当前full05具名通过。未装manager的A到B、同Arc、busy/取消等待与拒绝后所有路由保持分别验证；SDK RPC Result消费另由SDK owner验收，不能当Rust telemetry无消费者。原Memory/Skill journal/Jsonl已审机制及有限未审组合边界完整保留，不用“全部故障”制造无限验收，也不把已存在P1改叫普通限制。

原父风险、完整code/consumer/behavior/rule/finding关系不缩减；以下准备段的needs_review/未取得等字样仅保原时点，当前可消费声明与上述具名修复按本节和机器observed_at。相邻Behavior/Rule、开放Finding、Discovery与真实未知不自动升级。

# 已冻结的r04准备记录原文

# Trace、Eval、Improve 与 Evolution

## 资产身份

执行观察、评估、离线改进与持久语义演化的quality pipeline；父层范围不因两项局部修复缩为Improve/telemetry。

## 来源与消费者

ReactAgent trace、Eval/Improve公开API、Evolution runtime integration和原examples继续消费。新增evidence.source8-quality-current-source-review只接管原独审、QE两修复与对应实际范围，旧全部Finding/refs继续保留；补录Memory/Skill lifecycle及namespace已有后继证据，不用旧10resolved代替新问题的修复。

## 生命周期

Trace per-run correlation只观测；Eval持一次drive/deadline/settlement后再按同parent/turn/execution查RunStore，未知不评分/不删generation。Improve把传入的同一可选RunStore透传既有EvalRunner，真实失败trace才能进Analyzer/critique/suggestion；None仍output-only，singleton/split/iteration边界不变。

分层Memory与Skill mutation继续由原manager/journal管理prepare→projection→idempotent audit→settle，批准身份不能由usage/模型评分补造；自动writer、activation/recall、lineage/CAS保原source/owner。Telemetry get/list错误现在原样传播，get失败不put覆盖旧聚合；runtime可选诊断warning不升为业务terminal或Skill approval。

## 候选关系

Quality observation不是业务commit，Evolution持久mutation需原独立审计；Rule仍HostOwned，host承担批准策略。保留可选cold/Store菜单，不因应用只用部分能力删除。

## 未知与限制

原observed_at733d352fc719f922b21bab1cd46206139564367f及“provenance/Draft/recall仅候选”是旧阶段观察，后继已有各自verification/rereview，不把这些历史完成再当当前待实现；原Raw Store和MEMORY.md直读可见Prepared中间投影是明确合同，settled读须manager/启动对账；ChangeLog不是跨文件原子可见性，observer无durable ACK且恢复不重放。

本候选继续needs_review。QE-I1/M1已具名真实RED/GREEN及source复审，R15全量原票仍不补齐父层真实未审的动态边界：Memory later rollback/热文件codec完整故障分支、Skill journal全部恢复/路径、JsonlRunStore并发/跨进程、自动writer与安装顺序的完整当前复审。最小后继是按原owner列这些精确调用/失败场景并定向复核，不将它们改称普通limitation后active；无需新增store/调度器。observed_at待F真实caller绑定，未验外部模型/第三方Store、物理断电、SDK/native、性能及main资格继续分列。

## R15父层新发现的Store安装阻断

Quality父报告C0/I1/M1（SHA `45162983812197df9fd9871908559ebcf54d660fcf841b451269e79c82e90e45`）确认公开`set_store(B)`和`install_store(B)`在manager(A)已安装后无条件发布B，而recall、promoter及记忆工具仍引用A。原ADR0070和既有set_memory_store/install_memory_store已经要求同后端；这是现有公开入口未执行同一合同，不是需要新增Store或跨进程机制。原Framework作者的Store修复已获source pass并固定进入after d702，包含两个setter及对应Result消费者；本稿准备时尚无本反例实际NoRun/RED/GREEN；不能继续泛化为安装顺序unknown后active。

原Memory rollback、codec、Skill journal及Jsonl路径已有实现；此前“全部故障/全部恢复未审”仅描述证据覆盖边界，不是待实现新功能清单或无限穷举验收。原24/fresh工程已限定通过，与本反例修复验证及source/final仍须分列。

## 本稿固定后继的资格分列

本稿初始取件基底为R15 `197cd657…`，当前四组修复固定组合为after manifest `d702396280f50ecc8138ccfbd53ae251061a6593451e91af90d337de2b98525f`，assembly `734021fa…`、执行计划 `1fba9fb4…`及其独立静态组合复核只证明准确准备与限定source范围。它们都是普通取件身份，不能填observed_at或冒task/source身份。

本稿准备时，原c01-before-01已进入编译的状态来自Root受控交接；本线不轮询/重启原operation，未取得可用于本后继绑定的实际NoRun自然0、实际test list或新反例RED/GREEN。函数清单是prepared selectors，编译/setup失败不能算行为RED；R15原24与fresh限定工程资格保原来源，不追给after d702。后续真实观察、稳定源工程与正常source/final绑定均交F sole，父status、风险、真实unknown、旧Eq/Finding/Discovery不由本材料改判。
