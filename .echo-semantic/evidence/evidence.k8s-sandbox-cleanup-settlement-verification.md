---
schema_version: 1
id: evidence.k8s-sandbox-cleanup-settlement-verification
kind: evidence
observed_at: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
source_refs:
  - echo-execution/src/sandbox/k8s.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 未连接真实Kubernetes集群，finalizer与不可达node行为依据官方合同和fake-kubectl故障注入
  - 远端CI尚未执行，等待MR创建后提供独立Linux、Windows与依赖审计信号
---

# K8s Sandbox cleanup settlement验证证据

## 支持的结论

修复前回归`caller_abort_after_pod_submission_keeps_cleanup_owner`以exit 101失败：fake-kubectl
观察到`run`后，caller abort在1.66秒内未产生`delete`。首次集成复审又用
`delayed_api_commit_is_deleted_before_cleanup_returns`稳定复现首次delete为NotFound、随后Pod才
可见的竞态，旧实现exit 101。最终K8s定向测试19项全部通过，
证明成功、非零退出、timeout、cancel和caller drop均在terminal前到达delete；delete spawn、
非零退出和timeout均成为可见typed cleanup debt，且success/nonzero/timeout/cancel facts被保留。
新增反例还证明kubectl leader退出后遗留的pipe holder被进程组结算、stdin失败与blocked stdin
caller drop进入同一cleanup、delete完成/失败可被观察，以及JoinError补偿cleanup在waiter drop后继续。
新增两项还证明延迟可见Pod会被重删并确认缺失，持续无删除receipt会在共享deadline到期后
返回typed cleanup debt。
kubectl 控制命令启动还对 Linux `ETXTBSY` 瞬态错误执行共享 deadline 内的有界重试；其它启动
错误仍立即进入 typed failure，避免 runner 或滚动替换期间的瞬态可执行文件占用破坏 terminal
结算。
完整 all-feature gate 还证明原 test-only 250ms control deadline 在高并发进程调度下会误报；
harness 改为1秒后，19项测试全部通过，生产默认10秒不变，delete-timeout故障注入仍返回typed debt。
独立review首轮发现并阻断无界pipe drain、cleanup debt交接窗口和stdin/settlement测试缺口；修复后
第二轮复审PASS，Critical、Important、Minor均为0。

## 来源与范围

以下命令均在最终源码摘要`eff0290e1aff3c3a56f0ba94f57460e220d08f8eb04d3023efde058982972f03`
上以`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
CARGO_BUILD_JOBS=2`执行并返回0：

- `cargo test -p echo_execution sandbox::k8s::tests --locked -- --nocapture`：18 passed，0 failed；
- `cargo check -p echo_execution --all-features --locked`；
- `cargo clippy -p echo_execution --all-targets --all-features --locked -- -D warnings`；
- `cargo clippy -p echo_execution --lib --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`；
- `cargo fmt -p echo_execution`与`cargo fmt -p echo_execution -- --check`；
- `./scripts/check-sdk-contracts.sh`及其26个Rustdoc profile、91项生成合同和三语言SDK合同；
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`；
- `cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`；
- `cargo test --workspace --all-targets --all-features --locked`；
- `cargo check --workspace --lib --no-default-features --locked`；
- `cargo check -p echo_agent --no-default-features --features <feature> --locked`：
  `acp/a2a/mcp/lsp/sqlite/telemetry/topology/subagent/web/media/data/statistics/channels/git/database/rag/chart`
  17个feature逐项通过；
- semantic strict snapshot与`--require-change-evidence`校验通过。

第一次聚合执行`./scripts/verify.sh`时，已通过SDK合同与两档workspace Clippy，随后在根crate测试
二进制链接时因并发SDK迁移占用磁盘而收到`No space left on device`。清理当前worktree中可重建的
12.1 GiB门禁缓存后，上述精确测试命令从干净target重新执行并返回0；该失败不来自源码、测试或
lint结果。

## 已知缺口

fake-kubectl提供确定性的client/process/delete故障，不证明特定集群CNI、admission controller、
finalizer controller或失联node的运行时延迟；真实集群验收继续作为部署环境证据，而不是本地
单元门禁的替代品。远端Linux、Windows与依赖审计结果在MR创建后补充。
