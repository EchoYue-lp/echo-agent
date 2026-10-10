---
schema_version: 1
id: evidence.docker-control-fixture-deadlines
kind: evidence
observed_at: a8c2d1ae3fce675633ea20d4e3d49258c327f634
source_refs: [echo-execution/src/sandbox/docker.rs, echo-execution/src/sandbox/manager.rs, CHANGELOG.md]
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations: [原生 Docker Engine 不由 CLI fixture 证明, 本次只验证 Docker test fixture 的控制时序，不宣称全 sandbox Finding 闭合]
---

# Docker control fixture deadline regression

## 支持的结论

Framework main run 37758607806 的 Linux foundations 在 empty_or_invalid_create_output_still_uses_named_cleanup_authority 失败，PR 候选 run 曾通过。test-only with_program 把 info/create/rm 全部设成 100 ms，原 test 需要完成普通 create 验证，却共享了 hung-stage 的紧 deadline。

## 来源与范围

源码分层：Docker 本来属于 framework SandboxExecutor，生产 control timeout 为 10 秒；修复只涉及 test-only injected CLI 的默认配置与 dedicated timeout fixtures，不改变 EKO 产品或 Docker 生产路径。已有 FakeDocker 和 with_program 是唯一 fixture，复用它们而不加另一个执行器。新增有限延迟、无效 ID 的确定性 fixture 用于证明普通错误路径不能被 hung-stage deadline 抢先改写。

## 已知缺口

确定性回归在修复前退出 101：create-delayed-bad 返回 Sandbox::IoError(create control stage timed out)，而不是 StartFailed；改为普通 deadline 后 Docker 28/28 通过，包含三种 create 输出、100ms hung info/create/rm 分类、有界 cleanup、cancel/caller-drop 和输出 budget。没有降低断言或跳过用例。

Rust 1.99 本地完整命令链退出 0：fmt check、workspace/all-target/all-feature Clippy、lib/bins unwrap/expect/panic/unreachable Clippy、workspace/all-target/all-feature tests 和 workspace/lib no-default check。受影响 echo_execution 331 项全部通过；examples、learning contracts 与其它 workspace 测试纳入同一完整链路。strict source snapshot/change-evidence 也通过。

旧 main 的相同失败任务重跑后通过，印证该用例间歇性依赖进程调度；此修复用确定性有限延迟回归防止同类问题重现，不以单纯重跑充当修复。

远端候选与 main 复验须另行确认，本地证据不替代 Linux/Windows 信号。Examples 不消费 test-only CLI override；CLI/SDK/echo-website 无需改动。
