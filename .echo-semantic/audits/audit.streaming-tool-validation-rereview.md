---
schema_version: 1
id: audit.streaming-tool-validation-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: trigger_input
freshness: examined
revision: source:aaf0d4c101710a5879fe6066820ffc3145ab93b4295ab8aa22878d54e7a050b5
finding_refs: [finding.streaming-tool-validation]
challenges:
  stream-validation-parity:
    revision: source:aaf0d4c101710a5879fe6066820ffc3145ab93b4295ab8aa22878d54e7a050b5
    source_refs: [echo-execution/src/tools.rs, echo-core/src/tools/mod.rs]
    evidence_refs: [evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification]
  pre-effect-ordering:
    revision: source:aaf0d4c101710a5879fe6066820ffc3145ab93b4295ab8aa22878d54e7a050b5
    source_refs: [echo-execution/src/tools.rs]
    evidence_refs: [evidence.streaming-tool-validation-verification]
---

# Streaming Tool validation 独立复审

## 审查范围

复审ToolManager non-stream、stream与public async validator的schema/custom validation、pre-effect顺序、error parity及回归测试真实性。

## 已检查故障假设

验证stream是否仍绕过schema或custom validator，两级顺序是否漂移，非法输入是否先触及cancel/cache/permit/retry/Tool future，以及red是否仅来自编译失败或测试自造错误。

## 实际实现路径与证据

私有`validate_tool_input`唯一组合schema后custom，三个真实入口全部复用；stream在cancel、cache读写、permit、retry与Tool future之前调用。两条有效red使用fresh ToolManager和公开stream入口，旧实现实际执行默认stream并返回成功，测试才主动报告非法参数到达执行；最初Box模式编译失败已排除。Green中schema execution为0，custom validation为1且execution为0；25个ToolManager测试覆盖valid output、retry、timeout、cancel、cache、drain和forwarding。

## 问题记录

独立review无finding；`finding.streaming-tool-validation`具备repair、verification与rereview证据，可标记resolved。Cache、sandbox与permission Finding保持open。

## 残余风险

Custom validator自身仍可能产生副作用或阻塞；validation-before-cancellation保持既有顺序；cache并发和permission/sandbox组合不在本切片。

## 未检查项

未执行第三方/domain Tool、真实外部effect、完整workspace合并门禁、远端CI或其它80个open Finding。
