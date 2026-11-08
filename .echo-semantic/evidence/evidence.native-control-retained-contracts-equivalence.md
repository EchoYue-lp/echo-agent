---
schema_version: 1
id: evidence.native-control-retained-contracts-equivalence
kind: evidence
evidence_type: behavior_equivalence
title: 仅保留合同的限定来源对账
observed_at: source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f
source_refs:
- echo-core/src/agent/mod.rs
- echo-core/src/llm/types.rs
- echo-integration/src/providers/anthropic.rs
- src/agent/react/run/native_message_contract_tests.rs
- src/agent/react/run/phases/finalize.rs
- src/agent/react/run/phases/tools.rs
- src/agent/react/run/phases/verify.rs
- src/agent/react/run/pipeline.rs
- src/agent/react/run/stream_channel.rs
- src/state/file.rs
evidence_refs:
- evidence.current-control-history-before-source
supports:
- finding.native-control-tools-retirement
- asset.agent-execution-contract
limitations:
- 等价范围仅六个具名保留不变量；before为实际Git源码trace，after为原真实运行，未声称before测试通过。
- 五组具名退出/修正/新增面在单独普通观察Evidence/Finding记录，绝不列为matched。
- 原null草案阶段不能提交；本轮已用实际官方validate_task report.sourceSnapshot绑定，仍不授予全source或Gitpin资格。
- epoch08实际test unoptimized；当前默认7实际test optimized；保持原输入与profile边界，不宣称新profile全跑。
before_revision: 0e404185c8f252da3e335c35547e52249c74dc65
after_revision: task:99e4f0c8c8bef3b3fbb171f3cb4b645e39ac1f3f98d67b715963c4ead0ff3905
scenario_results:
  string-return-form:
    status: matched
    source_refs:
    - echo-core/src/agent/mod.rs
    - src/agent/react/run/native_message_contract_tests.rs
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test agent::react::run::native_message_contract_tests::native_message_explicit_model_transport_preserves_return_form_and_contexts
      ... ok
    limitations:
    - 只对String返回形式；新增model_streaming配置、native事件/wire及phase字段均不在before/after等价范围。
  signed-reasoning-replay:
    status: matched
    source_refs:
    - echo-core/src/llm/types.rs
    - echo-integration/src/providers/anthropic.rs
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test providers::anthropic::tests::signed_reasoning_blocks_are_replayed_before_tool_calls ... ok
    - test providers::anthropic::native_replay_tests::provider_native_anthropic_signed_blocks_keep_order_and_replay
      ... ok
    limitations:
    - 0e4 ConversationStore projection仅reasoning_content，无reasoning_blocks；当前FileConversationStore冷重载/typed blocks持久化增强只列after-only，绝不声称before签名同等保留。
  schema-output-guard:
    status: matched
    source_refs:
    - src/agent/react/run/phases/verify.rs
    - src/agent/react/run/stream_channel.rs
    - src/agent/react/run/native_message_contract_tests.rs
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test agent::react::run::native_message_contract_tests::native_message_guard_rejects_without_whole_output_leak
      ... ok
    - test agent::react::run::native_message_contract_tests::native_message_structured_rejection_never_publishes_invalid_candidate
      ... ok
    limitations:
    - 旧回调/SessionEnd早晚顺序的缺陷修复另列；只证明指定验证约束继续存在。
  first-stop-continuation:
    status: matched
    source_refs:
    - src/agent/react/run/phases/finalize.rs
    - src/agent/react/run/native_message_contract_tests.rs
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test agent::react::run::native_message_contract_tests::native_message_stop_continues_same_execution_and_publishes_success_once
      ... ok
    limitations:
    - 旧one-shot Stop上限与新重复Stop行为不等价；这里只比较第一次Continue。
  cas-revision-fence:
    status: matched
    source_refs:
    - src/state/file.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test state::file::tests::revisioned_cas_replays_and_fences_legacy_mutators ... ok
    limitations:
    - 新已核控制pending恢复和raw archive是新增适配；不宣称旧pending控制恢复策略未变。
  ordinary-tool-pairing:
    status: matched
    source_refs:
    - src/agent/react/run/phases/tools.rs
    - src/agent/react/run/pipeline.rs
    - src/agent/react/run/native_message_contract_tests.rs
    before_evidence_ref: evidence.native-control-retirement-before-source
    basis: before source trace + original actual after tests; limited invariant only
    actual_after_cases:
    - test agent::react::run::native_message_contract_tests::native_message_final_with_tool_executes_tool_then_requests_again
      ... ok
    - test agent::react::run::native_message_contract_tests::native_message_multiple_messages_and_parallel_tools_keep_order_and_identity
      ... ok
    limitations:
    - 不把旧保留名final_answer的特殊终态行为当作保留合同；该名称旧语义退出见intentional-changes。
