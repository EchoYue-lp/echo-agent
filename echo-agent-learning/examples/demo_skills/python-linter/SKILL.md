---
name: python-linter
description: Lint and format Python files using ruff. Activates automatically when Python files are touched. Use to check code quality and fix style issues.
allowed-tools: read_skill_resource run_skill_script Bash
metadata:
  author: echo-agent
  version: 1.0.0
---

## Python Linter

You have access to a Python linting script. When the user works with `.py` files:

1. Run the linter to check for issues
2. Report findings grouped by severity
3. Suggest auto-fixes where available

### Quick Check

Python available: !`python3 --version 2>&1 || echo "not installed"`

### Usage

```
run_skill_script("python-linter", "scripts/lint.sh", "<file_or_directory>")
```

The script outputs JSON with lint results grouped by file.
