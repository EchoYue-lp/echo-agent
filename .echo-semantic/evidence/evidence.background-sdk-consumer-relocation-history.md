---
schema_version: 1
id: evidence.background-sdk-consumer-relocation-history
kind: evidence
title: Background SDK 两项消费者的历史来源与迁仓职责
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
source_refs:
  - echo-sdk-protocol/tests/facade_inventory.rs
  - scripts/check-language-sdks.sh
supports: [asset.background-task]
limitations:
  - 此Evidence以真实Framework历史Git定位两条旧消费者；不将该历史观察当作当前Framework源码资格。
  - 独立SDK精确commit/blob及当前caller只在正文记录，本机Git可恢复不等于远端已发布/可获取或正式新pin。
  - 本迁仓源审未运行旧或当前SDK gate、生成器、Host/语言/运行期verification；后继正常引用/source/final是治理验证，不生成SDK运行等价或rereview收据。
  - 原Background handle修复/验证/复审与R20具名运行各保其source和范围，不据迁仓推导自动持久化或恢复Future。
  - 普通单仓source_refs只解析本Evidence的Framework历史观察，不能以跨仓URL/父目录/inline版本语法替代；正常连续性若仍有缺口须如实保留。
---

# Background SDK 两项消费者的历史来源与迁仓职责

## 支持的结论

两条旧Framework路径确实在原Asset observed_at=9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9存在，分别负责完整Rust facade inventory签名/scope/alias消费和三语言SDK/Host/quickstart gate。SDK产品归独立仓后，这两个职责仍由SDK当前精commit同路径和原caller承担；从Background当前本仓consumer_refs移出它们是已证迁仓，不是丢弃历史或取消职责。当前Framework双语文档和公开入口继续按各自真实角色保留，tasks/mod.rs或facade_smoke不能替代SDK两职责。

## 来源与范围

### 原 Framework 来源：只在此历史观察中恢复

原commit 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9 是本机真实Git提交，tree为38c34a0e928b977b535d78ac67937f28f27a8804。本Evidence的source_refs仅包含以下两条原Framework path，均按同一个真实observed_at解析；不混入当前SDK路径或不可恢复当前路径。

| 原路径／责任 | blob／mode／SHA256 |
| --- | --- |
| `echo-sdk-protocol/tests/facade_inventory.rs`：完整inventory签名、scope及alias分类测试；Background Clone属language_intrinsic，非三语facade新增 | `24bd4d7acc9964060adcfdd0757609fe3ec343c5`／`100644`／`9435e86c90275e799fa12238fa5c80893bdbfb8320cbfe03a69471cc38fbd38b` |
| `scripts/check-language-sdks.sh`：语言SDK、Host与quickstart原执行入口；历史脚本仍对scope计数作精确检查 | `c1080b5ea55710cdbeb2fc36cff0e9365834d944`／`100755`／`acfe30900b631c974fc2ca354da0a6754c8077ae3db90a509dc93d944ccaa75e` |

### 独立 SDK 当前职责：正文精确定位，不作为 Framework source_refs

当前读取的独立SDK本机commit为3fcb8d066e8a6b6560d7991f06b4255121f2a49b；以下两个文件与本机当前checkout同字节。精确GitHub blob链接是本机已核Git定位格式，**本次未联网核远端是否存在，不声称本地未push提交已发布或正式新Framework pin可用**。

| 原职责 → SDK 当前同role | SDK commit路径、blob、mode、SHA256／真实caller |
| --- | --- |
| inventory consumer | [echo-sdk-protocol/tests/facade_inventory.rs](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/echo-sdk-protocol/tests/facade_inventory.rs)；commit `3fcb8d066e8a6b6560d7991f06b4255121f2a49b`；blob `bab40bf454906780951e430d8341a04930f61c2f`／`100644`；SHA256 `3f2a07664ba97267e2437908d247be4c0c133816922c900839bbb91d40612226`；SDK scripts/check-sdk-contracts.sh:45–51调用cargo test --test facade_inventory；SDK CI rust-ci.yml:215调用该父gate |
| 语言gate | [scripts/check-language-sdks.sh](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/scripts/check-language-sdks.sh)；commit `3fcb8d066e8a6b6560d7991f06b4255121f2a49b`；blob `b504549e5c15d10ddde4b6eeef3f32dfa0927a70`／`100755`；SHA256 `a86c132008383a00001c91d5dbc859a5591cd1d0a84a5450eb401e1e75798803`；SDK CI .github/workflows/rust-ci.yml:261调用语言gate；其:55校catalog、:60校accepted映射、:116–123实际调用TS/Python/Java suites及quickstart |

