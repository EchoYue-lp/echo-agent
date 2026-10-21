---
schema_version: 1
kind: evidence
observed_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
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
id: evidence.framework-dependency-policy-repair
---


# 框架依赖策略修复：修复

## 支持的结论

6个Cargo路径限定替换：quinn-proto 0.11.16→0.11.18；5个manifest保持serde_yaml_ng alias但package改yaml_serde0.10.7，后端libyaml-rs0.3.0。移出旧serde_yaml_ng/unsafe-libyaml；其余registry条目与原lock逐字相同，初始resolver带来的5个无关edge已恢复。产品无关框架序列化依赖，未新增执行/注册/状态权威，既有YAML Value/Mapping/tag/parse/serde调用复用。

## 来源与范围

PR181 run38417628760 job115366181706在真实4eb提交dependency-policy失败。docs/dependencyrepair.md与docs/evidence/dependency-repair-20261013/receipt.json保存完整官方参考、before/after、source SHA、原始CI失败与本地审计/测试日志；全部SHA256SUMS条目及当前6source SHA已核对。

已有deny50duplicate graph diagnostics是原policy warn、不是Rust compiler warning，未隐藏；4既有例外与review日期保持。temporaryconfig只换准确advisory缓存地址，repo deny.toml/.cargo/audit.toml与CI命令不变。无新架构ADR因API/分层/运行时不变。examples源码API不变、正式说明记录无需website行为同步原因，examples仍须新全量门禁执行。

## 已知缺口

只记录实际focused与policy本地red/green；新版23、独立review、CI/PR/main仍待新结果。新SDK最终pin/Host/三语言必须在最终交付revision重验，旧34d5 Host二进制不借作新版消费证据。Registry发布排除，Planready维持。


## 2026-10-21 rustls-webpki 证书验证依赖修复候选

当前起点为 framework main `ebc69a56e6d0d5ff3a3d7c25e1cd2e25d578a1d5`。
其原 CI run `39530385802` / dependency-policy job `118994675210` 在
`cargo audit --deny warnings` 实际 exit 1：`rustls-webpki 0.103.14` 命中
RUSTSEC-2026-0382 / CVE-2026-111943。最终其余六项工程 job 成功；审计早退后，
bundled attribution 和 cargo-deny 步骤尚未执行，不能称为依赖门禁通过。
原 job raw 28,983 bytes，SHA-256
`10c3cb327136e567b08afb14063d6ace0a762654f6992bf63a2f7e2052dda6bf`，原件独立保全。
此前 main1d 的 Docker 断言没有打印实际 error，具体 cause 仍为 UNKNOWN；新 ebc
Linux 上原 strong case 和新增四组合 case 通过，仅是该实际 epoch 的 regression
结果，不冲销旧失败，也不抵消本次依赖漏洞。

