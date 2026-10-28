# Contributing to echo-agent

Thank you for your interest in contributing to echo-agent! This document provides guidelines and instructions for contributing.

## Development Setup

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent
cargo build --workspace
```

## Before Submitting a PR

Please ensure the following checks pass locally:

```bash
cargo fmt --all
./scripts/verify.sh
```

Changes to dependencies must also pass `cargo audit --deny warnings` and
`cargo deny check`. CI runs both commands against the repository policies in
`.cargo/audit.toml` and `deny.toml`.

## Commit Messages

- Use the present tense ("Add feature" not "Added feature")
- Use the imperative mood ("Move cursor to..." not "Moves cursor to...")
- Limit the first line to 72 characters or less
- Reference issues and pull requests liberally after the first line

## Pull Request Process

1. Fork the repository and create your branch from `main`
2. If you've added code that should be tested, add tests
3. If you've changed APIs, update the documentation
4. Ensure the test suite passes
5. Make sure your code lints and formats cleanly

## Reporting Issues

When filing an issue, please include:

- A clear and descriptive title
- Steps to reproduce the problem
- Expected vs. actual behavior
- Rust version and operating system
- Relevant code samples or error messages

## Code Style

- Follow the standard Rust naming conventions (<https://rust-lang.github.io/api-guidelines/naming.html>)
- Add doc comments to all public items
- Remove obsolete APIs once callers, tests, and documentation have moved to the replacement
- Write examples using `#![doc = include_str!("../README.md")]` conventions

## License

By contributing, you agree that your contributions will be licensed under the MIT License.


## 本地构建与测试 profile

本仓的根 `Cargo.toml` 只为测试构建图设置 `opt-level = 1`，并显式保留
`debug-assertions = true` 和 `overflow-checks = true`。普通 `cargo build`／`cargo check`
的 dev profile 和 release profile 不因此改变；独立消费方仍由自己的 Cargo 根决定 profile。

大型数据／媒体图在未优化的 macOS 测试镜像中可能使 `__eh_frame` 超出 compact-unwind
表的 16 MiB 引用范围。已有同源有界对照中，原组合镜像的该节为 34,340,908 B，使用
上述测试 profile 后为 12,103,348 B，原七个业务测试及检查保持通过。该记录说明当前链接
布局，不能当作运行性能基准，也不能把旧 profile 的 warning 改记为通过。

测试仍按 Rust 测试 harness 的正常 unwind 执行，不配置被 Cargo 忽略的 `profile.test.panic`，
也不使用 `panic=abort`、LLVM／linker 警告屏蔽或 `allow` 规避问题。
[Cargo 官方 profile 说明](https://doc.rust-lang.org/cargo/reference/profiles.html#panic)
说明测试 harness 对 panic 设置的限制。

### 示例与合同的最小相关验证

从本仓根运行下列命令。测试目标只能在它声明且实际需要的 feature 下验收；
`demo49_research_agent` 与 `demo04_subagent` 的协作路径需要 `subagent`。
SQLite 仅用于框架相应示例，不表示 EKO 应用启用 SQLite。

每个独立验证批使用自有 target、任务根 TMPDIR 和 TS_RS_EXPORT_DIR；并发、磁盘预算、
原始失败保全和准确清理遵循当前仓库规则。下面只列变更相关的十个示例及四模块七项合同，
不替代合并前的 workspace、all-features 和适用条件矩阵。

```bash
cargo check -p echo-agent-learning --no-default-features \
  --features content-guard,files,human-loop,shell,sqlite,testing,web,subagent \
  --locked -j 5 \
  --example demo01_tools --example demo02_tasks --example demo05_compressor \
  --example demo10_streaming --example demo15_structured_output --example demo20_audit \
  --example demo44_code_laboratory --example demo45_customer_service \
  --example demo49_research_agent --example demo61_agent_factory

env -u CARGO_PROFILE_TEST_OPT_LEVEL \
  -u CARGO_PROFILE_TEST_DEBUG_ASSERTIONS -u CARGO_PROFILE_TEST_OVERFLOW_CHECKS \
  cargo test -p echo-agent-learning --test example_contracts --no-default-features \
  --features data,human-loop,media,testing,subagent --locked -j 5 -- \
  --test-threads=1 --nocapture demo04_subagent:: demo12_resilience:: \
  demo43_data_tools:: demo64_tool_pipeline::
```

第一条只证明当前示例可编译，不执行真实模型或交互式 main。第二条运行全部七个合同：
三个独立合同各一项，demo64 的 inventory、duplicate、pre-execution order 与完整 pipeline
四项均保留。采用 profile 修复后，必须去掉临时 env-opt 覆盖，以普通命令确认根 manifest
默认生效、全部测试实际选中且零 warning；不能沿用诊断覆盖得到的绿票。
