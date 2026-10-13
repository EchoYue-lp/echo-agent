---
schema_version: 1
kind: evidence
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
source_refs:
- Cargo.lock
- Cargo.toml
- echo-core/Cargo.toml
- echo-execution/Cargo.toml
- echo-integration/Cargo.toml
- echo-agent-learning/Cargo.toml
- docs/dependencyrepair.md
- docs/evidence/dependency-repair-20261013/receipt.json
supports:
- behavior.workspace-composition
- rule.framework-layer-ownership
limitations:
- Dependency-only local candidate; new complete workspace gates, independent review, exact remote dependency-policy
  CI and main remain separately required.
- No runtime Rust/API/wire/state-authority changes and no new audit/deny exceptions; maintained libyaml-rs
  retains unsafe implementation details.
id: evidence.framework-dependency-policy-focused-verification
---


# 框架依赖策略修复：focused验证

## 支持的结论

与同一完整官方RustSec数据库commit29d1a60c3361576d366b1740bba2254dc6173434对照，原锁5vulnerabilities+1deniedmaintenance warning真实失败；新锁audit--deny warnings和deny四check退出0。1329官方Git文件hash匹配、1310advisories全载入；GitHTTP失败后官方GitHub tree/blob获取不删库、不syntheticcommit、不staleflag。459focused测试零fail/ignore；YAML old/new7Value+4typed/tagged+4malformed error/location与serialization bytes/roundtrip匹配。

## 来源与范围

PR181 run38417628760 job115366181706在真实4eb提交dependency-policy失败。docs/dependencyrepair.md与docs/evidence/dependency-repair-20261013/receipt.json保存完整官方参考、before/after、source SHA、原始CI失败与本地审计/测试日志；全部SHA256SUMS条目及当前6source SHA已核对。

已有deny50duplicate graph diagnostics是原policy warn、不是Rust compiler warning，未隐藏；4既有例外与review日期保持。temporaryconfig只换准确advisory缓存地址，repo deny.toml/.cargo/audit.toml与CI命令不变。无新架构ADR因API/分层/运行时不变。examples源码API不变、正式说明记录无需website行为同步原因，examples仍须新全量门禁执行。

## 已知缺口

只记录实际focused与policy本地red/green；新版23、独立review、CI/PR/main仍待新结果。新SDK最终pin/Host/三语言必须在最终交付revision重验，旧34d5 Host二进制不借作新版消费证据。Registry发布排除，Planready维持。