官方 [RustSec 公告](https://rustsec.org/advisories/RUSTSEC-2026-0382.html)
于 2026-10-20 发布：受影响版本在配置的 trust anchors 允许时可能接受含已撤信任
emSign intermediate public keys 的证书链，相关私钥已公开；稳定修复范围为
`>=0.103.16, <0.104.0-alpha.1`。官方
[修复 PR #546](https://github.com/rustls/webpki/pull/546) 于
2026-10-20T08:54:35Z 合入，commit
`8ecdcc76f56b68fda67a6adf7976f5eed5006a4c`，对 0.103 分支回补拒绝已撤信任
SPKI 路径的逻辑。[官方 0.103.16 版本](https://crates.io/crates/rustls-webpki/0.103.16)
及 crates.io registry index 均确认未 yanked，MSRV 仍 1.71，checksum 为
`4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281`。
官方 MD、release/PR 与 registry 元数据逐字保存于 ignored 本任务目录供复核。

候选只修改 `Cargo.lock` 中这一 package 的两个值：`0.103.14 -> 0.103.16`
及对应 checksum。registry source、`ring` / `rustls-pki-types` / `untrusted`
依赖列表和其余 678 packages 均不变；官方版本索引的依赖声明也相同。
`rustls 0.23.46` 对 webpki 的 `0.103.14` caret 要求允许该补丁；manifest、
features、cfg、公共 API、框架/应用分层与状态权威不变。新工程摘要为
`source:d66f9839e00eabaa99925e07aef98bc65412f24111d3d6c4af065d1234931913`，
不能借此前 source6eb 的完整门禁冒新源码 PASS。既有 Docker 文件仍为
`b81e0073381ec1e4fcb921401f72c4c0e8f5a7f8b7d8f9fbc765d4b8b619a57d`。

锁图可追溯 workspace member -> reqwest 0.12.28 -> rustls 0.23.46 ->
rustls-webpki 0.103.14；锁图包含 optional dependency，不能仅凭锁边断言运行时
已启用 rustls。已保存的本机 default reqwest rustc 实际使用 default/native TLS，
没有 rustls extern；这不是漏洞风险豁免或审计忽略理由，也没有声称复现恶意证书。
用户明确延期的 glib、proc-macro-error 和 bincode 不在本修复范围，版本不变。
不新增 audit/deny ignore，不套用 SDK duplicate 特例，也不扩大历史 warning 例外。

当前仅有官方来源和精确锁差异的只读证明，尚未执行 fresh cargo audit、bundled
attribution/deny、新六门禁或后继精确 main CI/cold 验收；测试数/审计 PASS 不预填。
该 diff 不改 manifest/features/cfg/公共签名，不自动触发额外 17-feature 矩阵，
旧 feature 结果不借作当前资格。无需新 ADR：没有新架构、TLS 选择策略或状态模型。
无示例调用/API及官网镜像说明变化，examples 仍由后续 all-target 门禁编译测试；
官网本次无需要同步的版本文案。SDK pin356 和三语言接受合同保持，registry 发布
仍在范围之外。历史 header、正文、supports、limitations 和广域状态全部保留。


## framework 精确49组 duplicate 合同的独立人类接受及自动门禁候选

2026-10-21 用户已明确接受本 framework 的精确49组 duplicate 合同（协调线程
`call_844c5549` 第0项回复）；这是 framework 本次独立裁决，不是继承 SDK 例外。
当前唯一机器身份源为 `.cargo/cargo-deny-duplicate-baseline.json`，绑定 Cargo.lock
`7607c1458036adc04e64858215e21d45f7f8ae1ffa992ad82fba97e7a72aa528`、
原 deny.toml、cargo-deny0.20.2 与 workspace/all-features/locked 四类检查范围。
只接受同49组 crate/version/source；新组、少组、版本/来源变化、其它工具类别和
warning/error 仍阻断。旧版本表及原观察保留其历史时点，不建立第二份当前版本表。

已有真实 policy5 先后执行主 audit、原 vendor 归因8单测、生成唯一 registry-origin
锁、该锁 audit、cargo-deny 四类检查：前四命令自然0；最后 scanner 自然0且四类
`advisories ok, bans ok, licenses ok, sources ok`，同时保留49条 `warning[duplicate]`，
原严格 caller exit1。最后 raw721364bytes，SHA-256
`7685514a76052a4b1538add808e0edef6008630c0d6e7b45bd2e713f2951f74b`。
数据库精官方 e6f94f3760d5e6c54c1b05f204002263bc025524、原工具/有效配置/全部
原 raw、Node、guard 和退出码独立保留；本次接受不修改原严格失败记录。

候选 helper 为 SDK 候选与已审通用v2的相同字节实现；独立仓库中自包含，不依赖
SDK checkout 或任何 ignored 路径。CI 保留原主 audit、vendor 归因8单测/生成器及
该锁 audit，再运行新的13项纯离线比较器单测。等价官方命令仍为
`cargo deny --log-level warn --manifest-path ./Cargo.toml --workspace --all-features --locked check`，
完整原始输出及自然退出码通过 raw/receipt artifact 保留；比较入口验证精确基线、
cwd/命令/工具、显式 signal/errorCode、字节计数及哈希，未知文本格式 fail closed。
现有 deny.toml/.cargo/audit.toml、四项历史例外、依赖及 Rust 编译配置均不修改。

这是机器门禁整合候选，采用后仍须重新真实执行全部 policy5 并独立复审；不能用
本次旧 raw 的离线匹配冒新 scanner PASS。旧 D66 工程 FULL6 的实际资格仍只归属
该原始 Rust+锁源码切片；当前新增 ACP ownership 回归、后续生产修复及本政策代码/
配置会有新的源码摘要与资格，不能标为旧 D66 全源绿。原9/5和11/6资源中断仍按
原始 UNKNOWN 自然退出保留。当前 ACP RED 正在运行，不借本材料预标其结果。
当前 main CI/cold、CLI/SDK 消费资格仍须各自实际结果。此次仅依赖政策工具链变化，
无 public API、示例调用、运行时架构或官网行为说明变化；无新 ADR、示例或官网同步
路径，既有 examples 继续归 all-target 工程门禁。SDK pin356 和旧三项延期依赖不变。


## 2026-10-21 新4b官方数据库的实际 policy5 收口

在当前 canonical `source:42398e4e4b01b53bb11e712ec1d303efbb6c71c14aaec4d39291e7736406159b`
和 Cargo.lock7607 上，2026-10-21 07:59:52–08:01:20 UTC 实际顺序执行原policy5：
主 `cargo audit --deny warnings`、原vendor归因8单测、registry-origin生成、该锁
`cargo audit --deny warnings`、原在线cargo-deny四检查及精49比较入口，五条均
自然0。两次audit实际加载1348条advisory/扫描679依赖，原8单测全部通过；
同源码比较器13单测已在前一个FAST窗口实际通过，不重复累计为新5命令。

本次cargo-deny0.20.2 scanner自然0，保留全部49条duplicate warning；完整四项
`advisories ok, bans ok, licenses ok, sources ok` 后，原比较器核49组/107个精确
version-source身份、lock/policy/tool/cwd/argv及raw完整性，独立gate自然0。
scanner raw721364B/SHA `7685514a76052a4b1538add808e0edef6008630c0d6e7b45bd2e713f2951f74b`
虽与历史raw内容相同，但这次有新的实际运行/innerreceipt和数据库前后证据，未借旧
文件换日期。原件集中于 ignored任务目录 `new4b-policy-five-control-v1/`，本次
`new4b-policy-five-result-proof.json` SHA为
`96607d9485348a1438a3e8cff45f4184cc0d00d8824ef5dc9dfeae61573a0276`。

官方数据库为正常Git精commit `4b587f00996ebe5c05a73eaef105ef241e876414` / tree
`523058420679c8a4901e43e8e5fcb1e1406e7436`。API先确认对象，再由同官方repository
的SSH读取精浅fetch并核Git对象；两份独立copy各1367个physical/non-.git文件，
逐Gitblob/mode相符、无link/额外ignored输入，五命令前后均同4b。只配置本次独立
DB path，semantic database URL仍原HTTPS，实际Git origin为官方SSH；受限
BatchMode/StrictHostKeyChecking只用于命令环境，没有新增信任、全局配置或凭据。
在线cargo-deny按既有origin fetch成功后仍是同4b；不声称HTTPS443已恢复或远端
以后永不推进。e6→4b只变两条webpki-root(s)既有公告的wrapper说明，三锁无该crate
命中是差分事实；本次实际扫描资格仅归F679，不冒SDK/CLI的新扫描通过。

旧e6audit0/strictcaller1/raw49、后来scanner网络1、只读probe128、HTTPS/HTTP2/
HTTP1.1取源失败均保留各自原义；CLI的SSH push不能推出HTTPS恢复。最终不采用
ignored中的optional offline候选：正式helper/原CI默认在线argv、49组机器身份及
四检查保持，未新增不必要接口或warning豁免。当前2RED→2GREEN、15runtimeunit/
31adapter是原源码epoch的有限资格，所有Rust、锁与本次扫描脚本字节保持；
后继完整全6、SDK12/consumer、精确main/7CI/cold仍待实际门禁。
独立 reviewer 已按本次actual5原raw/receipt及源冻结复核，C0/I0/M0；复审只覆盖上述精确本机政策切片，不代全6或main/consumer验收。