父caller也从同一SDK commit直接读取，未执行：

- [scripts/check-sdk-contracts.sh](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/scripts/check-sdk-contracts.sh)：blob `9094d699ae52ef6311e2f874b73aeb8145f18045`，mode `100755`，SHA256 `ec6099d7276d1ff718446dcaf3d3eb905ae16c6486f952d101c22b3d744d7d02`。
- [.github/workflows/rust-ci.yml](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/.github/workflows/rust-ci.yml)：blob `2351ff39fb2769e77b2d6452973f0c92e737ebb8`，mode `100644`，SHA256 `acbc0665a419ff33d54f6f89beb9801c9bc1ec61181e3cbd4df879f19bd65162`。

当前inventory测试:45比较完整签名，:83检查scope/alias；SDK语言脚本:67–79将完整Rust inventory scope计数列为telemetry，accepted-external-contract才是跨语言阻断范围。原9d脚本:52的5607/1765/781/90/1441精确计数不再是当前相同运行义务，不能将新旧脚本或数量说成全行为matched。当前parity里BackgroundTask Clone的一canonical＋三alias均language_intrinsic，signature digest为18d361f8080b07a9b2ac956a55119361d9c9e2e48d18aabac21e2fa01c47c928，validation仍指facade_inventory；它不形成三语言新增Background facade。

Framework抽取记录位于本机27c7701e1eb116db1076da7f84bb68898544a44c:docs/adr/0051-extract-sdk-repository.md，确认SDK完整产品迁出、Framework不依赖SDK wire。它是迁仓背景，不扩本Evidence的source_refs为其它版本。当前独立SDK Host/Cargo与check-sdk-contracts脚本仍pin原Framework f7bac62eee04a963f92a3e4308b6ea2336e91cc0；本次仅比对该pin下background_task.rs、background_state.rs、tasks/mod.rs与R23同字节，不推成整个R23/F当前后继已被SDK接纳。

### 实际证据类别与跨仓关系

本次仅只读Git blob/真实caller和原r01独立复审报告a102f88873437eaf265ef5c1b6492a87c65bb5ef5439cd44adb01a92f1ad2beb，属于来源/职责追踪，未执行这些SDK consumer。原R20具体handle/DTO运行票和旧Background verification/rereview各保原scope，不新增运行验证或Audit。新common source-review及三Asset由本轮原caller实际当前输出绑定；本历史Evidence仍独立使用Framework9d，不拿历史或SDK3f替当前身份。

单仓普通关系为Framework asset.background-task → 本历史Evidence → 原Framework9d的两source_refs；当前SDK职责在正文链接精commit/路径/caller。installed0.5没有通过URL或../自动导入外仓语义对象的source_refs机制。若后继需要SDK自身正式Evidence，可由SDK唯一writer在SDK观察身份下以本仓两consumer及父caller为source_refs，支持其本仓asset.sdk-source-product，再与此关系互相正文精确定位；本候选没有跨仓写入、虚构该Evidence存在或为其填写runtimeverification。本机定位不证明远程分发，强continuity的其它义务仍须原正常流程实际核验。

## 已知缺口

Framework当前三Asset及common Evidence分别绑定原caller的实际当前来源；本历史Evidence保持真实可恢复Framework9d身份，两者不混用。身份绑定不表示原source/final或当前SDK运行已经通过。正常连续性如要求更多当前replacement证据，原writer须按真实职责/全部父指纹接管；测试/脚本不属于仅documentation的存储迁移例外，不能套该例外或伪造matched。

当前SDK正式新Framework pin、Host/三语言、远端获取/CI/发布、native与真实外部服务本次均未运行/未验。当前完整inventory是telemetry，accepted合同不能与旧计数硬比；SDK其它具名退役/修复也不在此确认。Background仍不自动保存或恢复Future；caller-owned/原Discovery长期整合未知、真实shutdown/owner-drop/任意第三方任务清理范围以及其它五Asset、六项源码缺口均不由迁仓关系关闭。
