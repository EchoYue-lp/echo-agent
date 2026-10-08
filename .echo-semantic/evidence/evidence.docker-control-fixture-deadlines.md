---
schema_version: 1
id: evidence.docker-control-fixture-deadlines
kind: evidence
observed_at: source:01ef6cf7171a2b93c7e5ece6961c95f0059198a235b967650ce30e06f3b13dab
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

完整门禁及远端 main 复验结果在完成后追加；当前不提前宣称通过。Examples 已进入 all-target gate，不消费 test-only CLI override；CLI/SDK/echo-website 无需改动。
