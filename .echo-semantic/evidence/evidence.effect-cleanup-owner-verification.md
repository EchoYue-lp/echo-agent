---
schema_version: 1
id: evidence.effect-cleanup-owner-verification
kind: evidence
observed_at: source:0427aee5ee15ea51f4623b1ae3db84522ef774c616f10390c3d7da16064d2ec0
source_refs:
  - echo-core/src/tools/artifact.rs
  - echo-execution/src/sandbox/docker.rs
  - echo-execution/src/sandbox/k8s.rs
  - echo-execution/src/sandbox/manager.rs
  - echo-tools/src/git_worktree.rs
  - echo-tools/src/files/artifact.rs
  - echo-orchestration/src/tasks/command_cell.rs
  - src/agent/react/tests.rs
  - src/agent/react/run/pipeline.rs
  - echo-agent-learning/examples/demo58_git_worktree.rs
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - focused命令由实现者执行，独立reviewer只读复核结果和最终源码，未自行重跑测试
  - 远端PR/CI和main交付另行验收；本地矩阵不能代替远端平台信号
  - fake Docker/K8s客户端和本地Git临时仓库不等于真实集群或长期运行
---

# 精确资源 cleanup owner 验证证据

## 支持的结论

原清理缺口的定向反例在最终源码上通过：Artifact pending/deferred 与失败重试、
root alias/替换、async finalization；Docker/K8s 创建前后 owner、活动资源和精确
debt；worktree marker 失败、目录替换、receipt drop/ack；Agent close 在共享
sandbox owner 活动时报告未结算，owner 结束后可重试。

## 来源与范围

实现者在 `1f53a12c` 修复并合入 `bd17c730` 后的源码上报告以下 focused
结果，均 exit 0、无失败：Artifact 17/17、Docker 28/28、
K8s 21/21、Git worktree 13/13、Agent close 1/1、file artifact consumer
12/12、command-cell consumer 36/36。相关 crate 的 all-target/all-feature
Clippy `-D warnings`、根 crate no-default check、Windows `echo_core`/`echo_tools`
Git target、`demo58_git_worktree` example、`cargo fmt --all -- --check` 与
`git diff --check` 也均 exit 0。

最终又在 `src/agent/react/run/pipeline.rs` 将 artifact 路径断言改为物理根与
artifact 规范化后的包含关系；最终非语义源码摘要为
`fe131df2148d81ee04fef5804e38cf59a2e231e8e28d5e71b89323a5c2fa98b0`。
该 all-feature 定向回归 1/1 通过，独立 reviewer 对测试差异复核 PASS。
最终源码的隔离 target `./scripts/verify.sh` exit 0，覆盖 workspace fmt、
all-target/all-feature Clippy `-D warnings`、lib/bins panic/unwrap/expect/
unreachable Clippy、workspace all-target/all-feature 测试和 lib no-default
check。最终源码上 `cargo check -p echo_agent --no-default-features --features
"$feature" --locked` 的独立 17-feature 条件矩阵全部 exit 0：
`acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics
channels git database rag chart`。本地完整门禁与条件矩阵均已通过。

## 已知缺口

独立 reviewer 未重跑命令；远端 Linux、Windows 与依赖审计信号仍需 PR/CI。
外部进程在目录身份安全点之后同步改名的竞态，以及进程崩溃后的恢复，不由
本地 focused 测试证明。
