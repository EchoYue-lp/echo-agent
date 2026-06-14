# Skill System Design

This document describes the Skill system design of echo-agent, aligned with the [agentskills.io](https://agentskills.io/) specification.

---

## Overview

A Skill is a higher-level capability abstraction than a Tool. A Skill can contain multiple related tools + optional system prompt injection, forming a complete capability package.

### Skill vs Tool

| Dimension | Tool | Skill |
|-----------|------|-------|
| Granularity | Single atomic operation | Domain capability package (multiple tools + prompts) |
| Registration | `agent.add_tool(box)` | `agent.add_skill(box)` |
| Prompt | None | Can inject system prompts |
| Semantics | "Do one thing" | "I am proficient in a domain" |

---

## Two Skill Types

echo-agent supports two types of Skills:

### 1. Code-based Skills

Defined directly in Rust code, take effect immediately upon registration.

```rust
// src/skills/mod.rs
pub trait Skill: Send + Sync {
    /// Unique identifier (lowercase, e.g., "calculator")
    fn name(&self) -> &str;
    
    /// Human-readable description
    fn description(&self) -> &str;
    
    /// Provided tools (returns new instances each call)
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    
    /// Optional: text to inject into the system prompt
    fn system_prompt_injection(&self) -> Option<String> {
        None
    }
}
```

**Example: Research Skill**

```rust
pub struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }

    fn description(&self) -> &str {
        "Web research capability: searches the web and summarizes findings"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SearchTool),
            Box::new(FetchTool),
            Box::new(SummarizeTool),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("You have web research capabilities. When you need fresh information, first call search, then fetch the most relevant pages and summarize.".into())
    }
}

// Registration
agent.add_skill(Box::new(ResearchSkill));
```

### 2. File-based Skills

Defined via `SKILL.md` files, supporting Progressive Disclosure.

**Directory structure:**

```
skills/
├── code_review/
│   ├── SKILL.md          # Skill definition (required)
│   └── references/
│       ├── checklist.md  # Reference documents
│       └── style_guide.md
│
├── data_analyst/
│   ├── SKILL.md
│   └── references/
│       └── statistical_methods.md
│
└── web_researcher/
    ├── SKILL.md
    └── references/
        ├── research_template.md
        └── source_evaluation.md
```

**SKILL.md format:**

```markdown
---
name: code_review
description: Comprehensive code review capability
version: 1.0.0
author: echo-agent
---

# Code Review Skill

You are an expert code reviewer. When reviewing code, consider:

1. **Correctness**: Does the code do what it's supposed to?
2. **Performance**: Are there any obvious inefficiencies?
3. **Security**: Are there potential vulnerabilities?
4. **Readability**: Is the code easy to understand?

## Tools

- `review_file`: Review a single file
- `suggest_fix`: Suggest improvements

## References

- [Review Checklist](references/checklist.md)
- [Style Guide](references/style_guide.md)
```

---

## Progressive Disclosure

File-based Skills use a three-level progressive disclosure approach to minimize initial context overhead:

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Progressive Disclosure                             │
│                                                                      │
│   Level 1: Discovery                                                │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  Agent scans the skills/ directory at startup               │   │
│   │  Creates a SkillDescriptor for each SKILL.md               │   │
│   │  Registers a DiscoverySkillTool for subsequent activation   │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Level 2: Activation                                               │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  LLM calls discovery_skill to activate a skill              │   │
│   │  Loads full SKILL.md content and injects into system prompt │   │
│   │  Registers all tools provided by the skill                  │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Level 3: Usage                                                    │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  LLM calls specific tools to complete tasks                 │   │
│   │  Can access reference documents under references/           │   │
│   │  Intercepts and enhances tool calls via hooks               │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### Built-in Discovery Tool

```rust
// src/skills/external/discovery.rs

/// Discover available skills
pub struct DiscoverySkillTool;

impl Tool for DiscoverySkillTool {
    fn name(&self) -> &str { "discovery_skill" }
    fn description(&self) -> &str { "Discover and activate a skill" }
    
    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let skill_name: String = params.get("name")?;
            // Activate the skill
            // Inject prompt + register tools
            Ok(ToolResult::success(format!("Skill {} activated", skill_name)))
        })
    }
}
```

---

## SkillRegistry

```rust
// src/skills/registry.rs
pub struct SkillRegistry {
    // Code-based skills
    skills: HashMap<String, Box<dyn Skill>>,
    
    // File-based skills (descriptors only)
    descriptors: Vec<SkillDescriptor>,
    
    // Hooks for tool call interception
    hooks: HookRegistry,
}

impl SkillRegistry {
    /// Register a code-based Skill
    pub fn register(&mut self, skill: Box<dyn Skill>) {
        let info = SkillInfo {
            name: skill.name().to_string(),
            description: skill.description().to_string(),
            tool_names: skill.tools().iter().map(|t| t.name().to_string()).collect(),
            has_prompt_injection: skill.system_prompt_injection().is_some(),
        };
        self.skills.insert(info.name.clone(), skill);
    }
    
    /// Discover file-based Skills
    pub fn discover(&mut self, path: &Path) -> Result<Vec<SkillDescriptor>> {
        for entry in fs::read_dir(path)? {
            let skill_path = entry?.path().join("SKILL.md");
            if skill_path.exists() {
                let descriptor = SkillDescriptor::from_file(&skill_path)?;
                self.descriptors.push(descriptor);
            }
        }
        Ok(self.descriptors.clone())
    }
    
    /// Activate a file-based Skill
    pub fn activate(&mut self, name: &str) -> Result<SkillContent> {
        let descriptor = self.descriptors.iter()
            .find(|d| d.name == name)
            .ok_or(SkillError::NotFound)?;
        
        let content = descriptor.load_content()?;
        // Register tools, inject prompts
        Ok(content)
    }
}
```

---

## Hook System

Skills can define Hooks to intercept and enhance tool calls:

```rust
// src/skills/hooks.rs
pub struct HookRegistry {
    rules: Vec<HookRule>,
}

pub struct HookRule {
    pub skill_name: String,
    pub tool_pattern: Regex,
    pub event: HookEvent,
    pub action: HookAction,
}

pub enum HookEvent {
    BeforeCall,
    AfterCall,
    OnError,
}

pub enum HookAction {
    Transform(Box<dyn Fn(&mut Value) -> Result<()> + Send + Sync>),
    Validate(Box<dyn Fn(&Value) -> Result<()> + Send + Sync>),
    Log(Box<dyn Fn(&str, &Value) + Send + Sync>),
}
```

**Example: Code Review Skill Hook**

```yaml
# skills/code_review/SKILL.md
---
name: code_review
hooks:
  - tool: "review_file"
    event: before_call
    action: validate_file_exists
  - tool: "suggest_fix"
    event: after_call
    action: format_as_diff
---
```

---

## Usage Examples

### Code-based Skill

```rust
use echo_agent::prelude::*;
use echo_agent::skills::Skill;

// Define a Skill
struct GitWorkflowSkill;

impl Skill for GitWorkflowSkill {
    fn name(&self) -> &str { "git-workflow" }
    fn description(&self) -> &str { "Git operations: branches, commits, PR/MR, conflict resolution" }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(GitStatusTool),
            Box::new(GitDiffTool),
        ]
    }
    fn system_prompt_injection(&self) -> Option<String> {
        Some("You can perform git workflow operations. Always check status before making changes.".into())
    }
}

// Registration
let mut agent = ReactAgentBuilder::simple("qwen3-max", "Assistant")?;
agent.add_skill(Box::new(GitWorkflowSkill));
```

### File-based Skill

```rust
use echo_agent::skills::SkillRegistry;

// Discovery
let mut registry = SkillRegistry::new();
let discovered = registry.discover(Path::new("skills/"))?;

println!("Discovered {} skills:", discovered.len());
for desc in &discovered {
    println!("  - {}: {}", desc.name, desc.description);
}

// Use within an Agent
agent.set_skill_registry(registry);

// The LLM can call discovery_skill to activate the desired skill
```

---

## agentskills.io Specification

echo-agent's Skill system is aligned with the [agentskills.io](https://agentskills.io/specification) specification:

| Specification Requirement | echo-agent Implementation |
|---------------------------|---------------------------|
| SKILL.md format | ✓ Supports YAML frontmatter |
| Progressive Disclosure | ✓ Three levels: Discovery → Activation → Usage |
| Tool registration | ✓ Automatic registration |
| Prompt injection | ✓ System prompt appended |
| Reference documents | ✓ references/ directory |
| Hooks | ✓ Interception and enhancement |

---

## Best Practices

### 1. Skill Naming

- Use lowercase letters and underscores: `code_review`, `data_analyst`
- Names should reflect the domain: `web_researcher` rather than `research`
- Avoid conflicts with built-in Skills

### 2. Prompt Injection

```rust
fn system_prompt_injection(&self) -> Option<String> {
    Some(r#"
## Research Skill

You have web research capabilities. Use the following tools:

- search: search the web with a query
- fetch: download a specific URL's contents
- summarize: condense a fetched page

Note: prefer authoritative sources; cross-check claims across at least two pages.
"#.into())
}
```

### 3. Tool Design

- Tools within a Skill should be related and cohesive
- Avoid mixing unrelated tools in a single Skill
- Tool descriptions should clearly state their purpose

### 4. File-based Skill Directory Structure

```
skills/
└── my_skill/
    ├── SKILL.md              # Required
    ├── references/           # Optional: reference documents
    │   ├── guide.md
    │   └── examples.md
    └── scripts/              # Optional: script-based tools
        └── helper.py
```

---

## References

- [agentskills.io Specification](https://agentskills.io/specification)
- [LangChain Tools](https://python.langchain.com/docs/modules/tools/)
- [CrewAI Tools](https://docs.crewai.com/core-concepts/Tools)