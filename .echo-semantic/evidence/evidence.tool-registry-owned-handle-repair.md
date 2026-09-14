---
schema_version: 1
id: evidence.tool-registry-owned-handle-repair
kind: evidence
observed_at: e59fe773d92bca409fe0606b608c4812f87f7ac2
source_refs:
  - echo-execution/src/tools.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/run/phases/tools.rs
  - echo-agent-learning/examples/demo35_dynamic_tools.rs
  - docs/adr/0035-owned-tool-registry-handles.md
  - contracts/sdk/parity-manifest.json
supports: [behavior.effect-permission-execution, rule.permission-effect-order, behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - Arc只拥有Tool generation lifetime，不提供异步cleanup receipt或强制取消
  - register/replace/unregister仍是同步API，调用方不得在Tool自身同步回调中递归取得registration lock
---

# Tool registry owned handle 修复证据

## 支持的结论

基准`745a3f87fd51019aa3a96988e995dfd24bd1ff2f`把`Box<dyn Tool>`直接存入DashMap并让`get_tool`返回Ref；执行路径持有该Ref跨完整await。current-thread交错red在active Read后同步replace，5秒内无法完成并由受控进程组deadline以exit 124终止。

当前唯一registry value是`Arc<dyn Tool>`。Box注册输入在边界转换一次；lookup在同步map临界区clone Arc后立即释放guard；replace/unregister返回old Arc generation。Plan 12的epoch-before-get与mutation invalidation保持不变，old generation可完成但不能向new generation cache发布。

## 来源与范围

`echo-execution/src/tools.rs`拥有registry、generation publication、cache fence与current-thread测试；ReactAgent只透传owned返回，demo35说明迁移。ADR 0035记录Tokio、DashMap与OpenAI Codex依据、public Rust兼容和回滚。

## 已知缺口

不改变Tool内部资源清理、permission、sandbox或应用reload policy；其它open Finding不因本修复关闭。
