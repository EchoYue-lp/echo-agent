---
schema_version: 1
id: evidence.chart-dimension-fidelity
kind: evidence
observed_at: 302abc5b81b57bbdf1a0e7993ccc786a0d859643
source_refs:
- echo-tools/src/chart.rs
- echo-tools/src/registry.rs
- tests/chart_dimension_contract.rs
- echo-execution/src/tools.rs
- docs/adr/0083-shared-tool-input-validation.md
- docs/en/02-tools.md
- docs/zh/02-tools.md
- docs/evidence/chart-dimension-fidelity-20261014/receipt.json
- docs/evidence/chart-dimension-fidelity-20261014/final-gates/manifest.json
- docs/evidence/chart-dimension-fidelity-20261014/final-gates/post-archive-runtime-input-proof.json
supports:
- finding.chart-dimension-fidelity
limitations:
- Fresh23 framework gates and audit/deny passed; scoped source review passed with
  root-confirmed minor documentation correction. Final independent full receipt/metadata
  review and remote exact-main acceptance pending.
- No SDK source or three-language API expansion; published SDK pin and new chart boundary
  consumer proof are separate root delivery obligations.
- Before private snapshot reconstructs original implementation plus executed fixture;
  actual old runs remain independently logged, not attributed to reconstructed commit
  runtime.
---

# Chart integer dimension fidelity evidence

## 支持的结论

GenerateChartTool原width/height as_u64.unwrap_or(default) asu32存在溢出回绕和Some非法值
默认覆盖。直接原red1pass/2fail：width4294967296输出0，width-1被600默认吞。
原canonical validate_tool_input/ToolManager同原range inputs被schema接受，manager输出
回绕；原integration2pass/1fail。候选缺省None才默认600/400；Some先as_u64再u32::try_from
返回exactfield InvalidParameter，schema0..u32MAX保全原表示范围，不新增resource限额。

原321x123、零、1、u32MAX和缺省尺寸保持。negative、float1.5/1.0、string、bool、null及
大于u32MAX返回明确错误。直接最后3pass0fail、canonical manager最后3pass0fail零warning。
strict chart-onlyproductionClippy Dwarnings+4panic lint最后0、fmt/diffcheck0。原Clippy
registry35 vec_init_then_push失败保留，chartentry改原既有extendbatchpattern后samecommand0；
原managerfixture4warnings清理后sameintegrationrun zero-warning，未用allowlint/skip。

## 来源与范围

Nbranch fix/Echoyue/chart-dimension-fidelity baseb909，sourcechart22afe7f、rootfixture956d7b、
registry352cfb原构造一次、readonlyflag、order/membership保持。唯一工具/schema权限由
GenerateChartTool/ToolManager拥有，Host不新建validator。docs中英tool章与ADR0083同步。
原raw/全时间/exit/logSHA及源码档位于docs/evidence/chart-dimension-fidelity-20261014。
私有beforeGit e7c25221与after d8a94fa2仅恢复/复放源码，HEAD/index不动，不冒执行过相应
commit的新证明；真实构建时代由不可变logs记录。第一次预检旧higherflags拒与fixture写入
时序错误以及第二missingsemantic-ref拒如实receipt披露，生产改动之前successfulrecord/check。

## 已知缺口

独立复审、全部frameworkmergegates/当前严格semantic及exactmain尚待协调root交付。
没有新public方法/状态/limits，原Chart普通示例调用不需改；当前exampletree无chart专用示例，
全workspacealltargets仍编译原examples。网站/SDK消费者展示源投影及publishedpin另交付，
该证据不宣它们通过。Dsubagent修复和proposed5秒policy未触碰。

## 文稿精度非语义增量

framework_final_review对源码/21raw档/6case返回ImplementationPASS C0/I0/M1；唯一Minor
是Schema数学integer vs serde_json整数表示的文稿不精确。root实际读最终英/中/ADR
三句后确认Minor已清除，有限PASS0；保原reader当时Minor1，不伪造其重新跑审。
生产chart22afe、registry352cf、rootcase956d7未变，6case/strict0继续适用。最新文稿
schema约束数学数值范围，direct要求serde_json整数表示，1.0explicitErr而不default。
原archivefinal-docs保存前Minor时代SHA；当前文稿新SHA为en8a734b3f/zhcdd6b3eb/ADR72fc4634，
它们不是旧实际测试source的新运行。即将完整framework gates另取当前freeze，当前尚未完成。

## 当前冻结源码全量工程门禁

初始source885dcd5432119ba5126e03d7fe8c57ef1b1cd33dcaf04554e92c49e887e52375
上原23项门禁全部实际退出0、零compilerwarning，source/index前后守恒。
formatter/check、两workspaceClippy、alltargetsallfeatureslockedtest/noDefault、
17独立featurechecks全部执行。主suite3508passed/0failed/3既有opt-in ignored，
87targets含59examples；echo_state两nested1pass不叠加。

依赖audit--denywarnings退出0；deny原policy四checkOK退出0，原多版本warning是
configuredwarn，以下receipt按类别计数，未增忽略或改policy。这些本地gate不证明
远端CI/exactmain，最后独立完整receipt审阅与currentstrictsnapshot仍需Root收口。

原23logs/23runner/manifest和audit/deny/统计收据已按原字节正式归档到
`docs/evidence/chart-dimension-fidelity-20261014/final-gates/`。每份SHA与byte数保存，
post-archive-runtime-input-proof证明原23green编译/测试源与当前Rust/Cargo/scripts完全相同。
归档是非语义证据增量，完整源码摘要新切点如实更新，不抹除885d实际runtimeepoch。

## 当前源快照与有限复审

正式52raw档增量后的完整Git/source快照为302abc5b81b57bbdf1a0e7993ccc786a0d859643，
source_digest_at_revision严格等当前07caadeefd3db109af0778911e6c3fe16539c3eca9ca95baddc2ed1a59a489d8；
ref refs/echo-semantic/snapshots/chart-final-archive-07ca保恢复，真实HEAD/index未动。
原23runtimeepoch885d的569Rust/Cargo/scripts字节与当前cut全部相同，归档增量为非语义。

源独framework_final_review ImplementationPASS C0/I0/M1，唯一文稿Minor由root实际阅读
新英/中/ADR并确认纠正，有限PASS0。原reviewMinor1不假写成reviewer另跑审。新Audit
仅源/有限6case/registry/docs范围；full52raw统计/885d→07ca等价receipt末独审仍待。
本localFindingresolved基于已具源码修复、实际工程验证及独源复审三链；main/CI不因此完成。

## 当前严格校验收据

官方verify_semantic.py --strict-snapshot --baseb9099558620f5f9693c736c85c214db14f62a8cb
--require-change-evidence 在当前source07ca上实际退出0，完整正常historicalreader读取可恢复Git。
原stdout145bytes，SHA256 `963b40bc64222bddd83d53cc0a74f8a4c9c300ddbdbed2206dc89806f219e747`。以下小收据无损Base64
保存在Evidence，原大rawgatelogs均在正式docs/final-gates，不重复大档；该收据不改变
已验证nonsemantic源码摘要，不代表remoteCI/main交付或SDKgoal整体闭合。

```base64
6K+t5LmJ5rK755CG5qCh6aqM6YCa6L+H77yaL1VzZXJzL2xzL015V29yay9jb2RlL3lscF9hZ2VudF9sZWFybi9scC1hZ2VudC8ud29ya3RyZWVzL2VjaG8tYWdlbnQtd29ya3RyZWVzL2NoYW5uZWwtaGFuZGxlci1mb3J3YXJkLy5lY2hvLXNlbWFudGljCg==
```