command_results:
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 agent::react:: -- --test-threads=5 --nocapture'
  exit_code: 0
  run_sha256: 4bd14df2eb03657246394a0b5a1d0a9a093862d346a64f23bb5d26c0afd30171
  log_sha256: bad67368c3f91c3968367f269bf00598095b56f9cbaefd3c78be9efcf58b0eed
  profile:
  - '    Finished `test` profile [unoptimized] target(s) in 0.14s'
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 state::legacy_control_history::tests:: -- --test-threads=5'
  exit_code: 0
  run_sha256: 3da4fb824432fbeac19fcebc4cd1dd3e3170526c9cd178feff4fa6878454d0c7
  log_sha256: 56b3d0488d52b780ef9be1390af386a7e6f824a51624b975be21eca936a75890
  profile:
  - '    Finished `test` profile [unoptimized] target(s) in 0.38s'
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 state::legacy_control_history::pending_recovery::tests:: -- --test-threads=5 --nocapture'
  exit_code: 0
  run_sha256: 79d3f4e581525d3ad67ed42323dd3f0cd56efd66d91801d43e52adcaf7e20bc6
  log_sha256: 8e5ab540a83c4c4626e1ced91424ffbcae9bd2a5a17b4e1e48eceaacd6682ebb
  profile:
  - '    Finished `test` profile [unoptimized] target(s) in 28.76s'
- command: 'env -u CARGO_PROFILE_TEST_OPT_LEVEL -u CARGO_PROFILE_TEST_DEBUG_ASSERTIONS -u CARGO_PROFILE_TEST_OVERFLOW_CHECKS
    CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/ts-rs
    env -u CARGO_PROFILE_TEST_OPT_LEVEL -u CARGO_PROFILE_TEST_DEBUG_ASSERTIONS -u CARGO_PROFILE_TEST_OVERFLOW_CHECKS
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo-agent-learning
    --test example_contracts --no-default-features --features data,human-loop,media,testing,subagent --locked -j
    5 -- --test-threads=1 --nocapture demo04_subagent:: demo12_resilience:: demo43_data_tools:: demo64_tool_pipeline::'
  exit_code: 0
  run_sha256: f10208a43c216760c6f682096895f1d69cf6b451311ef97b2a8dc80b9c617ad6
  log_sha256: 5c369979ed80c059130d4b71b50467eee9512b43c882b04fb6348c020679d881
  profile:
  - '    Finished `test` profile [optimized] target(s) in 4m 34s'
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 state::file:: -- --test-threads=5'
  exit_code: 0
  run_sha256: 83ee260a0d31aaddcba4ecd18ea8d658ab95372f673876593c1865948c0a53d0
  log_sha256: 24753cea71d1ad4e36efdfa90483b8268ce072a077064b0a64068012fc7674cf
  profile:
  - '    Finished `test` profile [unoptimized] target(s) in 0.14s'
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_integration
    --lib providers:: --locked -j 5'
  exit_code: 0
  run_sha256: 844303c26df38932a69b6569bc77570e24d51f096f4c6f49f4b592fade0df593
  log_sha256: c2840a12a3c2829ffd46f67a1d04164a8b9c4a1d4cddff4b11d7894a29974b72
  profile:
  - '    Finished `test` profile [unoptimized] target(s) in 9.77s'
- command: cargo test --workspace --all-targets --all-features --locked
  exit_code: 0
  source_input_sha256: 0e598041ce1434429ebdfe37b8cfcdb5f4e39d3f6365cf5e86e9e5428b3b48e5
  log_sha256: f0b828c53088ba39299567a5c62d76a54abe2b22c0596f12a1e319a7af35a84a
  receipt_scope: actual current all-workspace/all-target/all-feature execution;3957
    top-level pass+2 preexisting child controls,0fail,4existing ignored; no whole
    runtime equivalence claimed
coverage:
- 公开execute/chat的String返回形式保留；不主张before已有新增model_streaming开关
- Signed/Redacted reasoning在Message JSON中表示且由Anthropic provider保签名回放；不主张旧ConversationStore持久化签名
- 最终候选继续受已配置schema与output guard约束，拒绝候选不能作为合法最终答案
- 首次Stop要求继续时返回同一主循环Continue，不就此发送成功FinalAnswer
- 既有File Store按scope/runtime加锁，expected revision/epoch冲突拒绝覆盖，replay复用原CAS
- 普通工具仍经同一pipeline以call_id/name/结果成对写入历史并发送ToolResult
deleted_paths:
- echo-agent-learning/examples/demo23_a2a.rs
- echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs
- echo-core/src/lsp/client.rs
- echo-core/src/lsp/mod.rs
- echo-core/src/lsp/types.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/config.rs
- echo-integration/src/lsp/jsonrpc.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/lsp/mod.rs
- src/a2a/auth.rs
- src/a2a/client.rs
- src/a2a/mod.rs
- src/a2a/serve.rs
- src/a2a/server.rs
- src/a2a/types.rs
- src/lsp.rs
- src/tools/builtin/answer.rs
- src/tools/builtin/think.rs
- src/tools/lsp.rs
---

# 六个限定保留合同的来源与实际观察

## 支持的结论

