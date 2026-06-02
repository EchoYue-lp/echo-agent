#!/usr/bin/env bash
# Lint Python files using ruff (falls back to basic checks if ruff is not installed)
set -euo pipefail

TARGET="${1:-.}"

if command -v ruff &>/dev/null; then
    ruff check --output-format json "$TARGET" 2>/dev/null || true
else
    echo '{"warning": "ruff not installed, running basic syntax check"}'
    find "$TARGET" -name '*.py' -print0 | while IFS= read -r -d '' file; do
        python3 -c "import py_compile; py_compile.compile('$file', doraise=True)" 2>&1 || true
    done
fi
