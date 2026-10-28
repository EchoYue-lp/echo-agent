# A2A退役与历史恢复

A2A 协议 client／server、Agent Card、任务 HTTP／SSE、鉴权及其公开 facade 已从本次
Framework 源码撤销，`a2a` Cargo feature 和专属 `demo23_a2a` 不再提供。这里不保留一个
默认成功的 server、空 discovery 或假任务控制器。

A2A 与 ACP 是不同协议。ACP、MCP、channels、Task/Subagent、普通 AgentRouter 协作、
通用工具结果与存储实现继续保留；EKO 的 IM／cron 和共享恢复也不依赖一个替代 A2A server。
独立 SDK Host stdio 是保留的 SDK 产品传输，不是 EKO JSONL 产品入口。

原完整协议实现、schema 和测试证据可从本机固定 `pre-retirement-2026-10-24` tag 与
`chore/Echoyue/full-capabilities-baseline` 分支对照：

```bash
git show pre-retirement-2026-10-24:src/a2a/mod.rs
```

这些 refs 不改指，用户数据、Exa 独立提交和其它未合流工作不随退役清理。恢复需新任务评估，
不回滚现主线。历史 ADR 和回执保原来源；源码与文档同步不代表删后消费者、正式 SDK pin、
锁、native、main CI 或发布已通过。

SDK 原 Plan04 撤销269项身份／23条路由／39个操作的证明保持，不重复撤销；保留5352项／
431条路由按已接受合同核验，物理删除不自动授权新的 accepted 差量。

边界见[ADR0090](../../../adr/0090-a2a-lsp-retirement-contract.md)。
