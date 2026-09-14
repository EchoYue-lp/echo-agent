---
schema_version: 1
id: evidence.tool-read-cache-authority-repair
kind: evidence
observed_at: 745a3f87fd51019aa3a96988e995dfd24bd1ff2f
source_refs:
  - echo-execution/src/tools.rs
  - echo-core/src/tools/mod.rs
  - docs/adr/0034-context-scoped-tool-result-cache.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 等价路径的不同词法写法可能产生额外cache miss，但不会跨scope复用
  - cache只按ToolRiskLevel::ReadOnly分类，不证明每个第三方Tool声明均准确
---

# Tool read cache authority 修复证据

## 支持的结论

基准`50890faac10ab91c90dc45769854c4b6e35f8376`仅用tool name与parameters作为key，且Write只在执行前clear。确定性red证明不同workspace/run只执行一次，并证明旧Read可在Write完成后写回其捕获值0，使后续Read返回0而不是新值1。

当前唯一result cache key增加effective working directory、conversation/run/turn/message/execution lineage及artifact root/retention/threshold/max-age；working_dir相对路径与artifact相对root分别按各自真实consumer语义解析到process cwd，无法确定effective cwd时禁用cache。每个非Read调用持有write-lifetime invalidation guard，在进入与Drop时bump epoch并clear；Read只在cache write lock内确认epoch未变后insert。Tool register/batch/replace/unregister与显式definition refresh也失效result epoch。执行期间持有的DashMap Tool guard使replacement在旧结果发布后再swap/clear，调用又在取得guard前观察epoch，避免旧实现进入新代cache。

## 来源与范围

`echo-execution/src/tools.rs`包含stream/non-stream cache入口、TTL/capacity、epoch与测试；`ToolContext`继续由`echo-core`定义且未新增字段。ADR 0034记录RFC 9111类比、scope、call_id取舍、失败/cancel语义和回滚。

## 已知缺口

本修复不验证第三方ReadOnly声明的上下文纯度，也不改变permission、sandbox、validator或Tool内部缓存；当前内建ReadOnly Tool没有以active message、visibility、script profile、cancel、trace、resource guard、delegation或Subagent uplink改变结果。
