---
schema_version: 1
id: evidence.guard-direction-contract-verification
kind: evidence
observed_at: source:041e17dc4bd59659d6bf15646a7a71bd89a9ba7bb118d35df06fa3e5f3866b6b
source_refs:
  - echo-core/src/guard/mod.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/phases/think.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/snapshot.rs
  - docs/en/18-guard-system.md
  - docs/zh/18-guard-system.md
supports: [finding.guard-direction-contract]
limitations:
  - 仅是候选分支 focused 证据；完整 workspace/feature 门禁与独立复审待 main 交付阶段执行
  - 当前候选的 semantic change-evidence 校验仍有 47 项共享 source digest/既有 SDK continuity 错误；本 Finding 新证据无格式或引用错误，整合 strict 尚未通过
command_results:
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/channel-attachment-projection/target cargo test -p echo_agent --features content-guard agent::react::run::pipeline::tests --locked --lib", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/channel-attachment-projection/target cargo test -p echo_agent --features content-guard agent::react::run::phases::tools::tests --locked --lib", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/channel-attachment-projection/target cargo test -p echo_agent --features content-guard agent::react::run::stream_channel::tests --locked --lib", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/channel-attachment-projection/target cargo clippy -p echo_core -p echo_agent --all-targets --features content-guard --locked -- -D warnings", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/channel-attachment-projection/target cargo clippy -p echo_core -p echo_agent --lib --features content-guard --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# Issue 57 focused 验证证据

## 支持的结论

Core guard 与初始 Agent Guard focused tests 已在本切片较早候选通过；本次最终增量
以完整受影响 pipeline、工具发布和 stream_channel suites 为准。
其中真实 `execute_tool_with_policy` 入口覆盖 streaming/non-streaming ToolInput 阻断、
error/Transform fail-closed、未执行工具、ToolInput GuardBlock audit 恰好一次、
ToolOutput Block/Transform；final text terminal 测试覆盖 Output 阻断、Failed Trace、
Error event 和无 Token/FinalAnswer 泄漏；Transform 后 Token、FinalAnswer 和 Context
一致。provider 失败的部分文本不能绕过 Output guard；stream_channel 62/62 通过。
## 来源与范围

真实 streaming caller 无 Guard 时保留两段实时 chunk；配置 Guard 后 Block 与
Transform 均无未受检 ToolStream，受检内容在终态 ToolResult 可达。error-only 工具
的 Block、Transform、Guard Err 三种路径均断言 caller、Trace、Audit、callback
使用受检诊断，typed FileEdit effect 在结果和 Trace 中保留；transcript publisher
消费受检错误的边界另有定向回归。最终候选 pipeline suite 40/40、工具发布
suite 6/6、stream_channel 62/62 通过。
独立复审新增两个红测：原候选分别因 `data.is_none()` 和非空 output 旁原始 error
断言失败；绿测证实 ToolOutput Block/Transform 下 data/rich/kind/metadata 不再
可见，失败 output/error 两段文本均受检，FileEdit effect 不丢失。text Pass 但
data/metadata/kind/MIME 独有敏感值的回归也通过；Guard Pass 保留已检结构，
但图片 URL 无法文本证明安全，真实 transcript publisher 回归确认其不会经模型
Context 重新注入。无 Guard 时富内容仍可用。
受影响 crates 两档 Clippy 均退出 0。
新增真实 command hook 回归：PostToolUse hook 读取 tool output 后将其写入阻断
reason，原候选 caller error 泄漏 raw output 的断言 red；修正后 caller、Trace、
callback 使用同一受检原因，FileEdit effect 事实未丢。另一条真实 partial-failure
回归先 red 于 raw idempotency key；修正后 key 撤销、postcondition 受检，
category/recovery/side_effect 保留，caller 与 Trace 均无该自由文本。

## 已知缺口

这组收据不替代合并前完整门禁、feature 矩阵、基线摘要刷新、独立复审及远端验收。
`verify_semantic.py --base d5ad3584 --require-change-evidence` exit 1，剩余 47 项为
全局旧 source digest 引用与两项既有 SDK continuity；本候选新 evidence 的六项
必需标题错误已修复，未将失败宣称为通过。
