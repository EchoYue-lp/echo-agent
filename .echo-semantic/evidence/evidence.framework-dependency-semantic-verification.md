---
schema_version: 1
id: evidence.framework-dependency-semantic-verification
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
- Installed0.5 deterministic full semantic contract only; final native baseline contract check and independent
  final receipt review remain separate.
- Remote dependency-policy/PR/main and SDK published-pin consumer evidence are separate delivery proof.
---

# 依赖修复当前严格语义收据

## 支持的结论

新完整工程Evidence、独立源码Audit、49原文件归档、旧34d5历史4eb与current模型刷新后，installed0.5 strict snapshot/base4eb/change evidence实际exit0。加入此原始归档后复验最终stdout应相同。

## 来源与范围

下面2文件base64保存实际stdout与记录收据，记录时间不冒充命令起止时间。独立dependency task preflight真实check0；baseline祖先保95ec目标main可恢复谱系，本轮差异base4eb与其含义不同。

## 已知缺口

strict不替代新23工程或nativebaseline3focused、独立readerfinal、remote/main/SDK；Planready和Registry发布排除维持。

```json
[
  {
    "original_path": "/tmp/framework-dependency-semantic-final-strict.log",
    "sha256": "f72cd073939e72b8c1946482bbd1c54d1bce6cdc6c21bb0810797752739e5e15",
    "bytes": 148,
    "base64": "6K+t5LmJ5rK755CG5qCh6aqM6YCa6L+H77yaL1VzZXJzL2xzL015V29yay9jb2RlL3lscF9hZ2VudF9sZWFybi9scC1hZ2VudC8ud29ya3RyZWVzL2VjaG8tYWdlbnQtd29ya3RyZWVzL3Rhc2stcnVudGltZS1zdG9yZS1jb250cm9sLy5lY2hvLXNlbWFudGljCg=="
  },
  {
    "original_path": "/tmp/framework-dependency-semantic-final-strict.receipt.json",
    "sha256": "ed17684f328cd2178864993d0c6166e03a8af252c566f31d459511812d98a679",
    "bytes": 984,
    "base64": "ewogICJzb3VyY2UiOiAic291cmNlOjFjNTE1YzA5ODczOTY4YTI3OWU2MDc2Y2Y2YWJmNjU5OTIzZjFmYTk2MTM3MTQ0NDIzOTcwYmZjZDBlMTg4MWMiLAogICJ0YXNrX2lkIjogInByMTgxLWRlcGVuZGVuY3ktcmVwYWlyLWFmdGVyLTRlYjAwNWMiLAogICJjb21tYW5kIjogWwogICAgInV2IiwKICAgICJydW4iLAogICAgIi9Vc2Vycy9scy8uY29kZXgvcGx1Z2lucy9jYWNoZS9lY2hvLXNlbWFudGljL2VjaG8tc2VtYW50aWMvMC41LjAvc2tpbGxzL3NlbWFudGljLWNvbnRyYWN0L3NjcmlwdHMvdmVyaWZ5X3NlbWFudGljLnB5IiwKICAgICItLXJvb3QiLAogICAgIi9Vc2Vycy9scy9NeVdvcmsvY29kZS95bHBfYWdlbnRfbGVhcm4vbHAtYWdlbnQvLndvcmt0cmVlcy9lY2hvLWFnZW50LXdvcmt0cmVlcy90YXNrLXJ1bnRpbWUtc3RvcmUtY29udHJvbCIsCiAgICAiLS1zdHJpY3Qtc25hcHNob3QiLAogICAgIi0tYmFzZSIsCiAgICAiNGViMDA1YzE1YjljMDAzNmI3M2U0MDkwMjZjNjY4YzAyZjBhOTU3NSIsCiAgICAiLS1yZXF1aXJlLWNoYW5nZS1ldmlkZW5jZSIKICBdLAogICJleGl0X2NvZGUiOiAwLAogICJjaGVja2VyX3NoYTI1NiI6ICJhODhlOGZkODk3NGRhYjYwNTgxNDVlNjQ2ODFlMDcxZWMyZmQ3ZjI4ZjkzNGZjMzM4MzVkMzAxYWExNjc5YmYzIiwKICAibG9nX3BhdGgiOiAiL3RtcC9mcmFtZXdvcmstZGVwZW5kZW5jeS1zZW1hbnRpYy1maW5hbC1zdHJpY3QubG9nIiwKICAibG9nX3NoYTI1NiI6ICJmNzJjZDA3MzkzOWU3MmI4YzE5NDY0ODJiYmQxYzU0ZDFiY2U2Y2RjNmMyMWJiMDgxMDc5Nzc1MjczOWU1ZTE1IiwKICAicmVjb3JkZWRfYXQiOiAiMjAyNi0xMC0xM1QxNTozNzo1OC4yODI0MjIrMDA6MDAiLAogICJzY29wZSI6ICJkZXRlcm1pbmlzdGljIHNlbWFudGljIHN0cnVjdHVyZS9yZWZlcmVuY2VzL2N1cnJlbnQgc25hcHNob3QvY2hhbmdlIGJhc2lzOyBub3QgY29uc3VtZXIvcmVtb3RlIGRlbGl2ZXJ5Igp9"
  }
]
```
