---
schema_version: 1
id: evidence.current-control-history-retained-contracts-equivalence
kind: evidence
evidence_type: behavior_equivalence
title: Git684到当前格式的九项有限共同不变量
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
source_refs:
- echo-core/src/llm/types.rs
- echo-integration/src/providers/anthropic.rs
- echo-state/src/memory/conversation.rs
- src/agent/react/run/native_message_contract_tests.rs
- src/agent/react/run/phases/finalize.rs
- src/agent/react/run/phases/tools.rs
- src/agent/react/run/phases/verify.rs
- src/agent/react/run/stream_channel.rs
- src/agent/snapshot.rs
- src/state/current_native_recovery_tests.rs
- src/state/file.rs
- src/state/mod.rs
- src/state/sqlite.rs
supports:
- finding.current-control-history-retirement
- asset.agent-execution-contract
- asset.runtime-state-store
limitations:
- before仅真实Git684源码/39测试职责静态trace，没有before行为执行；after有原R15及当前正式F原命令，来源分别记录，未造before退出0。
- 有限matched只覆盖R1-R9当前合法格式共同不变量，T1-T6取消义务及C1-C4/Source8修复after-only分别记普通Evidence，不冒全行为相同。
- B336只读inspection不修scope属于T1退出；B1821兼容external-context getter属于T1退出，当前显式资源owner另证。
- U8 F1/F3独立磁盘组合未执行，不写actual；旧B936 raw归档proof退出，不新增任意unknown JSON永久成功CAS保留合同。
- 旧20路径0e4 Eq/Finding和旧source/raw全部保原；本新实体只覆盖本次两D，不扩其before/after/deletepaths。
- 4个ignored、真实外部provider/平台/SDK新pin/三语言/native/远端main仍独立未验。
- after_revision已按正常报告绑定Task；原source观察另保。TaskScope after_revision取原validate_task实际report.sourceSnapshot，不用manifest或自算task哈希；后续repair路由变化仍须重新按实际report复验。
before_revision: 684ad1d9d0de9bff57e27e4c04f04232c6af9f1f
after_revision: task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815
scenario_results:
  R1-custom同名无内置结束特权:
    status: matched
    source_refs:
    - src/state/current_native_recovery_tests.rs
    - src/agent/react/run/phases/tools.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::current_native_recovery_tests::new_custom_tools_named_think_and_final_answer_execute_as_ordinary_tools
    limitations:
    - 只保普通custom同名身份；旧builtin来源proof接纳属于T2，不匹配热替换所有排列。
  R2-严格配对与完成ID可复用:
    status: matched
    source_refs:
    - src/state/mod.rs
    - src/state/current_native_recovery_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::checkpoint_tests::checkpoint_restores_paired_tool_history
    - state::checkpoint_tests::checkpoint_rejects_unpaired_or_duplicate_tool_results
    - state::current_native_recovery_tests::current_unpaired_tool_cannot_bypass_strict_restore_or_become_final
    - state::current_native_recovery_tests::completed_tool_ids_can_be_reused_in_current_history_and_original_model_request
    limitations:
    - 只保普通配对、当前schema3完成ID复用及不补结果/不重放；旧来源归一与旧pending control接纳退出。
  R3-scope/version/epoch的typedCAS/replay围栏:
    status: matched
    source_refs:
    - src/state/file.rs
    - src/state/sqlite.rs
    - src/state/current_native_recovery_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::current_native_recovery_tests::current_cas_replay_and_stale_hydration_preserve_revision_and_epoch
    - state::file::tests::revisioned_cas_replays_and_fences_raw_mutators
    - state::sqlite::tests::revisioned_cas_replays_and_fences_raw_mutators
    - state::file::tests::owner_first_crash_cut_repairs_scope_revision
    limitations:
    - 匹配已定义scope/version/epoch、typed expected/replay、owner-first/stale围栏；旧全raw归档proof、readonly inspection零修scope不是此比较域。
  R4-合法context-only派生保source/cursor:
    status: matched
    source_refs:
    - src/agent/snapshot.rs
    - src/state/current_native_recovery_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::current_native_recovery_tests::current_context_view_derivation_keeps_cursor_lineage_and_original_source
    - state::current_native_recovery_tests::current_managed_cursor_corruption_is_rejected_without_owner_rewrite
    limitations:
    - 只保合法current context-view的cursor/source派生；normal load可reconcile，不等于旧readonly inspection；B336混合测试不整体matched。
  R5-pending债务与原proofACK:
    status: matched
    source_refs:
    - src/state/mod.rs
    - src/state/file.rs
    - src/state/sqlite.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::checkpoint_tests::pending_transcript_projection_round_trips_and_requires_managed_recovery
    - state::file::tests::pending_transcript_requires_proof_carrying_acknowledgement
    - state::sqlite::tests::pending_transcript_requires_proof_carrying_acknowledgement
    limitations:
    - 只保current pending债务与原proof ACK；不宣称旧专用recovery commit或全部电断点。
  R6-current准入physicalowner/resource/deadline:
    status: matched
    source_refs:
    - src/agent/react/run/stream_channel.rs
    - src/agent/snapshot.rs
    - src/state/current_native_recovery_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - state::current_native_recovery_tests::runtime_only_deadline_waits_for_actual_scope_load_runtime_load_and_cas_settlement
    - state::current_native_recovery_tests::current_normal_load_unknown_keeps_exact_owner_and_resources_without_fallback
    - state::current_native_recovery_tests::dropped_admitted_current_load_keeps_original_owner_until_physical_observation
    - state::current_native_recovery_tests::original_critic_failure_keeps_owner_through_metadata_save_and_late_cancel
    limitations:
    - 只保current已准入调用的原资源owner和绝对deadline；B1821旧external-context兼容getter及原queued组合属于T1，不冒after实跑。
  R7-候选不等成功与配置验证约束:
    status: matched
    source_refs:
    - src/agent/react/run/phases/verify.rs
    - src/agent/react/run/phases/finalize.rs
    - src/agent/react/run/native_message_contract_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - agent::react::run::native_message_contract_tests::native_message_guard_rejects_without_whole_output_leak
    - agent::react::run::native_message_contract_tests::native_message_structured_rejection_never_publishes_invalid_candidate
    - agent::react::run::native_message_contract_tests::native_message_configured_critic_veto_and_min_score_both_remain_authoritative
    - agent::react::run::native_message_contract_tests::native_message_critic_backend_failure_cannot_publish_success
    - agent::react::run::native_message_contract_tests::native_message_stop_continues_same_execution_and_publishes_success_once
    limitations:
    - 只保配置schema/Guard/Critic与第一次Stop约束；旧final候选接纳属于T4，完整terminal/EOF及lateerror顺序更强路径属于C2/C3。
  R8-完成普通effect冷恢复不重放:
    status: matched
    source_refs:
    - src/agent/react/run/native_message_contract_tests.rs
    - src/state/file.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - agent::react::run::native_message_contract_tests::native_message_cancel_after_completed_effect_then_cold_resume_does_not_replay
    limitations:
    - 仅当前fixture完成effect后File冷恢复总effect1；不宣称第三方外部effect普遍exactly-once。
  R9-当前合法Message内容与原顺序:
    status: matched
    source_refs:
    - echo-core/src/llm/types.rs
    - echo-integration/src/providers/anthropic.rs
    - echo-state/src/memory/conversation.rs
    - src/agent/react/run/native_message_contract_tests.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
    basis: 真实Git684共同不变量源码对照＋当前正式F原full05具名after；不是双侧同输入执行。
    actual_after_cases:
    - agent::react::run::native_message_contract_tests::native_message_file_conversation_reload_preserves_typed_blocks_and_native_reasoning
    - providers::anthropic::tests::signed_reasoning_blocks_are_replayed_before_tool_calls
    - providers::anthropic::native_replay_tests::provider_native_anthropic_signed_blocks_keep_order_and_replay
    limitations:
    - 只比较已声明合法Message内容、签名/opaque顺序；当前native phase/storage增强是C1，真实provider/native/全SDK未验。
