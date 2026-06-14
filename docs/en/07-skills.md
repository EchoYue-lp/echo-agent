# Skill System

## What It Is

A Skill is a higher-level capability unit compared to a Tool. Echo Agent provides two skill types:

| Type | Registration | Loading strategy |
|------|-------------|-----------------|
| **Code-based** | `agent.add_skill(Box::new(MySkill))` | Eager (tools + prompt injected immediately) |
| **File-based** | `agent.discover_skills(scopes)` | Progressive disclosure (catalog → activate → resources) |

```
Tool:  a single atomic operation ("read file")
Skill: a domain capability pack ("filesystem" = read_file + write_file + list_dir + usage guidance)
```

The framework half (`Skill` trait + `SkillRegistry`) lives in `echo-agent/echo-core/` and `echo-agent/echo-execution/`. The product half (the user-facing skill **catalogue / marketplace** under `~/.echo-agent/skills/`) lives in `echo-agent-cli/echo-agent-app-core/src/skills_hub/` and is orthogonal to the runtime registry — see [SkillsHub vs SkillRegistry](#skillshub-vs-skillregistry) below.

---

## Skill vs Tool

| Dimension | Tool | Skill |
|-----------|------|-------|
| Granularity | Single operation | Domain capability pack |
| Registration | `agent.add_tool(box)` | `agent.add_skill(box)` (code-based) or `discover_skills` (file-based) |
| System prompt | None | Carries a prompt injection fragment / SKILL.md body |
| Tool count | 1 | Multiple |
| Semantics | "Do one thing" | "I'm proficient in a domain" |

---

## Built-in Code-based Skills

Two `Skill` trait implementations ship in the framework, both feature-gated:

| Skill | Feature flag | Included Tools | Description |
|-------|--------------|----------------|-------------|
| `FileSystemSkill` | `files` | `read_file`, `write_file`, `list_dir` | File system operations |
| `ShellSkill` | `shell` | `shell` | Shell command execution |

```rust,no_run
use echo_agent::prelude::*;

# fn demo() -> echo_agent::error::Result<()> {
let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a helpful assistant")
    .build()?;

#[cfg(feature = "files")]
agent.add_skill(Box::new(FileSystemSkill));
#[cfg(feature = "shell")]
agent.add_skill(Box::new(ShellSkill));
# Ok(())
# }
```

> **Heads-up**: older revisions of this doc listed `CalculatorSkill` and `WeatherSkill`. Those are gone — only `FileSystemSkill` and `ShellSkill` remain in-tree as code-based skills. Domain capabilities (web search, paper search, coding, etc.) now live as **file-based skills** under `echo-agent-cli/skills/` (see [Built-in File-based Skills](#built-in-file-based-skills)).

---

## Custom Code-based Skill

Implement the `Skill` trait:

```rust,no_run
use echo_agent::skills::Skill;
use echo_agent::tools::Tool;

struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }
    fn description(&self) -> &str { "Web research: search + summarize" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(SearchTool), Box::new(SummarizeTool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("When you need web information, first use web_search, \
              then use summarize to organize the results.".to_string())
    }
}

# fn demo(agent: &mut echo_agent::ReactAgent) {
agent.add_skill(Box::new(ResearchSkill));
# }
```

---

## File-based Skills (Progressive Disclosure)

Aligned with the [agentskills.io specification](https://agentskills.io/specification). Skills are loaded from the filesystem — **no code changes** needed to extend an Agent's capabilities.

### Three-tier Progressive Disclosure Model

The core design principle: don't load everything at once. Instead, content is revealed layer by layer on demand, keeping the context window lean.

| Tier | Content | Trigger | Token cost |
|------|---------|---------|------------|
| **Tier 1: Catalog** | name + description (frontmatter) | Auto-scanned at startup | ~50-100 / skill |
| **Tier 2: Activation** | Full instructions + resource listing | LLM calls `activate_skill` (or IntentRouter classifies) | <5000 / skill |
| **Tier 3a: Resources** | Reference file contents | LLM calls `read_skill_resource` | On demand |
| **Tier 3b: Scripts** | Python/Bash/TS/PowerShell script execution | LLM calls `run_skill_script` | On demand |

### SKILL.md Format (agentskills.io standard)

```markdown
---
name: code-review
description: >-
  Professional code review skill: identify defects, security risks,
  and best practice violations. Use when asked to review code quality.
license: Apache-2.0
shell: bash
paths:
  - "*.rs"
  - "*.py"
triggers:
  - review my code
  - find bugs
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
depends_on:
  - coding
metadata:
  team: backend
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: prompt
          prompt: "Verify command safety before execution"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: command
          command: "${SKILL_DIR}/scripts/log_usage.sh"
          timeout: 5
sandbox:
  isolation: process
  network: deny
  allowed_paths:
    - "${SKILL_DIR}"
---

## Code Review

When asked to review code:

1. Load checklist: `read_skill_resource("code-review", "references/checklist.md")`
2. Analyze code against each checklist item
3. Output structured review findings

Current environment: !`uname -s`
Skill directory: ${SKILL_DIR}
```

### Frontmatter Fields (Current)

Defined in `echo-agent/echo-execution/src/skills/external/types.rs` (`SkillDescriptor` at line 75 and `RawFrontmatter` at line 415).

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique name, kebab-case, 1-64 chars |
| `description` | Yes | Description, max 1024 chars, explains when to use |
| `license` | | SPDX license identifier |
| `compatibility` | | Free-form compatibility note (echo-agent versions, OS, etc.) |
| `shell` | | Shell for inline commands: `bash` (default) or `powershell` |
| `paths` | | Conditional activation file glob patterns (e.g., `["*.py"]`) — also surfaced in catalog |
| `triggers` | | User-phrase triggers consumed by `KeywordClassifier` (see [Two activation paths](#two-skill-activation-paths)) |
| `allowed-tools` (alias `allowed_tools`) | | Whitelist of pre-registered tools this skill is allowed to use — **not** a list of tools to register |
| `depends_on` | | Other skills auto-activated first; cycles are detected and warned (`loader.rs:387-446`) |
| `hooks` | | `PreToolUse` / `PostToolUse` hook definitions |
| `sandbox` | | Per-skill sandbox policy: `isolation`, `network`, `allowed_paths`, `denied_paths`, `timeout` |
| `metadata` | | Arbitrary key-value pairs |

### Frontmatter Fields (Legacy)

The following fields still parse but are deprecated; activating a skill that uses them logs a deprecation warning at load time (`loader.rs:279-285`). They will be removed in a future release.

| Legacy field | Replacement |
|--------------|-------------|
| `version` / `author` / `tags` | Move into `metadata:` |
| `instructions` | Place the body directly in the Markdown after the closing `---` |
| `resources` | Drop in favour of automatic enumeration of `references/`, `scripts/`, `assets/` |

### Inline Command Execution

When a skill is activated, commands in the Markdown body are executed and replaced with their output:

```markdown
Current host: !`uname -s`
```
→ After activation: `Current host: Darwin`

Block commands:

````markdown
```!
rustc --version
```
````
→ After activation: `rustc 1.93.0 (254b59607 2026-01-19)`

**Security**: MCP-sourced skills **never execute** inline commands (untrusted remote content).

When inline commands or hook commands fall back to direct process spawning (no `SandboxManager`
configured), the runtime now:
- clears inherited environment variables before applying a minimal whitelist (`PATH`, `SKILL_DIR`, `SESSION_ID`)
- uses best-effort timeout termination via `kill_on_drop(true)`

This fallback is suitable for demos and local development, but production setups should still
prefer a configured `SandboxManager`.

### Variable Substitution

| Variable | Value |
|----------|-------|
| `${SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` | Absolute path to the skill directory |
| `${SESSION_ID}` / `${CLAUDE_SESSION_ID}` | Current session identifier |
| `${ARGUMENTS}` | All arguments (space-joined) |
| `${1}`, `${2}`, ... | Positional arguments |

### Directory Structure

```
skills/
├── code-review/
│   ├── SKILL.md              ← skill definition
│   ├── scripts/
│   │   └── lint.sh           ← executable script
│   └── references/
│       ├── checklist.md      ← reference document
│       └── style_guide.md
└── project-stats/
    ├── SKILL.md
    ├── scripts/
    │   ├── count_lines.py    ← Python script
    │   ├── find_todos.sh     ← Bash script
    │   └── dep_summary.ts    ← TypeScript script
    └── references/
        └── metrics_guide.md
```

### Discovery & Loading

```rust,no_run
use echo_agent::prelude::*;

# async fn demo(agent: &mut ReactAgent) -> echo_agent::error::Result<()> {
// Option 1: Auto-discover (project-level + user-level)
let skills = agent.discover_skills(&[
    DiscoveryScope::Project(".".into()),  // ./skills/ + ./.agents/skills/
    DiscoveryScope::User,                 // ~/.agents/skills/
]).await?;

// Option 2: Specific directory (backward-compatible)
let skills = agent.load_skills_from_dir("./skills").await?;
# Ok(())
# }
```

After discovery, three progressive-disclosure tools are automatically registered (`capabilities.rs:577-587`):

| Tool | Purpose |
|------|---------|
| `activate_skill` | Load full instructions + resource listing (supports `arguments` parameter) |
| `read_skill_resource` | Read reference files |
| `run_skill_script` | Execute Python/Bash/TS/PowerShell scripts |

If the same agent later calls `discover_skills()` again and finds additional file-based skills, these three tools are refreshed via `replace_tool` so their shared registry and available-skill view stay aligned with the latest discovery result.

### Dependencies and Cycle Detection

When a skill declares `depends_on`, every dependency is recursively activated before the requested skill (`registry.rs:236-325`). Cycles are detected by a DFS pass during loading and produce a warning (`loader.rs:387-446`); the loader does not block — duplicates are deduplicated and one acyclic activation order is picked.

---

## Two Skill Activation Paths

> ⚠️ **Important: the two paths produce slightly different message types and one of them is *not* protected from compression.**

### Path 1: LLM-driven activation via `activate_skill` tool

The LLM calls the `activate_skill` tool. The tool's success result is wrapped in an XML envelope by `ActivateContent::to_prompt_block` (`echo-execution/src/skills/external/types.rs:334-372`):

```
<skill_content name="paper-search">
{instructions}

Skill directory: ...
<allowed_tools> ... </allowed_tools>
<skill_resources>
  <file kind="reference">references/...</file>
</skill_resources>
</skill_content>
```

This block lands in the message stream as a `Role::Tool` message and contains the substring `"<skill_content"` registered as a **protected marker** in `ContextManager` (`agent/react/capabilities.rs:589-597`). The compaction passes therefore exclude it via `is_protected` (`compression/mod.rs:456-465`) and re-insert it via `merge_protected` after compression.

### Path 2: IntentRouter pre-classification

When a `KeywordClassifier` / `LlmIntentClassifier` returns `Intent::SkillRequired`, `react_loop.rs:738-764` activates the skill **and pushes the raw `content.instructions` as a `Message::system`**:

```rust
self.memory.context.lock().await
    .push(Message::system(content.instructions));
```

It does **not** call `to_prompt_block()`, so the resulting message lacks the `<skill_content` substring. ⚠️ **The IntentRouter-injected skill content is therefore *not* protected from compression** — sliding-window or summary compaction can drop it. This divergence is tracked in [07-cross-cutting.md §3](../../../echo-agent-cli/docs/system-deep-dive/07-cross-cutting.md).

### Where triggers come from

`KeywordClassifier` is populated at runtime from each `SkillDescriptor.triggers` (`echo-agent-cli/echo-agent-app-core/src/runtime.rs:196-255`). If a skill ships zero `triggers`, the IntentRouter cannot fire on it — the LLM tool path remains the only entry.

---

## Hooks System

Skills can intercept tool calls via Hooks for security auditing, logging, input/output modification, and more.

### Hook Events

| Event | When | Capabilities |
|-------|------|-------------|
| `PreToolUse` | Before tool execution | Block execution, modify input, inject prompts |
| `PostToolUse` | After tool execution | Inspect output, trigger follow-up actions |

### Hook Types

| Type | Behavior |
|------|----------|
| `command` | Execute a shell command; stdin receives JSON context, stdout returns JSON control directives |
| `prompt` | Inject a prompt message for the LLM |

### Command Hook Input (stdin JSON)

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {"command": "git status"},
  "tool_output": null
}
```

### Command Hook Output (stdout JSON)

```json
{
  "decision": "block",
  "reason": "Unsafe command detected",
  "updatedInput": {"command": "git status --short"},
  "continue": false
}
```

| Field | Description |
|-------|-------------|
| `decision` | `"allow"` to proceed / `"block"` to stop |
| `reason` | Reason for blocking |
| `updatedInput` | Modified tool input (PreToolUse only) |
| `continue` | `false` to stop further hooks |

If multiple matching hooks emit a `permission_mode_override`, the runtime keeps the
last non-empty override. Permission decisions themselves still follow the stricter
priority order (`deny > ask > allow`).

### Matcher Rules

- `"*"` — matches all tools
- `"Bash"` — exact match
- `"Bash"` also matches `"Bash(git:*)"` and similar parenthesized variants

---

## Conditional Activation by Path

Skills with `paths` are always discoverable in the catalog, but runtime activation is
guarded by a matching `context_path`:

```yaml
paths:
  - "*.py"
  - "tests/**"
```

The catalog shows: `- python-linter: ... [activates for: *.py, tests/**]`

At activation time, call:

```json
{
  "name": "python-linter",
  "context_path": "tests/test_api.py"
}
```

If `context_path` is missing or doesn't match the declared globs, `activate_skill`
returns an error instead of loading the skill.

---

## allowed-tools Whitelist

`allowed-tools` does **not** register tools — it filters tool calls against the union of every activated skill's whitelist (`registry.rs:178-199`):

```yaml
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
  - "Bash(git:*)"
  - "*"           # wildcard
```

Match semantics (`types.rs:277-307`):
- exact name (`"read_skill_resource"`)
- wildcard `"*"` (allow everything)
- prefix-paren (`"Bash"` matches `"Bash(git:status)"`)
- glob via `glob::Pattern` (`"Bash(git:*)"`)

The built-in `read_skill_resource` and `run_skill_script` tools also enforce this whitelist at call time and reject calls that are not permitted by the activated skill's allow-list.

---

## Cross-platform Script Execution

`run_skill_script` auto-detects the correct interpreter:

| Extension | Unix | Windows |
|-----------|------|---------|
| `.py` | `python3` | `python` / `py -3` |
| `.js` | `node` | `node` |
| `.ts` | `bun` → `deno` → `npx tsx` | Same detection |
| `.sh` | `bash` | Git Bash → PowerShell fallback |
| `.ps1` | `pwsh` | `powershell` |
| `.rb` | `ruby` | `ruby` |

Interpreters are invoked directly (not via `sh -c` / `cmd /C`) to prevent shell injection.

Additional runtime guarantees:
- the `script` path must be relative and must canonicalize under the activated skill directory
- malformed `args` strings are rejected instead of being silently treated as one opaque argument

---

## Context Protection

Activated skill instructions delivered through the **`activate_skill` tool path** are protected from compression — even when the context exceeds the token limit, that wrapped block survives compaction.

```rust,ignore
// Internal mechanism: messages containing "<skill_content" are excluded from compression
ctx.add_protected_marker("<skill_content".to_string());
```

⚠️ The protection is registered via `try_lock` (`agent/react/capabilities.rs:589-597`); if the lock is contended at install time, the marker registration is silently skipped. Subsequent skill activations would then be vulnerable to compression. Skills delivered through the **IntentRouter path** are not protected at all — see [Two activation paths](#two-skill-activation-paths).

---

## SkillsHub vs SkillRegistry

These are **two completely different concepts** and they coexist in the codebase. Don't confuse them.

| Concern | `SkillRegistry` (framework) | `SkillsHub` (product) |
|---------|----------------------------|-----------------------|
| Crate | `echo-agent/echo-execution/src/skills/registry.rs` | `echo-agent-cli/echo-agent-app-core/src/skills_hub/` |
| Purpose | Runtime lifecycle: discover → catalog → activate → resources for an agent instance | Local skill **catalogue / marketplace** UI: browse, install, uninstall on disk |
| Default scan path | `<project>/skills/`, `<project>/.agents/skills/`, `~/.agents/skills/` | `~/.echo-agent/skills/` only |
| Frontmatter parser | Full `serde_yaml_ng` (`loader.rs`) | Hand-rolled tiny key-value parser (no full YAML) |
| State | Activation set, sandbox policies, code-based skills, session_id | Just a list of locally-installed skill metadata |
| Used by | Every `ReactAgent` turn | The CLI `/skills` slash command (list / search / install / uninstall) |
| Install/uninstall | None — discovery only | `git clone https-only` + local copy + uninstall |

The `SkillsHub` does **not** load skills into agents. Built-in skills under `echo-agent-cli/skills/` are loaded into the agent through the framework's `discover_skills` path at boot (`echo-agent-cli/echo-agent-app-core/src/runtime.rs:133-153`), independent of the hub.

---

## Built-in File-based Skills

Eleven file-based skills ship under `echo-agent-cli/skills/` and are loaded at startup via the discovery path above:

| Skill | One-line role |
|-------|---------------|
| `coding` | Programming and software engineering: write/debug/refactor/review code |
| `data-visualization` | Charts and visualisation (bar/line/dashboard) |
| `data-wrangling` | Loading, cleaning, EDA on CSV/Excel/JSON |
| `doc-writing` | Reports, technical docs, articles, emails, proposals |
| `evidence-medicine` | Medical literature search + evidence-based analysis (PubMed, ClinicalTrials, PICO, GRADE) |
| `git-workflow` | Git ops (branches, commits, PR/MR, conflicts) |
| `paper-reader` | Deep reading + critical evaluation of a single paper |
| `paper-search` | Academic paper search across ArXiv + Semantic Scholar (non-medical) |
| `statistical-analysis` | Statistical tests (t/χ²/ANOVA), regression, modelling |
| `translation` | Translation, proof-reading, localisation |
| `web-search` | Internet info retrieval + cross-source verification |

Each ships with `SKILL.md` + an optional `references/` subdirectory. They are managed independently from `SkillsHub`.

---

## Skill Telemetry

A separate module `echo-state/src/skill_telemetry.rs` defines `SkillExecutionRecord` and `SkillTelemetry` types backed by the `Store` trait under namespace `["agent", "skill_telemetry"]`. The CLI's `evolution` command reads these records (`echo-agent-cli/src/cli/cmd_impls/evolution.rs:7,344`).

⚠️ **The runtime does not currently write to this store.** No `record_execution` call site exists in the agent runtime today; the schema is in place but the producer side is not yet wired into activation paths.

---

## Querying Installed Skills

```rust,no_run
use echo_agent::prelude::*;

# fn demo(agent: &echo_agent::ReactAgent) {
// List all installed Skills
for info in agent.list_skills() {
    println!("- {} ({} tools)", info.name, info.tool_names.len());
}

// Check if a Skill is installed
if agent.has_skill("paper-search") {
    println!("paper-search skill is installed");
}

// Total count
println!("{} skills installed", agent.skill_count());
# }
```

---

## Removed Component: SkillGateway

`SkillGateway` (a product-layer skill router used in earlier revisions) has been removed. Its responsibilities are split between:

- `IntentRouter` + `KeywordClassifier` — for keyword/semantic routing (framework)
- `SkillRegistry` — for activation and lifecycle (framework)
- `SkillsHub` — for the user-facing catalogue UI (product)

If a downstream eval harness or doc still mentions `SkillGateway`, treat it as historical.

---

## Examples

See the example files:
- `examples/demo07_skills.rs` — Code-based skill demo
- `examples/demo08_external_skills.rs` — File-based skill full feature demo (progressive disclosure + script execution + inline commands + hooks)
