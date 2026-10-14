---
schema_version: 1
id: behavior.telemetry-subscriber-composition
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- time_lifecycle
- failure_concurrency
- permission_external
- contract_evidence
boundary: boundary.workspace-architecture
observed_at: 39e78a93c90b3bb61640cb4b78ce58dd700da5c1
code_refs:
- src/telemetry.rs
- tests/telemetry_composition.rs
- docs/adr/0087-telemetry-subscriber-composition.md
- docs/telemetry-composition-design.md
- docs/en/27-tracing.md
rule_refs:
- rule.framework-layer-ownership
evidence_refs:
- evidence.telemetry-subscriber-composition-repair
finding_refs: []
---

# Telemetry 与既有 subscriber 组合

## 重要承诺

框架拥有原 metrics/provider/runtime、采样与一次性初始化/关闭。embedding Host 可以组合已有 subscriber，而不引入第二 telemetry 状态机、ID 生成器或 export scheduler。Rust composition helper 不自动成为三语言合同。

## 当前行为

稳定 OTel layer 使用原 tracer 的标准委托与 downcast，OTel layer 与 marker 共享过滤，不因组合而打开被过滤的 TRACE；ConsoleFilter 独立观察同一 runtime 的 enable_console。standalone 在安装 subscriber 前先完成 exporter/provider 构造；冲突或构建失败不发布 globals，rollback 在原同步调用内完成。既有 METRICS、TELEMETRY_RUNTIME 和初始化锁保有唯一权威。

## 期望行为

上下文、有效采样和 parent/trace IDs 保留；console 与既有 diagnostics 只有一个格式 writer。默认行为、init/shutdown 签名及 one-shot 语义不变。Host 自己拥有 stderr 与异步 blocking-job 结算，不以取消 RPC 或 EOF 推断 exporter 已结束。

## 触发、结果与副作用

init 构造原 OTLP exporters 并发布一次状态；layer 只委托原 runtime；shutdown flush/close 原 providers。原初始化日志不输出配置中的 raw endpoint/query secret；这是具体日志字段保证，不是任意 URL/日志的泛化脱敏承诺。

## 失败、重试与恢复

不兼容 subscriber、构建失败、重复 init/shutdown 走现有错误；失败不得先占用成功 globals。定向隔离进程验证单一并发成功、rollback、standalone、真实 collector flush 与 context。网络故障无限重试、跨进程恢复或所有活跃 span 跨 shutdown 不由此承诺。

## 证据

evidence.telemetry-subscriber-composition-repair 与 ADR 0087 记录实际执行源、5个隔离场景加1个 utility、endpoint 日志真实红绿和原独立复审。完整组合门禁、框架发布、SDK 精确 pin/新 Host/原生验收仍是独立后继，不把旧 native 切点当组合成功。

## 裁决记录

既有 framework/application 分层不变，技术修复设计经本 goal 独立审查通过。没有新增三语言 public identity、协议、权限或物理结算政策；本对象保持 needs_review，不因定向测试数提升为全能力 verified。