仅对六个共同不变量matched：String返回形式；Message JSON/Anthropic签名payload回放；
已配置schema/output guard；首次Stop继续；File CAS revision fence；普通工具call_id/
name/结果成对投影。before为实际Git源码trace，after为原真实运行，不声称before运行。
五组具名退出/修正/新增不写matched，尤其旧Store签名持久化不在等价范围。

## 来源与范围

before_revision为 `0e404185c8f252da3e335c35547e52249c74dc65`，before源码由独立before-source Evidence绑定历史Git；
本source_refs绑定当前 `source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f`。每场景before行锚、after case、原run/log/input SHA与
原profile详见`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028/retained-contract-matrix.json`。14个观察条目/11去重源的当前
字节均等于各原运行输入；这不表示1490整包或新test profile与原票完全相同。

## 已知缺口

after_revision取自本轮未改官方validate_task的实际完整闭包report.sourceSnapshot；
原null阶段失败与schema补齐原件保留，本轮绑定不使用source692或发布索引SHA5908。原before
运行并未执行；如需要双版本运行，该事实不能由源码trace替代。本Evidence不消除原#10
全部loaded needs_review聚合，也不授予全source/Gitpin/旧402通过资格。


## 当前维护切点的绑定与限定边界

原observed_at、六个scenario_results、原command_results与原sourceRefs保留历史观察，
不是把旧来源改称当前重跑。现追加的是实际0e598稳定输入的whole workspace test
原票（3957顶层+2既有子控制、0fail/4原ignored）；六个具名共同不变量的实际case
都在该原log中找到通过行。String、provider签名回传、验证拒绝、首次Stop、通用CAS
fence及普通工具结果配对继续是限定matched对象；A2A/LSP退出面、旧控制工具退出、
NoCompat取消的旧兼容义务或未完成新功能均不写matched。

deleted_paths更新只覆盖原审批20路径的source准入，不增加scenario等价声明。
after_revision提案值由原未改validate_task失败report的真实sourceSnapshot算法复现，
再仅加入新历史asset对18具名退出源的0e4 Git引用得到；没有checker overlay或排除。
Root正式metadata采用后必须用原validate_task实际确认该值，任何不同仍自然失败并诊断。

本提案不将旧10、402或本轮45项失败改成0，90个loaded未知refs不在此关闭；
SDK newPin、native/MainCI/Goal仍另验。before源码不冒before执行；after绑定更新不
意味着所有新旧runtime行为相同。


## 恢复后的实际当前增量绑定

当前HEAD e0cda8f95e6695a99c04ded2285057b5fa68d9f3仅保存原WIP，原迁移before仍为0e4，
本Evidence before_revision与原observed_at不改。原未改validate_task在当前HEAD正常
预检后实际返回当前metadata增量sourceSnapshot `task:99e4f0c8c8bef3b3fbb171f3cb4b645e39ac1f3f98d67b715963c4ead0ff3905`，该值用作
当前after绑定；不是沿用旧base下035d提案值。当前task无新增产品删除，不能把这个
增量结果解释成原完整删除45→0、外部#10/旧402或runtime全等通过。

0.5.0原preflight.py记录强取gitHEAD，preflight_contract.py要求baseRevision==currentHEAD；
锁内record-preflight再次核验。因此HEAD已保存后，正常预检无法同时把原Outcome
0e4当currentbase。本轮如实记录该外部base/HEAD绑定限制，不改工具、不换HEAD、
不造源码副本、不清unknown；完整历史删除与保留面资格继续保原失败/已验证边界。

## 2026-11-01：退役源码的历史关联入口

顶层及 `cas-revision-fence` 中的 pending 源码关联迁入既有 `evidence_refs`，由
`evidence.current-control-history-before-source` 按其自身 Git
`684ad1d9d0de9bff57e27e4c04f04232c6af9f1f` 读取历史原件。
该路径不再被非 Git `observed_at` 解释为当前工作树源码；原 CAS 场景仍直接引用
`src/state/file.rs`，仍只比较既有 revision/epoch fence 共同不变量。

| 原关联位置 | 原关联路径 | 现有历史读源 owner |
| --- | --- | --- |
| 顶层 `source_refs` | `src/state/legacy_control_history/pending_recovery.rs` | `evidence.current-control-history-before-source` / Git684 |
| `cas-revision-fence.source_refs` | `src/state/legacy_control_history/pending_recovery.rs` | 同一 owner / Git684；场景限制仍将 pending 恢复与 raw archive 列为新增适配 |

这只调整历史关联的读取入口。原 `observed_at=source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f`、
`before_revision`、`after_revision=task:99e4f0c8c8bef3b3fbb171f3cb4b645e39ac1f3f98d67b715963c4ead0ff3905`、
六个场景的名称、matched 状态、basis、case、limitations、命令、原结果与 profile 均保持。
旧六项 Eq 不续签为当前行为验收，不以 current-control 九项 Eq 取代它，也不把两文件之外的
旧工程输入整体重绑到 Git684。原源码关联与本次迁移关系并存于上述表和保全原件中。
