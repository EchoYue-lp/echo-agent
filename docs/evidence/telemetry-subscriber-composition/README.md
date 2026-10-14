# Framework Telemetry 组合修复定向证据

## 支持的结论

本包是 framework 自有取证副本，支持原 Telemetry 单一 provider/runtime/init-lock 权威下的 subscriber 组合、一次性初始化、rollback、上下文与 flush 的有限修复。当前组合源码 src/telemetry.rs SHA `52724271a9d0021cd850cf35dfe27c9e4f364e82908a581809cbf76c667ff269`、tests/telemetry_composition.rs SHA `996048f7eba783d008fbd01731edca53d345619684e1e903bf6b9fdd42233feb` 与原执行源逐字节一致。

原 raw endpoint 初始化日志在合法带 query 的 endpoint 上包含测试 secret；真实旧源单测失败1项。仅移除原日志事件的 endpoint 字段后，同定向集合通过6项：5个真正隔离进程场景与1个正常发现的 collector utility，不虚增成6种行为。最终格式化后同6项通过，受影响库严格 lint、fmt/check 均退出0。原独立实现复审与此 secret 增量复审均 PASS，零 Critical/Important/Minor；结论是该有限源范围，不是组合全门禁或 SDK 交付通过。

## 来源与范围

[endpoint-log-repair/SHA256SUMS.json](endpoint-log-repair/SHA256SUMS.json) 的12个成员及该清单自身，逐字节复制原 SDK 治理过程保存的执行材料。原 before/after 源、实际日志和 [receipt](endpoint-log-repair/receipt.json) 不改写；receipt SHA `d1976d39a22a8f4dfe0f6bffec3353bade748540d27ce00e89c6e9cb6ebc90ae`。这些取证 `.txt` 是原执行源副本，不是新的运行入口或状态权威。

原收据中的 nativePipelinePriorProof/newPublicCloseProof 为其生成当时 SDK 专属历史关系，未纳入本 F 包的支持范围；不需要这些未发布 SDK 文件来复核本包的框架源码、5个隔离场景或日志字段修复。旧SDK正常导出及public-close正/反例各有自己的源码切点，不从旧结果推断当前 N+Telemetry 组合已通过。

## 已知缺口

Root 正在对组合框架执行完整门禁；最终组合 Host、最新原生客户端、正式框架发布及 SDK 精确 pin/main 验收均独立。既有 API/one-shot 语义保留，新增 composition helpers 为 Rust/Host-only，不自动扩大三语言接受合同；没有泛化为所有 URL 的日志 redaction 保证，也不承诺任意网络故障可靠导出或所有活跃 span 跨 shutdown。

ADR 0087、docs/telemetry-composition-design.md、英文 tracing 文档及两个 README 已随原代码提交维护；此次材料只保取证。已有 examples 的编译/执行由原完整门禁验证，没有新示例 API 或官网入口需要增加。
