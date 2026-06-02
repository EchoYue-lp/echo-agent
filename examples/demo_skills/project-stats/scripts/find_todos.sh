#!/usr/bin/env bash
# Find TODO, FIXME, HACK, XXX comments in a codebase.
#
# Usage: bash find_todos.sh [directory]
#
# Output: JSON with categorized findings and counts.

set -euo pipefail

DIR="${1:-.}"
DIR="$(cd "$DIR" && pwd)"

# Use rg (ripgrep) if available, fall back to grep
if command -v rg &>/dev/null; then
    GREP_CMD="rg"
    GREP_OPTS=("--no-heading" "--line-number" "--no-messages"
               "--glob" "!.git" "--glob" "!node_modules" "--glob" "!target"
               "--glob" "!*.lock" "--glob" "!package-lock.json")
else
    GREP_CMD="grep"
    GREP_OPTS=("-rn" "--include=*.py" "--include=*.rs" "--include=*.js"
               "--include=*.ts" "--include=*.go" "--include=*.java"
               "--include=*.rb" "--include=*.sh" "--include=*.c"
               "--include=*.cpp" "--include=*.h")
fi

count_pattern() {
    local pattern="$1"
    "$GREP_CMD" "${GREP_OPTS[@]}" -c "$pattern" "$DIR" 2>/dev/null | \
        awk -F: '{sum += $NF} END {print sum+0}'
}

sample_pattern() {
    local pattern="$1"
    local limit="${2:-5}"
    "$GREP_CMD" "${GREP_OPTS[@]}" "$pattern" "$DIR" 2>/dev/null | head -n "$limit"
}

todo_count=$(count_pattern 'TODO')
fixme_count=$(count_pattern 'FIXME')
hack_count=$(count_pattern 'HACK')
xxx_count=$(count_pattern 'XXX')
total=$((todo_count + fixme_count + hack_count + xxx_count))

# Build JSON output
cat <<ENDJSON
{
  "directory": "$DIR",
  "summary": {
    "total": $total,
    "TODO": $todo_count,
    "FIXME": $fixme_count,
    "HACK": $hack_count,
    "XXX": $xxx_count
  },
  "samples": {
    "TODO": [
$(sample_pattern 'TODO' 5 | while IFS= read -r line; do
    escaped=$(echo "$line" | sed 's/\\/\\\\/g; s/"/\\"/g')
    echo "      \"$escaped\","
done)
      null
    ],
    "FIXME": [
$(sample_pattern 'FIXME' 3 | while IFS= read -r line; do
    escaped=$(echo "$line" | sed 's/\\/\\\\/g; s/"/\\"/g')
    echo "      \"$escaped\","
done)
      null
    ]
  }
}
ENDJSON
