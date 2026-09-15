---
schema_version: 1
id: evidence.plugin-component-preparation-verification
kind: evidence
observed_at: source:298b7209a1d4a5151d191db785daa2e394ab43f75a3bc321d8275f6cdf56c757
source_refs:
  - src/plugin/prepared.rs
  - docs/adr/0012-immutable-plugin-preparation.md
  - docs/en/32-plugin-system.md
  - docs/zh/32-plugin-system.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 磁盘空间不足时完整workspace合并门禁留给后续MR验收与远端CI
  - semantic baseline source digest已在合流最新main后统一刷新
  - 未验证#72到#75的apply/unwire/lifecycle/owner并发故障
---

# Plugin component preparation 隔离验证证据

## 支持的结论

新回归场景在旧适用性判定上以exit 101稳定失败：无效Skill已被loader排除并产生结构化
diagnostic，但整代仍被`is_applicable=false`阻断。恢复组件隔离判定后，同一场景exit 0，
并证明健康Skill与Hook仍位于同一个prepared generation并可wire，健康Subagent/LSP冻结文档
仍被保留供应用层第二阶段解析。

首轮Plugin prepared定向测试8/8通过。复审后新增无效Hook action、MCP与完整Plugin准备失败
场景，当前11/11通过；同时覆盖依赖排序拒绝、共享有界cache、并发同revision单Arc、identity
变化及zero-read apply/rollback。Framework `mcp` focused lib Clippy以`-D warnings`通过。

合流framework main `c5f7688212d45d5bdcdbf60342605e8bfb176cae`后，在统一
`source:298b7209a1d4a5151d191db785daa2e394ab43f75a3bc321d8275f6cdf56c757`快照重新运行
`mcp`单feature prepared测试，11/11通过；同feature root-lib Clippy在`-D warnings`及
`unwrap/expect/panic/unreachable`禁用lint下通过，`cargo fmt --all -- --check`通过。

EKO在独立分支先以commit `0182e8e`无损适配当前framework合同；真实组合app-core check通过，
TaskGraph execution precondition测试1/1通过。其上的Plugin回归3/3通过，分别覆盖初始坏
Subagent、坏Hook action+MCP reload及坏Subagent+LSP reload；健康组件继续发布且两级诊断进入
`ReloadSummary.errors`。App-core focused lib Clippy以`-D warnings`通过。最终代码未改变Rust
public identity；本轮未触碰SDK host/protocol，且受磁盘与任务范围约束未重跑完整SDK
inventory，早期组合结果不冒充最终增量证据。

## 来源与范围

验证直接覆盖`PluginIntegrator::prepare`到`wire_prepared`生产路径。Semantic diff将影响闭包
限定为`behavior.extension-publication`、`rule.extension-generation-authority`、Plugin prepare
map场景与Finding #71；相邻#72/#73/#74/#75保持开放。

## 已知缺口

本集成候选已汇合最新main并统一刷新semantic baseline source digest。受本机磁盘空间限制，
完整workspace/all-feature门禁与远端CI仍留给后续MR验收；该保留不替代已通过的focused工程
验证，也不扩大Finding #71的闭合范围。
