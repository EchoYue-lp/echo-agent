---
name: project-stats
description: Analyze project structure, count lines of code by language, find TODOs/FIXMEs, and generate project health summaries. Use when asked about codebase size, composition, technical debt, or project overview.
license: MIT
metadata:
  author: echo-agent
  version: 2.0.0
  tags: code, analysis, statistics, project
---

## Project Statistics

Skill directory: ${SKILL_DIR}
Session: ${SESSION_ID}

You have access to scripts that analyze codebases. Use them to answer questions
about project size, composition, and health.

### Environment Info

Current host: !`uname -s 2>/dev/null || echo unknown`

### Available Scripts

| Script | Purpose | Usage |
|--------|---------|-------|
| `scripts/count_lines.py` | Count lines of code by language | `run_skill_script("project-stats", "scripts/count_lines.py", "<directory>")` |
| `scripts/find_todos.sh` | Find TODO/FIXME/HACK comments | `run_skill_script("project-stats", "scripts/find_todos.sh", "<directory>")` |
| `scripts/dep_summary.ts` | Summarize project dependencies | `run_skill_script("project-stats", "scripts/dep_summary.ts", "<directory>")` |

### Workflow

1. Ask the user for the project path (or use the current directory)
2. Run `count_lines.py` to get an overview of languages and code volume
3. Run `find_todos.sh` to surface technical debt markers
4. If the project has `package.json` or `Cargo.toml`, run `dep_summary.ts` for dependency info
5. Synthesize findings into a concise report

### Output Format

Present results as a structured report with sections:
- **Overview**: total files, total lines, primary language
- **Language Breakdown**: table of language → files → lines → percentage
- **Technical Debt**: TODO/FIXME count and notable items
- **Dependencies** (if applicable): direct vs dev dependency count
