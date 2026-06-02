# Skill Authoring Guide

This guide teaches you how to create custom Skills for echo-agent. For the Skill system API and runtime mechanics, see [07-skills.md](./07-skills.md). This guide focuses on **how to write high-quality Skill content**.

---

## Choosing a Skill Type

| Type | Use Case | Complexity |
|------|----------|------------|
| **File-based** | Domain knowledge packs, prompts + references + scripts | Low (Markdown only) |
| **Code-based** | Custom tool logic, complex computation | High (requires Rust) |

**Guideline:** Domain knowledge and prompts only → File-based; custom tool execution logic → Code-based.

---

## File-based Skills (Recommended for Beginners)

Aligned with the [agentskills.io](https://agentskills.io/specification) specification. No code changes needed.

### Directory Structure

```
skills/
└── my-skill/
    ├── SKILL.md              # Required: skill definition
    ├── references/           # Optional: reference docs
    │   └── guide.md
    └── scripts/              # Optional: executable scripts
        └── analyze.py
```

### SKILL.md Format

```markdown
---
name: my-skill
description: Short description of the skill
version: 1.0.0
author: Your Name
tags: [domain, category]
activation_patterns:
  - "keyword1"
  - "keyword2"
---

# My Skill

## When to Use
Describe when the Agent should activate this skill.

## Usage Guide
Detailed step-by-step instructions.

## Available Resources
- `references/guide.md` - Detailed guide

## Available Scripts
- `scripts/analyze.py` - Data analysis script
  - Args: `--input <file>` `--output <file>`
```

### Three-Tier Progressive Disclosure

| Tier | Content | Trigger | Token Cost |
|------|---------|---------|-----------|
| **Tier 1** | Name + description | Auto-scanned at startup | ~50-100 / skill |
| **Tier 2** | Full guide + resource list | `activate_skill` | <5000 / skill |
| **Tier 3** | Reference files / scripts | `read_skill_resource` / `run_skill_script` | On demand |

**Principle:** Keep Tier 1 minimal (frontmatter only), Tier 2 complete but under 5000 tokens, Tier 3 loaded on demand.

### Writing High-Quality SKILL.md

**✅ Good practices:**
- Precise `activation_patterns` (avoid overly broad patterns)
- Structured guide with headings, lists, code blocks
- Output format examples
- Keep Tier 2 under 5000 tokens

**❌ Avoid:**
- Vague descriptions (e.g., `description: code-related features`)
- Missing concrete steps
- Overly long SKILL.md (split into references/)

### Writing Scripts

Scripts go in `scripts/`, supporting Python, Bash, Node.js, etc.:

```python
#!/usr/bin/env python3
"""Data analysis script"""
import argparse
import pandas as pd

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--input', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    
    df = pd.read_csv(args.input)
    report = f"# Analysis Report\n\nRows: {len(df)}\n"
    
    with open(args.output, 'w') as f:
        f.write(report)

if __name__ == '__main__':
    main()
```

Reference in SKILL.md:

```markdown
## Available Scripts
- `scripts/analyze.py` - Analyze CSV data
  - Args: `--input <csv>` `--output <md>`
  - Dependencies: `pandas`
```

### Testing

```bash
# Place in project-level or user-level directory
mkdir -p ./skills/my-skill

# Verify loading
agent.discover_skills(vec!["./skills".into()]).await?;
```

---

## Code-based Skills

Implement the `Skill` trait for custom tool logic:

```rust
use echo_agent::skills::Skill;
use echo_agent::tools::Tool;

pub struct MySkill;

impl Skill for MySkill {
    fn name(&self) -> &str { "my-skill" }
    fn description(&self) -> &str { "Describe skill purpose" }
    
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(MyTool1), Box::new(MyTool2)]
    }
    
    fn system_prompt_injection(&self) -> Option<String> {
        Some("When the user requests X, call my_tool1 first, then my_tool2.".into())
    }
}

// Register
agent.add_skill(Box::new(MySkill));
```

See `demo07_skills.rs` for a complete example.

---

## Best Practices

| Principle | Description |
|-----------|-------------|
| **Naming** | Skills: `kebab-case` (`code-review`), tools: `snake_case` (`web_search`) |
| **Description** | Be specific: `"Professional code review for defects and security vulnerabilities"` |
| **Activation** | Precise: `["review code", "check code"]`, avoid `["code"]` |
| **Prompts** | Concise (<200 words), clear action guidelines |
| **Error handling** | Validate input, return clear error messages |

---

## See Also

- [Skill System API](./07-skills.md) — SkillRegistry, DiscoveryScope, activate_skill
- [agentskills.io specification](https://agentskills.io/specification)
- `demo07_skills.rs` — Code-based Skill example
- `demo08_external_skills.rs` — File-based Skill example
