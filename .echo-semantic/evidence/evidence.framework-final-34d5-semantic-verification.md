---
schema_version: 1
id: evidence.framework-final-34d5-semantic-verification
kind: evidence
observed_at: 4eb005c15b9c0036b73e409026c668c02f0a9575
source_refs:
- echo-core/src/guard/content.rs
- echo-tools/src/data.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/mod.rs
- docs/adr/0084-channel-owned-operations.md
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
- behavior.protocol-projection
- rule.protocol-role-separation
- behavior.effect-permission-execution
- rule.permission-effect-order
- behavior.extension-publication
- rule.extension-generation-authority
limitations:
- Only installed0.5 deterministic semantic contract; does not substitute engineering gates, independent implementation
  review or consumer/main acceptance.
---


# 当前34d5严格语义门禁收据

## 支持的结论

installed echo-semantic0.5唯一verify_semantic.py，在最终Audit与完整工程Evidence归档之后执行 --strict-snapshot --base95ec --require-change-evidence，实际exit0。

## 来源与范围

下面逐字节保存实际stdout与命令/退出码收据；包含checker SHA，recorded_at是收据记录时间，不伪造命令开始/结束时间。其后仅加入本对象并复验，最终stdout应相同，源码摘要仍34d5。

## 已知缺口

通过只证明结构、引用、路径、快照和变更依据，不代替新23工程门禁、独立业务审查或main/SDK。用户Registry发布排除、Planready保持。

```json
[
  {
    "original_path": "/tmp/framework-semantic-final-34d5-strict.log",
    "sha256": "f72cd073939e72b8c1946482bbd1c54d1bce6cdc6c21bb0810797752739e5e15",
    "bytes": 148,
    "base64": "6K+t5LmJ5rK755CG5qCh6aqM6YCa6L+H77yaL1VzZXJzL2xzL015V29yay9jb2RlL3lscF9hZ2VudF9sZWFybi9scC1hZ2VudC8ud29ya3RyZWVzL2VjaG8tYWdlbnQtd29ya3RyZWVzL3Rhc2stcnVudGltZS1zdG9yZS1jb250cm9sLy5lY2hvLXNlbWFudGljCg=="
  },
  {
    "original_path": "/tmp/framework-semantic-final-34d5-strict.receipt.json",
    "sha256": "0d841a2bf33d7d35c6e5afc13f1b4ef4c77f01d95c0d8a3949b16a6b0e959c1c",
    "bytes": 980,
    "base64": "ewogICJzb3VyY2UiOiAic291cmNlOjM0ZDVmNTU3NGM5Y2Y1YTY0NTk3MGNkOGI1ODM2N2MzOWJjZTI2ODYzMDQ4OGI2OGQzZWE1NGU5OTY3MjhlMTciLAogICJjb21tYW5kIjogWwogICAgInV2IiwKICAgICJydW4iLAogICAgIi9Vc2Vycy9scy8uY29kZXgvcGx1Z2lucy9jYWNoZS9lY2hvLXNlbWFudGljL2VjaG8tc2VtYW50aWMvMC41LjAvc2tpbGxzL3NlbWFudGljLWNvbnRyYWN0L3NjcmlwdHMvdmVyaWZ5X3NlbWFudGljLnB5IiwKICAgICItLXJvb3QiLAogICAgIi9Vc2Vycy9scy9NeVdvcmsvY29kZS95bHBfYWdlbnRfbGVhcm4vbHAtYWdlbnQvLndvcmt0cmVlcy9lY2hvLWFnZW50LXdvcmt0cmVlcy90YXNrLXJ1bnRpbWUtc3RvcmUtY29udHJvbCIsCiAgICAiLS1zdHJpY3Qtc25hcHNob3QiLAogICAgIi0tYmFzZSIsCiAgICAiOTVlYzY2YzYwODNhOGZlYjE3YTFjMjhkZTBkMDFlMWEyNGNhOGEyOCIsCiAgICAiLS1yZXF1aXJlLWNoYW5nZS1ldmlkZW5jZSIKICBdLAogICJleGl0X2NvZGUiOiAwLAogICJsb2dfcGF0aCI6ICIvdG1wL2ZyYW1ld29yay1zZW1hbnRpYy1maW5hbC0zNGQ1LXN0cmljdC5sb2ciLAogICJsb2dfc2hhMjU2IjogImY3MmNkMDczOTM5ZTcyYjhjMTk0NjQ4MmJiZDFjNTRkMWJjZTZjZGM2YzIxYmIwODEwNzk3NzUyNzM5ZTVlMTUiLAogICJjaGVja2VyX3NoYTI1NiI6ICJhODhlOGZkODk3NGRhYjYwNTgxNDVlNjQ2ODFlMDcxZWMyZmQ3ZjI4ZjkzNGZjMzM4MzVkMzAxYWExNjc5YmYzIiwKICAicmVjZWlwdF9yZWNvcmRlZF9hdCI6ICIyMDI2LTEwLTEzVDEwOjA3OjA1LjYzMTU3NiswMDowMCIsCiAgInNjb3BlIjogImRldGVybWluaXN0aWMgd2hvbGUtcmVwb3NpdG9yeSBzZW1hbnRpYyBzdHJ1Y3R1cmVzL3JlZmVyZW5jZXMvY3VycmVudCBzb3VyY2Ugc25hcHNob3QvaGlnaC1yaXNrIGNoYW5nZSBldmlkZW5jZTsgbm8gZW5naW5lZXJpbmcgb3IgYnVzaW5lc3MgcmVwbGFjZW1lbnQiCn0="
  }
]
```


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
