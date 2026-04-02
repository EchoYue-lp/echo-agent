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

---

## Skill vs Tool

| Dimension | Tool | Skill |
|-----------|------|-------|
| Granularity | Single operation | Domain capability pack |
| Registration | `agent.add_tool(box)` | `agent.add_skill(box)` |
| System prompt | None | Carries a prompt injection fragment |
| Tool count | 1 | Multiple |
| Semantics | "Do one thing" | "I'm proficient in a domain" |

---

## Built-in Skills

| Skill | Included Tools | Description |
|-------|----------------|-------------|
| `CalculatorSkill` | add/subtract/multiply/divide | Mathematical computation |
| `FileSystemSkill` | read_file/write_file/list_dir | File system operations |
| `ShellSkill` | shell | Shell command execution |
| `WeatherSkill` | get_weather | Weather queries |

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(
    AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant")
        .enable_tool(true)
);

agent.add_skill(Box::new(CalculatorSkill));
agent.add_skill(Box::new(FileSystemSkill));

let answer = agent.execute("Calculate 42 * 8 and write the result to result.txt").await?;
```

---

## Custom Code-based Skill

Implement the `Skill` trait:

```rust
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

agent.add_skill(Box::new(ResearchSkill));
```

---

## External Skills (Progressive Disclosure)

Aligned with the [agentskills.io specification](https://agentskills.io/specification). Skills are loaded from the filesystem — **no code changes** needed to extend an Agent's capabilities.

### Three-tier Progressive Disclosure Model

The core design principle: don't load everything at once. Instead, content is revealed layer by layer on demand, keeping the context window lean.

| Tier | Content | Trigger | Token cost |
|------|---------|---------|------------|
| **Tier 1: Catalog** | name + description (frontmatter) | Auto-scanned at startup | ~50-100 / skill |
| **Tier 2: Activation** | Full instructions + resource listing | LLM calls `activate_skill` | <5000 / skill |
| **Tier 3a: Resources** | Reference file contents | LLM calls `read_skill_resource` | On demand |
| **Tier 3b: Scripts** | Python/Bash/TS script execution | LLM calls `run_skill_script` | On demand |

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
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
metadata:
  author: my-team
  version: "1.0.0"
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
---

## Code Review

When asked to review code:

1. Load checklist: `read_skill_resource("code-review", "references/checklist.md")`
2. Analyze code against each checklist item
3. Output structured review findings

Current environment: !`uname -s`
Skill directory: ${SKILL_DIR}
```

### Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique name, kebab-case, 1-64 chars |
| `description` | Yes | Description, max 1024 chars, explains when to use |
| `license` | | SPDX license identifier |
| `shell` | | Shell for inline commands: `bash` (default) or `powershell` |
| `paths` | | Conditional activation file glob patterns (e.g., `["*.py"]`) |
| `allowed-tools` | | Restrict which tools this skill may use |
| `hooks` | | PreToolUse / PostToolUse hook definitions |
| `metadata` | | Arbitrary key-value pairs (author, version, tags, etc.) |

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

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(config);

// Option 1: Auto-discover (project-level + user-level)
let skills = agent.discover_skills(&[
    DiscoveryScope::Project(".".into()),  // ./skills/ + ./.agents/skills/
    DiscoveryScope::User,                 // ~/.agents/skills/
]).await?;

// Option 2: Specific directory (backward-compatible)
let skills = agent.load_skills_from_dir("./skills").await?;
```

After discovery, three progressive disclosure tools are automatically registered:

| Tool | Purpose |
|------|---------|
| `activate_skill` | Load full instructions + resource listing (supports `arguments` parameter) |
| `read_skill_resource` | Read reference files |
| `run_skill_script` | Execute Python/Bash/TS/PowerShell scripts |

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

### Example: YAML Definition

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "${SKILL_DIR}/scripts/validate.sh"
          timeout: 5
        - type: prompt
          prompt: "Verify command safety before execution"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: prompt
          prompt: "Check output for sensitive information"
```

### Matcher Rules

- `"*"` — matches all tools
- `"Bash"` — exact match
- `"Bash"` also matches `"Bash(git:*)"` and similar parenthesized variants

---

## Conditional Activation

Skills with `paths` are only surfaced/activated when matching files are touched:

```yaml
paths:
  - "*.py"
  - "tests/**"
```

The catalog shows: `- python-linter: ... [activates for: *.py, tests/**]`

---

## Tool Permission Restriction

`allowed-tools` restricts which tools a skill may use. The constraint is injected into the activation prompt:

```yaml
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
```

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

---

## Context Protection

Activated skill instructions are **protected from compression** — even when the context exceeds the token limit, skill instructions survive compaction.

```rust
// Internal mechanism: messages containing <skill_content are excluded from compression
ctx.add_protected_marker("<skill_content".to_string());
```

---

## Querying Installed Skills

```rust
// List all installed Skills
for info in agent.list_skills() {
    println!("- {} ({} tools)", info.name, info.tool_names.len());
}

// Check if a Skill is installed
if agent.has_skill("calculator") {
    println!("Calculator skill is installed");
}

// Total count
println!("{} skills installed", agent.skill_count());
```

---

## Examples

See the example files:
- `examples/demo07_skills.rs` — Code-based skill demo
- `examples/demo08_external_skills.rs` — File-based skill full feature demo (progressive disclosure + script execution + inline commands + hooks)
