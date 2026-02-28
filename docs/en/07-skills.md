# Skill System

## What It Is

A Skill is a higher-level capability unit compared to a Tool. It bundles a group of related Tools together with LLM guidance (a system prompt injection fragment) and installs them onto an Agent as a cohesive package.

```
Tool:  a single atomic operation ("read file")
Skill: a domain capability pack ("filesystem" = read_file + write_file + list_dir + usage guidance prompt)
```

---

## Problem It Solves

Problems with registering Tools directly:
- **Scattered registration**: 5 related tools require 5 separate `add_tool()` calls
- **No semantic guidance**: Tools have descriptions, but no overall "when to use this group" instructions
- **Reuse friction**: Using the same tool set across multiple Agents requires duplicating configuration

A Skill packages "tools + usage instructions" into a reusable capability unit. One `add_skill()` call handles everything.

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

---

## Using Built-in Skills

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant")
    .enable_tool(true);

let mut agent = ReactAgent::new(config);

// Install multiple Skills in one step
agent.add_skill(Box::new(CalculatorSkill));
agent.add_skill(Box::new(FileSystemSkill));
// Equivalent to registering all tools + appending usage instructions to system prompt

let answer = agent.execute("Calculate 42 * 8 and write the result to result.txt").await?;
```

---

## Creating a Custom Skill

Implement the `Skill` trait:

```rust
use echo_agent::skills::Skill;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use echo_agent::error::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

struct SearchTool;
struct SummarizeTool;

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "Search the web for information" }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] })
    }
    async fn execute(&self, _params: ToolParameters) -> Result<ToolResult> {
        Ok(ToolResult::success("Search results...".to_string()))
    }
}

// (SummarizeTool implementation omitted for brevity)

// Bundle tools into a Skill
struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }

    fn description(&self) -> &str { "Web research capability: search + summarize" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SearchTool),
            Box::new(SummarizeTool),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("When you need web information, first use web_search, \
              then use summarize to organize the results. \
              Keep search queries concise — no more than 5 words.".to_string())
    }
}

// Install on an Agent
let mut agent = ReactAgent::new(config);
agent.add_skill(Box::new(ResearchSkill));
```

---

## External Skills (loaded from filesystem)

Beyond code-defined Skills, echo-agent also supports loading **SKILL.md** files from a directory — extending Agent capabilities without modifying source code.

### SKILL.md format

```markdown
---
name: code_review
description: Code review skill
tools:
  - read_file
  - shell
load_on_startup:
  - guidelines.md
---

## Instructions

When asked to review code:
1. Use read_file to read the source file
2. Check against the standards in guidelines.md
3. Output a structured review
```

### Loading external Skills

```rust
// Scan all SKILL.md files under the skills/ directory
let loaded = agent.load_skills_from_dir("./skills").await?;
println!("Loaded skills: {:?}", loaded);
```

### Directory structure

```
skills/
├── code_review/
│   ├── SKILL.md       ← skill definition (YAML frontmatter + instructions)
│   └── guidelines.md  ← load_on_startup resource (auto-injected into system prompt)
└── data_analysis/
    ├── SKILL.md
    └── schema.json
```

---

## Querying Installed Skills

```rust
// List all installed Skills
for info in agent.skill_manager().list() {
    println!(
        "- {} ({} tools, {}prompt injection)",
        info.name,
        info.tool_names.len(),
        if info.has_prompt_injection { "has " } else { "no " }
    );
}

// Check if a Skill is installed
if agent.skill_manager().is_installed("calculator") {
    println!("Calculator skill is installed");
}
```

See: `examples/demo07_skills.rs`, `examples/demo08_external_skills.rs`
