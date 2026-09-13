---
schema_version: 1
id: audit.tool-permission-sandbox.permission-external
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.plan-mode-write-surface, finding.readonly-tools-custom-registration-bypass, finding.approval-authority, finding.hook-protected-path, finding.hook-permission-precedence]
challenges:
  plan-and-readonly-gates:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/pipeline.rs, src/agent/react/builder.rs, src/agent/react/mod.rs, echo-tools/src/registry.rs]
    evidence_refs: [evidence.effects-extensions]
  approval-authority:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-orchestration/src/human_loop/service.rs, echo-core/src/tools/mod.rs, echo-tools/src/shell.rs, src/agent/react/run/pipeline.rs]
    evidence_refs: [evidence.effects-extensions]
  hook-permission-precedence:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-execution/src/skills/hooks.rs, src/agent/react/run/pipeline.rs, docs/en/07-skills.md]
    evidence_refs: [evidence.effects-extensions]
---

# Agent Tool 权限与外部 Effect 审计

## 审查范围

审查 ReactAgent 自动 Tool 的 Plan/read-only gate、PermissionService、Shell CommandPolicy、protected paths 和 Hook permission；不覆盖 direct-user surface。

## 已检查故障假设

验证 Git/worktree/custom mutation 是否穿过只读声明、一次人工批准是否被下游 Shell 消费，以及 Hook Allow/source short-circuit 是否绕过 protected-path/deny-first。

## 实际实现路径与证据

Plan/read-only 按少量工具名过滤，Git/worktree Write/Execute 与 custom tools 可穿过。PermissionService 批准 Shell Execute 后不产生 approval receipt，Shell CommandPolicy 可再次拒绝。PreToolUse/PermissionRequest Hook Allow 在 PermissionService 前返回并绕过 protected path。Hook source 固定 UserConfig→Plugin→Skill，declarative Permission 又总是 stop propagation，较早 Allow 可阻止较晚 Deny。

## 问题记录

四个既有 Finding 均确认，新增 readonly custom registration bypass。Hook source precedence 与 global deny 的产品预期进入 semantic-decide；其它问题无需等待权限放宽裁决即可修复。

## 残余风险

公开 ToolManager 是 caller-owned primitive，trusted Hook command/http/MCP effect 是用户扩展能力；只有 Hook 被用来授权后续 Agent 自动调用时才进入本权限冲突。

## 未检查项

未检查 echo-agent-cli direct-user adapter、真实 UI approval provider、OS sandbox 或 Hook 外部命令运行。
