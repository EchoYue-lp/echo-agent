---
schema_version: 1
id: evidence.sandbox-manager-stream-failure-typing-verification
kind: evidence
observed_at: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
source_refs:
  - echo-execution/src/sandbox/manager.rs
  - echo-execution/src/sandbox/local.rs
  - echo-core/src/sandbox.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
supports: [finding.sandbox-manager-stream-failure-typing]
limitations:
  - 仅覆盖框架 focused tests 与受影响 crates 的静态检查，不等同完整 workspace 合并门禁
  - 没有真实 Docker/Kubernetes 故障注入；Local backend-start failure 已通过真实 manager caller 入口验证
  - 独立复审已 PASS，但 PR/CI、远端 main 与 Issue 关闭仍待交付验收
command_results:
  - { command: "cargo test -p echo_execution sandbox::manager::tests::backend_stream_start_failure_is_typed_terminal --locked", exit_code: 0 }
  - { command: "cargo test -p echo_execution sandbox::manager::tests --locked", exit_code: 0 }
  - { command: "cargo test -p echo_execution sandbox::local::tests --locked", exit_code: 0 }
  - { command: "cargo test -p echo_core sandbox::tests --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_execution -p echo_core --all-targets --all-features --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo clippy -p echo_execution -p echo_core --lib --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# Issue 82 SandboxManager typed stream failure验证证据

## 支持的结论

定向 manager suite 13/13、Local suite 23/23、core sandbox suite 4/4 均通过。
真实 caller 入口将不存在的 working directory 交给 `SandboxManager::execute_stream`，
首个且唯一终态为 `SandboxStreamEvent::Failed(IoError)`，随后 stream EOF；没有
`Complete(exit_code=-1)`。受影响 crates 的 all-target `-D warnings` Clippy、lib
panic-policy Clippy、formatter 与 diff check 均 exit 0。

## 来源与范围

所有命令针对整合提交 `bf95ef61`、`origin/main@dbe9e1112e38a8a835871242fcf8dbf5abd3db0b`
基准和 `source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f`。
这组收据证明 backend 建流失败的 typed terminal 行为，不证明真实外部沙箱服务或远端
平台信号。

## 已知缺口

完整 `./scripts/verify.sh` 已在整合快照上通过；17-feature 矩阵不适用于本次无公共 API
形状变化的修复，远端 CI 与 main 交付仍待验收。独立 reviewer 已读取最终 diff、源码与本证据，
结论 PASS、无阻塞发现。
