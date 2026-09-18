---
schema_version: 1
id: audit.lsp-runtime-state-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: time_lifecycle
freshness: examined
revision: 2dae76a93054c2e3be116b2b6e603d50cad21ce1
finding_refs: [finding.lsp-runtime-state]
challenges:
  transport-terminal-settlement:
    revision: 2dae76a93054c2e3be116b2b6e603d50cad21ce1
    source_refs: [echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs]
    evidence_refs: [evidence.lsp-runtime-state-repair, evidence.lsp-runtime-state-verification]
  reload-and-restart-authority:
    revision: 2dae76a93054c2e3be116b2b6e603d50cad21ce1
    source_refs: [echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs, docs/adr/0043-lsp-derived-handle-lifecycle.md]
    evidence_refs: [evidence.lsp-runtime-state-repair, evidence.lsp-runtime-state-verification]
---

# LSP runtime state 独立复审

## 审查范围

独立 reviewer 分三轮检查 LspManager、StdioLspClient、EOF/framing/writer failure、pending admission、child owner、restart budget、reload、错误历史、测试与 ADR。

## 已检查故障假设

检查 EOF 清空后仍接纳请求、stdin writer 单独失败、非法 Content-Length 卡住、terminal status 先于真实 child 退出、主动 stop 抹除错误、reader/writer 自等待和锁环，以及 reload 旧 handle 复活。

## 实际实现路径与证据

Reader 与 writer 异常均先原子关闭 admission 并结算 pending，再清 cache、kill/wait 共享 child，最后发布 running=false 和 pid=None。外部 close 才 bounded join tasks，终态 task 不 join 自身。reload 等待旧 client 撤销后整体替换 config/routes，restart_count 和 last_error 由共享 runtime authority 维护。

## 问题记录

前两轮发现的 EOF admission、writer、framing、last_error 和 child reaping 问题均已修复；最终 framework 复审结论 pass。

## 残余风险

EKO cold-load Result 处理与 SDK Host existing-manager async reload 是跨仓消费端交付条件；framework pass 不替代其编译和 E2E。

## 未检查项

未运行真实 rust-analyzer 等 language server smoke；仓库保留的 live smoke test仍为 opt-in ignored。
