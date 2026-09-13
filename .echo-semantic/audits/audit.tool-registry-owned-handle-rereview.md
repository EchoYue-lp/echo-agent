---
schema_version: 1
id: audit.tool-registry-owned-handle-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: failure_concurrency
freshness: examined
revision: source:35a8d39143d7569cc99181972c94f487d02a37b82ae8cfcec3548838b871c28f
finding_refs: [finding.tool-registry-mutation-active-call-deadlock]
challenges:
  guard-lifetime:
    revision: source:35a8d39143d7569cc99181972c94f487d02a37b82ae8cfcec3548838b871c28f
    source_refs: [echo-execution/src/tools.rs]
    evidence_refs: [evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification]
  generation-and-cache-fence:
    revision: source:35a8d39143d7569cc99181972c94f487d02a37b82ae8cfcec3548838b871c28f
    source_refs: [echo-execution/src/tools.rs]
    evidence_refs: [evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification, evidence.tool-read-cache-authority-verification]
  public-contract-scope:
    revision: source:35a8d39143d7569cc99181972c94f487d02a37b82ae8cfcec3548838b871c28f
    source_refs: [src/agent/react/capabilities.rs, contracts/sdk/parity-manifest.json, contracts/sdk/public-api.txt, docs/adr/0035-owned-tool-registry-handles.md]
    evidence_refs: [evidence.tool-registry-owned-handle-verification, evidence.sdk-contracts]
---

# Tool registry owned handle 独立复审

## 审查范围

复审ToolManager registry guard生命周期、current-thread mutation、old/new generation线性化、result cache epoch、Rust public返回、SDK scope、Arc Drop与异步cleanup边界。

## 已检查故障假设

验证active Tool是否仍持有DashMap guard，replace/unregister是否等待旧future，lookup是否可能取得混合generation，旧代结果是否污染新cache，Arc返回是否误扩散为三语言合同，以及Arc Drop是否被误当作cleanup settlement。

## 实际实现路径与证据

唯一registry是`DashMap<String, Arc<dyn Tool>>`；`get_tool`只在同步map闭包内clone Arc，返回前释放Ref。Execution在lookup前捕获epoch，成功mutation发布后bump并clear，因此old generation晚完成不能store。Current-thread replace/unregister测试在old Read发出started并等待release后同步调用mutation；测试只有mutation先返回才能发送release，旧实现受控red为timeout 124，修复后全部32个ToolManager tests通过。

Public inventory差异为12个Rust路径（5 canonical、7 re-export alias），全部保持Host/Rust-only；总量与scope不变，operation catalog、wire/schema、fixtures和三语言facade不变。ADR 0035明确Arc只拥有generation lifetime，unregister不取消旧调用，async cleanup仍需独立合同。

## 问题记录

独立review无Critical、Important或Minor finding；`finding.tool-registry-mutation-active-call-deadlock`具备repair、verification与rereview证据，可标记resolved。Issue #115保持open，等待本地修复进入远端main后关闭。

## 残余风险

Old generation的外部副作用可继续到caller生命周期终点；第三方Tool的阻塞Drop或异步close不由Arc解决；未来若回退borrowed Ref，current-thread regression会依赖外层test deadline中止。

## 未检查项

最终16-feature与三语言SDK门禁已在review后通过；未执行长期高并发mutation stress、完整workspace合并门禁、远端CI或其它78个open Finding。