command_results:
- command: /Users/ls/.cargo/bin/cargo +eko-native-1.99.1 --config /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.cargo/config.toml
    test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.echo-semantic/local/source8-repair-validation-20261030/formal-source8-r15/Cargo.toml
    --workspace --all-targets --all-features --locked
  exit_code: 0
  raw_sha256: 78c32bed0c98a437dc1a65358972bbe9aa741745cca75f719a7b9c26637b25d5
  receipt_sha256: fef960d74a62fc503bfb59cdbb2a152bba00c78aba47c2ecdd4f7a83a48032d1
  scope: R15原workspace/alltargets/allfeatures/locked，4215outerpassed/0failed/4ignored，不加child摘要；不是before执行
- command: /Users/ls/.cargo/bin/cargo +eko-native-1.99.1 test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml
    --workspace --all-targets --all-features --locked
  exit_code: 0
  raw_sha256: 3a7d09ced07f0ec86677443dc148c48a4c6344de6fb66ecffafb71c80b590f28
  receipt_sha256: 0af3e493187448cce819fe9fece24a84d304ff556f3752e4a2e17467b94c8e49
  scope: 当前正式F workspace/alltargets/allfeatures/locked，4262外层通过0失败4忽略；不是before运行，旧R15票保原。
deleted_paths:
- src/state/legacy_control_history.rs
- src/state/legacy_control_history/pending_recovery.rs
coverage:
- 仅Git684这一个before，两个具名删除路径与39条混合职责台账
- 仅R1-R9当前合法字段共同不变量；全部T/C分流在独立普通Evidence，取消场景无matched条目
- U7真实schema3两轮同ID→实际nextrequest纳R2；U8 typedCAS/epoch/ACK/digest/owner纳R3-R6，F1/F3未测限制保留
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 第一份Task报告绑定过程记录

