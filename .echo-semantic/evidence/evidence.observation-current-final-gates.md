---
schema_version: 1
id: evidence.observation-current-final-gates
kind: evidence
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
source_refs:
  - echo-core/src/agent/mod.rs
  - echo-core/src/audit.rs
  - echo-core/src/tools/mod.rs
  - echo-core/src/utils/retention.rs
  - echo-execution/src/sandbox/docker.rs
  - echo-state/src/audit/file.rs
  - echo-state/src/audit/memory.rs
  - echo-state/src/audit/mod.rs
  - echo-tools/src/files/apply_patch.rs
  - echo-tools/src/files/files.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/react/capabilities.rs
  - src/agent/snapshot.rs
  - src/eval/regression.rs
  - src/eval/replay.rs
  - src/eval/runner.rs
  - src/evolution/background_review.rs
  - src/improve/analyzer.rs
  - src/improve/trajectory.rs
  - src/tools/builtin/agent_dispatch.rs
  - src/trace/analyzer.rs
  - src/trace/mod.rs
  - echo-agent-learning/tests/example_contracts/demo50_eval.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.background-review-detached-persistence-settlement, finding.in-memory-audit-successful-drop, finding.trace-audit-secret-boundary, finding.trace-effect-event-producers, finding.tool-terminal-observation-divergence, finding.transcript-projection-settlement, finding.diagnostic-persistence-failure-visibility, behavior.observation-persistence, behavior.effect-permission-execution, behavior.eval-evolution, rule.fact-projection-separation, rule.quality-observation-boundary]
limitations:
  - Commands ran in the current local session with an external dedicated target; no repository log artifact was created
  - The workspace Clippy/tests/check/feature matrix ran before the final reserved-key marker repair and must be rerun for a final-digest full gate
  - Three tests were intentionally ignored by their suites; no executed test failed in that earlier matrix
  - The post-marker focused test and targeted independent rereview do not establish SDK/CLI consumer or remote CI parity
command_results:
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
  - { command: "cargo test -p echo_core utils::retention::tests --locked", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/.cache/codex-targets/eko-observation-final-01a0b249 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/.cache/codex-targets/eko-observation-final-01a0b249 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/.cache/codex-targets/eko-observation-final-01a0b249 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --all-targets --all-features --locked --quiet", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/.cache/codex-targets/eko-observation-final-01a0b249 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo check --workspace --lib --no-default-features --locked", exit_code: 0 }
  - { command: "for feature in a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart; do CARGO_TARGET_DIR=/Users/ls/.cache/codex-targets/eko-observation-final-01a0b249 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p echo_agent --no-default-features --features $feature --locked || exit 1; done", exit_code: 0 }
---

# Observation repair engineering gates and final marker check

## 支持的结论

最终 reserved-key marker 修复之后，`cargo test -p echo_core utils::retention::tests --locked`
12 passed、0 failed，`cargo fmt --all -- --check` 和 `git diff --check` 退出 0；独立定向
reviewer 在当时完整 diff 上报告 Critical/Important/Minor 0。此前一轮源码在独立于仓库的
target 上通过两档 Clippy、workspace all-target/all-feature 测试、no-default workspace
library check 与 16 个独立 feature check。此前 workspace 测试中 root 953、core 392、
execution 323、integration 373、state 357、tools 193，learning 28、example contracts 21；
tools 有 2 个 ignored，另有 1 个 ignored，实际执行 0 failed。这一整套结果早于最后的
marker 改动，不能直接宣称是最终 digest 的完整门禁。

## 来源与范围

命令由主代理在本会话执行并回报 exit code 与测试计数；最终 marker focused/review 收据
来自独立定向复审报告。构建目录位于用户 cache，未加入 Git inventory 或语义摘要。
此证据按时间区分旧矩阵与最终 focused，不替代当前 digest 的完整合并门禁。

## 已知缺口

当前 digest 的完整 workspace/feature 矩阵仍须重新运行；SDK/CLI 的 protocol、Host、
语言与应用生命周期门禁也不由 framework 命令覆盖。
