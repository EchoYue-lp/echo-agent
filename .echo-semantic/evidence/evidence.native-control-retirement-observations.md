---
schema_version: 1
id: evidence.native-control-retirement-observations
kind: evidence
title: 原生控制工具退役与保留合同的真实观察
observed_at: source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f
source_refs:
- docs/adr/0101-native-message-execution.md
- src/agent/react/mod.rs
- src/tools/builtin/mod.rs
- src/agent/react/run/native_message_contract_tests.rs
- src/agent/react/run/stream_channel.rs
- src/state/legacy_control_history.rs
- src/state/legacy_control_history/pending_recovery.rs
- echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
supports:
- finding.native-control-tools-retirement
- asset.agent-execution-contract
limitations:
- 本记录为真实after观察，不填behavior_equivalence或删除前后全API等价；两原源码仅由历史asset绑定。
- epoch08原profile实际证据按发布5908实施不变关系复用，不重标为新profile运行。
- 本候选尚未解决删除分支读取全部历史unresolved/needs_review的范围关联限制，也不是source通过收据。
command_results:
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 agent::react:: -- --test-threads=5 --nocapture'
  exit_code: 0
  receipt_sha256: 4bd14df2eb03657246394a0b5a1d0a9a093862d346a64f23bb5d26c0afd30171
  log_sha256: bad67368c3f91c3968367f269bf00598095b56f9cbaefd3c78be9efcf58b0eed
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 state::legacy_control_history::tests:: -- --test-threads=5'
  exit_code: 0
  receipt_sha256: 3da4fb824432fbeac19fcebc4cd1dd3e3170526c9cd178feff4fa6878454d0c7
  log_sha256: 56b3d0488d52b780ef9be1390af386a7e6f824a51624b975be21eca936a75890
- command: 'env CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/repair-epoch08/ts-rs
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo_agent
    --lib --locked -j 5 state::legacy_control_history::pending_recovery::tests:: -- --test-threads=5 --nocapture'
  exit_code: 0
  receipt_sha256: 79d3f4e581525d3ad67ed42323dd3f0cd56efd66d91801d43e52adcaf7e20bc6
  log_sha256: 8e5ab540a83c4c4626e1ced91424ffbcae9bd2a5a17b4e1e48eceaacd6682ebb
- command: 'env -u CARGO_PROFILE_TEST_OPT_LEVEL -u CARGO_PROFILE_TEST_DEBUG_ASSERTIONS -u CARGO_PROFILE_TEST_OVERFLOW_CHECKS
    CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/target
    CARGO_BUILD_JOBS=5 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/tmp
    TS_RS_EXPORT_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/source-epoch10/ts-rs
    env -u CARGO_PROFILE_TEST_OPT_LEVEL -u CARGO_PROFILE_TEST_DEBUG_ASSERTIONS -u CARGO_PROFILE_TEST_OVERFLOW_CHECKS
    cargo test --manifest-path /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/Cargo.toml -p echo-agent-learning
    --test example_contracts --no-default-features --features data,human-loop,media,testing,subagent --locked
    -j 5 -- --test-threads=1 --nocapture demo04_subagent:: demo12_resilience:: demo43_data_tools:: demo64_tool_pipeline::'
  exit_code: 0
  receipt_sha256: f10208a43c216760c6f682096895f1d69cf6b451311ef97b2a8dc80b9c617ad6
  log_sha256: 5c369979ed80c059130d4b71b50467eee9512b43c882b04fb6348c020679d881
---

# 两控制工具退出与after实际观察

## 支持的结论

已批准的旧实现/控制wire/默认FinalAnswer注册退出；原native43（在ReAct407内）实际
通过旧提案拒绝且不成功负控。Legacy17中think/final_answer自定义正控仍按普通工具执行。
Pending22保留未知/真实effect暂停、已核恢复经原CAS/完整raw保全/Guard/Stop及物理owner。
这五组退出/改正/新增面不标before/after matched，不称全部builtin数量为零。

## 来源与范围

具体当前源由source_refs绑定 `source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f`。原ReAct407/Legacy17/Pending22/File30/
Provider104和当前默认7的run/log/input SHA、原命令、成功用例行及profile见
`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028/after-actual-proof-index.json`。epoch08原test为unoptimized，默认7为optimized；
原完整输入对当前有9/17项差异，14观察条目是11个去重源，均与对应原运行input SHA相同。
默认7实际仍调用agent_dispatch，不把两退役builtin零注册偷换成全部builtin=0。

## 已知缺口

这是after实际观察，不是before运行，不宣称全API等价。旧Store不保签名，而当前
FileConversationStore冷重载Signed/Opaque blocks属于after增强；旧按名称终态、critic
预算接受、one-shot Stop与新增phase/status/model_streaming均单列有意变化。全source/
旧402/正式Gitpin与全部loaded needs_review仍独立。