原未改validate_task在23份采用后实际返回task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815。本Finding由已经审阅的Git684 before39职责分流、R1—R9有限共同不变量、当前formal full05具名after、原source-backed rereview承接；不是新增before运行或全行为相同。当前仅按本次两删除路径声明resolved，旧20路径Finding/Eq不改，取消义务和after-only/未验边界不参与matched。repair引用切换后必须按正常Task报告复验；本节不预填Source/final成功。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# 新限定保留合同

## 支持的结论

九项仅限逐场景声明的当前合法格式共同不变量。before是Git源码trace，after是原R15实际测试；这种有限源对照按独立a237充分性/source审查成立，不是形式化全等或旧兼容接受成功。

## 来源与范围

原source/taskcaller的身份与正常引用结果分列。当前原件位于本工作区 `.echo-semantic/local/source8-repair-validation-20261030/`，original24 qualified `cd912e627cbc1c662bf2522ff1d22237a41a1b3e6434d1906080d4113a534483`；完整R15 raw `78c32bed0c98a437dc1a65358972bbe9aa741745cca75f719a7b9c26637b25d5`。命令只记录真实after，before未跑不造0。

## 已知缺口

- before仅真实Git684源码/39测试职责静态trace，没有before行为执行；after仅原R15真实命令，未造before退出0。
- 有限matched只覆盖R1-R9当前合法格式共同不变量，T1-T6取消义务及C1-C4/Source8修复after-only分别记普通Evidence，不冒全行为相同。
- B336只读inspection不修scope属于T1退出；B1821兼容external-context getter属于T1退出，当前显式资源owner另证。
- U8 F1/F3独立磁盘组合未执行，不写actual；旧B936 raw归档proof退出，不新增任意unknown JSON永久成功CAS保留合同。
- 旧20路径0e4 Eq/Finding和旧source/raw全部保原；本新实体只覆盖本次两D，不扩其before/after/deletepaths。
- 4个ignored、真实外部provider/平台/SDK新pin/三语言/native/远端main仍独立未验。
