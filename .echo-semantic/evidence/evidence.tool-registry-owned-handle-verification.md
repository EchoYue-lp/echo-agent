---
schema_version: 1
id: evidence.tool-registry-owned-handle-verification
kind: evidence
observed_at: e59fe773d92bca409fe0606b608c4812f87f7ac2
source_refs:
  - echo-execution/src/tools.rs
  - src/agent/react/capabilities.rs
  - echo-agent-learning/examples/demo35_dynamic_tools.rs
  - contracts/sdk/public-api.txt
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/source-contract.json
  - sdks/shared/contract-digests.json
  - docs/adr/0035-owned-tool-registry-handles.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order, behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行长时间registry mutation stress或Tool异步析构验收
---

# Tool registry owned handle 验证证据

## 支持的结论

修复后三个registry lifecycle测试在current-thread路径通过：active old Read期间同步replace和unregister均返回，old generation随后结算；replacement可见、unregister后lookup为空，old result不污染当前cache。完整32个ToolManager tests通过，`echo_agent --all-features` check、`echo_execution` all-target Clippy、demo35编译和16个独立feature check均exit 0。

Public inventory的12个Rust路径（5个canonical与7个re-export alias）只改变Box/Ref到Arc的signature digest，全部保持`host_or_rust_only`。SDK contract最终门禁确认90个artifact byte-stable、full inventory 31667项，协议测试5/29/75通过；canonical 9683与五类scope 5607/1765/780/90/1441不变，operation catalog、extension schema、fixtures和三语言facade零新增。Language gate中TypeScript 156、Python 168测试通过，Java SDK完成真实Host连接。

## 来源与范围

red日志为`plan13-red-current-thread-replace.log`，green与直接门禁记录在`.supreme/logs/plan13-*`。生成差异限于public-api、parity manifest、source contract和shared source digest。

## 已知缺口

独立review与最终feature/language门禁已通过；本Evidence须与最后一次semantic/Issue门禁结果共同作为提交证据，Issue #115在远端保持open。
