---
schema_version: 1
id: evidence.effect-cleanup-owner-verification
kind: evidence
observed_at: 07e4270380c40df3f412f99c0aecb6145410cb2a
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


## 2026-10-20 Docker 中断事实的局部反例与修复

本节仅补充既有 ADR 0002 的 primary terminal facts 约束，保留本 Evidence 的历史
`observed_at`、原 Docker 28/28 等计数、Finding #47 已解决结论及其它广域审查边界。
实际起点为 framework main `1d740858c788d0c53603c937ff17c5ed8966d6d8`；其原
CI run `39415055057` 最终六项成功、一项 foundations 失败。原失败断言检查
`timed_out=true`，但没有输出实际错误正文，故其具体触发原因仍为 **UNKNOWN**。
不能将本机未复现称为 flaky、无关失败或已证明该 CI 的唯一根因。

本机干净 main 的原 case 实际为 1 passed / 0 failed / 0 ignored / 334 filtered，
exit 0。随后仅加入诊断和确定性 fixture、保持生产 helper 原字节，对 timeout/cancel
分别注入 client-cleanup 或 output-drain error：四条路径均保留 secondary cause，
但均缺少 timeout/cancel 布尔事实，新测试实际 0 passed / 1 failed / 0 ignored /
335 filtered，exit 101。修复只从既有 `DockerInterruption` 推导并保留两个事实字段，
继续复用原 bounded error formatter，不创建第二状态权威、伪造未知输出计数，或更改
清理策略、超时、公共 API 和错误类型。原强断言保留并增加真实错误诊断。

同一确定性测试 after 为 1 passed / 0 failed / 0 ignored / 335 filtered，exit 0；
原 case after 为 1 / 0 / 0 / 335，exit 0；Docker 同模块为 29 / 0 / 0 / 307，exit 0。
这些选择互相重叠，不累加为 distinct 总数。精确 Docker 源码为
`b81e0073381ec1e4fcb921401f72c4c0e8f5a7f8b7d8f9fbc765d4b8b619a57d`；after
代码、尚未添加本次 ADR 文本时的实际工程快照为
`source:76a05591b29b1e43910329e1f5de88430d46dbf7a009e3ef2962b98f22601766`。
各命令保留真实 HEAD `1d740858...` 加各自 working-tree 输入快照，不冒 clean-main
源码运行；2,329 输入在各命令内部无漂移，原 failed before 和 Node/guard/raw 收据保留。
包含后来 ADR 文本的当前 1,764 工程输入摘要为 `source:6eb59d7e3a91d9c297c0f1e8e43a5f44e5c62d872d96f600f410514fee8fb773`，
尚未以该文档后快照重新运行完整 Cargo 门禁。

本轮使用原 framework 默认编译 profile，jobs=1、incremental=0、dev/test debug=0、
strip=symbols，无新 opt-level/RUSTFLAGS/assertion/overflow/unwind 覆盖；既有 macOS
linker 配置保持。原 9 GiB target cap / 5 GiB free floor、2 秒采样、三次 du 重试和
INT30/TERM10/KILL5 守卫不变；实际 peak target 8,259,682,304 bytes，最低 free
24,108,847,104 bytes，无编译 warning、源码漂移、资源中断或缺失收据。
源码与运行证据独立复审 C0/I0/M0；单源码路径必要性 C0。

后继 workspace 六门禁、真实公开验证源码 epoch 的具名 Audit challenge、独立复审及
精确 main 七项 CI 仍须另行完成，本节不预标 PASS，不新增 Finding，不替代 native
Hook/Stop。官网 ADR 0002 的 en/zh 镜像将在最终接受的 framework main 后同步，
不能归为不适用。无示例调用或 API 变化；原 all-target 编译/测试链仍承担示例验证，
本 focused 不冒全部 examples 已重新执行。SDK framework pin356 和三语言接受合同
保持不变，npm/PyPI/Maven 发布不在范围内。


