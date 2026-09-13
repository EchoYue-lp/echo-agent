---
schema_version: 1
id: evidence.tool-read-cache-authority-verification
kind: evidence
observed_at: source:efcb720425a2b1b4bbe38d080d40b6b323ffcc116f6499582c6561b703d9e14e
source_refs:
  - echo-execution/src/tools.rs
  - docs/adr/0034-context-scoped-tool-result-cache.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行真实文件系统Tool、长时间stress或跨进程cache验收
  - Tool registry仍持有DashMap Ref跨await，同步mutation的阻塞风险由独立Finding跟踪
---

# Tool read cache authority 验证证据

## 支持的结论

旧实现的scope test以calls 1/expected 2失败，in-flight test以output 0/expected 1失败，均exit 101。修复后31个ToolManager tests通过：workspace、run/execution/message及artifact config分别隔离且同scope命中；relative artifact root与working-dir下同后缀absolute root不碰撞；同名ReadOnly replacement对已完成cache和in-flight旧实现均返回新实现结果；Read捕获0、Write更新1并完成、旧Read随后返回0，但下一Read重新执行并返回1/calls 2。既有TTL capacity、stream validation、retry、timeout、cancel与drain不回归。

## 来源与范围

第二轮review blocker修正后的focused命令`cargo test -p echo_execution execute_with_context_tests --locked`以31 passed/0 failed退出；最终源码的`echo_execution` all-target Clippy以`-D warnings`通过，crate check也通过。`contracts/sdk`与`sdks/shared`相对基准零diff。Epoch compare与insert位于同一cache write临界区，guard Drop覆盖所有Rust退出路径。

## 已知缺口

未执行真实domain Tool或stress；持续高频Write会保守降低hit rate，但不会让旧Read成为Write后的cache authority。
