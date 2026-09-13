---
schema_version: 1
id: evidence.subagent-factory-singleflight-verification
kind: evidence
observed_at: 6d66479fd520da9cbbb66723faa35ce69a8963a8
source_refs:
  - src/agent/subagent/registry.rs
  - docs/adr/0033-subagent-factory-singleflight-publication.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - 远端Linux、Windows与发布环境CI尚未执行
  - 未进行loom、进程级故障注入或第三方factory资源清理验收
---

# Subagent factory single-flight 验证证据

## 支持的结论

旧实现上的取消恢复test以`cancelled factory attempt retained single-flight ownership`失败，publication boundary test以`factory started duplicate attempt 2 before publication`失败，均为exit 101。OnceCell实现后的15个registry tests全部通过，覆盖取消恢复、同代单创建、Arc复用、factory error后重试、旧代结果隔离、prebuilt/definition/factory注册、list与remove；3个Executor fresh-factory consumer tests通过，确认`create_fresh_agent`仍逐请求构造。

## 来源与范围

定向Clippy在`echo_agent + subagent + all-targets`下以`-D warnings`通过。只读`export_schema --check`重算完整Rust public inventory并以exit 0确认当前diff未改变public identity、signature、协议、schema或SDK artifact；独立review通过后，semantic strict snapshot与change-evidence也均以exit 0完成。

## 已知缺口

当前证据不证明factory内部外部副作用具备补偿，也不关闭TaskClaim/Attempt、definition catalog或其它Task/Workflow Finding。