## Docker 完整六门禁的当前本地资格

当前实际验证源码为公开 Git `7ded20c7c6f8e2ad4f8d54458f0c2afc003bd8c0`，
工程摘要 `source:6eb59d7e3a91d9c297c0f1e8e43a5f44e5c62d872d96f600f410514fee8fb773`；
Docker 文件仍为 `b81e0073381ec1e4fcb921401f72c4c0e8f5a7f8b7d8f9fbc765d4b8b619a57d`。
这是真实包含 ADR 文本的 1,764 工程输入版本；前节代码-only focused 快照、旧 pending
及所有历史段落保留原义，不重新标成当前整套运行。

原六条命令依次自然 exit 0：`cargo fmt --all`；
`cargo fmt --all -- --check`；
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`；
`cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`；
`cargo test --workspace --all-targets --all-features --locked`；
`cargo check --workspace --lib --no-default-features --locked`。
逐命令的 2,329 跟踪输入 before/after/当前工作树及 Git 源码身份一致，无编译 warning、
源码漂移、信号、资源中断或 Node/raw/guard 错误。未变更 Cargo/feature/cfg/公共签名，
因此没有新增 17-feature 条件矩阵，本次也不借旧 Builder feature 编译作为当前资格。

唯一完整 runtime 结果为 3,659 passed / 0 failed / 3 ignored / 0 filtered。
实际 89 个 Cargo targets 中，87 个有外层 Rust harness 摘要，两个 custom benches
没有 Rust 摘要，不计为测试通过；同一 echo_state target 的两个 nested/selfprobe
摘要已排除。root harness 实际 1,267 passed，echo_execution 实际 336 passed；
不叠加 focused 或不同 feature 配置。三个既有 opt-in 忽略项分别要求
`ECHO_AGENT_LSP_SMOKE=1`、`ECHO_AGENT_PROVIDER_SMOKE=1`、
`ZOTERO_API_KEY` 与 `ZOTERO_LIBRARY_ID`，均不算作真实外部集成覆盖。
完整测试原 raw 为 367,672 bytes，SHA-256
`20b176d7af2c10af378b789b8e0318274a39ce3cde082a86f9a2376ab181c60a`；
89 个实际执行产物逐 SHA 核验，最后 no-default 编译前冻结的 inode/size/mtime
在读取最终 executable SHA 时仍一致，原 raw、Node、guard 和分 target 计数均独立保全。

真实 framework 默认 unoptimized profile 保持，jobs=1 / incremental=0 /
dev,test debug=0 / strip=symbols，无新 opt-level、RUSTFLAGS、assertion、overflow
或 unwind 覆盖。既有 macOS linker 配置不变。已独立复审的 `888b1988...` 守卫
来自 44 组纯 fake 控制，其离线结果不当作产品 runtime；本次实际运行保留原
2 秒采样、9 GiB target cap、5 GiB free floor、三次 du 重试和
INT30/TERM10/KILL5/reap5。实际 peak target 8,721,489,920 bytes，最低 free
19,825,758,208 bytes。完整六门禁原件、全部输入/执行产物身份、外层摘要与资源
由独立 reviewer 复核，已实际返回 C0/I0/M0；这是当前本地六门禁的资格。

原 main `1d740858...` / CI run `39415055057` 的六成功、一失败结果及具体 cause
UNKNOWN 不被冲销、重试或改记成功。新 exact-main 七项 CI、远端 main 冷恢复、
CLI 完整八门禁和 native Hook/Stop 仍待各自验收。既有 Audit 仅补充指向真实公开
`7ded20c7...` 的单个具名 challenge；旧 header/freshness/challenges、Finding #47
已解决结论和广域 Behavior 状态不变。官网 ADR 0002 的 en/zh 镜像在最终接受的
framework main 后同步；无新增示例调用/API，示例已随本次 all-target 链编译测试。
SDK pin356 与三语言接受合同不变，包发布不在范围内。
