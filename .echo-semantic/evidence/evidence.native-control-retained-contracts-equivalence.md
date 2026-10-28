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
- src/state/legacy_control_history/pending_recovery.rs
supports:
- finding.native-control-tools-retirement
- asset.agent-execution-contract
limitations:
- 等价范围仅六个具名保留不变量；before为实际Git源码trace，after为原真实运行，未声称before测试通过。
- 五组具名退出/修正/新增面在单独普通观察Evidence/Finding记录，绝不列为matched。
- 原null草案阶段不能提交；本轮已用实际官方validate_task report.sourceSnapshot绑定，仍不授予全source或Gitpin资格。
- epoch08实际test unoptimized；当前默认7实际test optimized；保持原输入与profile边界，不宣称新profile全跑。
before_revision: 0e404185c8f252da3e335c35547e52249c74dc65
after_revision: task:355a8528f1bcecb7b72f14646f487bdbfb6f53d0e8eb86926b65c7962d9c66ff
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
    - src/state/legacy_control_history/pending_recovery.rs
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
coverage:
- 公开execute/chat的String返回形式保留；不主张before已有新增model_streaming开关
- Signed/Redacted reasoning在Message JSON中表示且由Anthropic provider保签名回放；不主张旧ConversationStore持久化签名
- 最终候选继续受已配置schema与output guard约束，拒绝候选不能作为合法最终答案
- 首次Stop要求继续时返回同一主循环Continue，不就此发送成功FinalAnswer
- 既有File Store按scope/runtime加锁，expected revision/epoch冲突拒绝覆盖，replay复用原CAS
- 普通工具仍经同一pipeline以call_id/name/结果成对写入历史并发送ToolResult
deleted_paths:
- src/tools/builtin/think.rs
- src/tools/builtin/answer.rs
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
