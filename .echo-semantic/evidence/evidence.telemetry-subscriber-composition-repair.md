---
schema_version: 1
id: evidence.telemetry-subscriber-composition-repair
kind: evidence
observed_at: 4200d02a431366a0cf07033319c40a40be757a87
source_refs:
- src/telemetry.rs
- tests/telemetry_composition.rs
- Cargo.toml
- Cargo.lock
- README.md
- README.zh.md
- docs/adr/0087-telemetry-subscriber-composition.md
- docs/telemetry-composition-design.md
- docs/en/27-tracing.md
- docs/evidence/telemetry-subscriber-composition/README.md
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/SHA256SUMS.json
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/before-telemetry.rs.txt
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/endpoint-log-after.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/endpoint-log-before.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/no-endpoint-log-binaries.json
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/receipt.json
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/sdk-secret-host-build.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/secret-final-tests.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/secret-fmt-check.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/secret-fmt.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/secret-lint.log
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/telemetry.rs.txt
- docs/evidence/telemetry-subscriber-composition/endpoint-log-repair/telemetry_composition.rs.txt
supports:
- behavior.telemetry-subscriber-composition
- rule.framework-layer-ownership
limitations:
- Focused framework source verification and secret-log delta are not final combined workspace, SDK native or main acceptance.
- The six discovered tests contain five real isolated-process scenarios and one utility; overlapping runs are not unique capability counts.
- Earlier SDK native and public-close epochs are not proof of the combined ACP/telemetry source.
- No general URL redaction, arbitrary-network delivery, active-span cross-shutdown or new three-language API promise is inferred.
---

# Telemetry subscriber 组合与初始化日志修复证据

## 支持的结论

本次业务源观察固定于真实组合提交 39e78a93c90b3bb61640cb4b78ce58dd700da5c1。src/telemetry.rs SHA 52724271a9d0021cd850cf35dfe27c9e4f364e82908a581809cbf76c667ff269、tests/telemetry_composition.rs SHA 996048f7eba783d008fbd01731edca53d345619684e1e903bf6b9fdd42233feb 与定向实际执行源相同。原 METRICS/TELEMETRY_RUNTIME/init lock 继续拥有 providers、有效采样、one-shot 生命周期；稳定 layer 标准委托保留 context/downcast，console 与 diagnostics 只有一个 fmt writer，无新 runtime authority。

原 raw endpoint 初始化事件泄露合法测试 query secret，旧源真实一项失败。仅删除该日志事件的 endpoint 字段后，同源定向集合6项通过：5个隔离进程场景、1个 collector utility；格式化后原6项再次通过。原 Clippy -D warnings 及 fmt/check 实际退出0。完整源复审及 secret 增量复审均由本 goal 独立 reader 实际 PASS，零 Critical/Important/Minor；不伪造新的 Audit 执行或把相同用例重跑累加为更多能力。

## 来源与范围

框架自有副本位于 docs/evidence/telemetry-subscriber-composition/endpoint-log-repair。12个原 checksum 成员及原清单逐字节复制，原 receipt SHA d1976d39a22a8f4dfe0f6bffec3353bade748540d27ce00e89c6e9cb6ebc90ae；保留 before/after 文本与真实失败/成功原日志，不依赖未发布 SDK 的源文件才能复核框架结论。副本中的 .txt 是取证源，不是可调用的第二实现。

归档与窄 raw-log 属性已由主任务正式提交到 4200d02a431366a0cf07033319c40a40be757a87；本 Evidence 的 source_refs 与观察版本绑定该真实 Git。该提交没有改变39e中的 runtime/测试字节；原 receipt 中 SDK native/public-close 相对指针属于当时历史，不是本 F 包的支持来源，也不把旧0bac/诊断Host成功重标为组合源成功。

ADR 0087、设计、英文 tracing 文档及中英 README 已随原 runtime 维护；Cargo 的相关新增边是已有版本测试 collector 支持，不构成新的 SDK membership。原 N ACP 128项清单和 c87源 Evidence 独立保留，组合不让 telemetry 证据吞并 ACP 兼容义务。

## 已知缺口

Root 正在执行组合框架全门禁；其最终收据、框架发布、SDK 精确 pin/无patch构建及最新原生/main 验收另行绑定。这个 Evidence 不改变旧 Finding/Audit/基线义务，不关闭5秒物理结算或12项接受身份选择。原 API 未变，没有新增示例 API；既有 examples 的可执行检查继续属于正式合并门禁，官网无新增 telemetry 交互要同步。
