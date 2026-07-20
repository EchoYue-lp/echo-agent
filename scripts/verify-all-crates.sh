#!/usr/bin/env bash
# echo-agent workspace 验证包装器。
#
# Cargo workspace 已原生覆盖根 crate 和全部子 crate。本脚本不再重复跑
# default/all-features 两套 check/test/clippy；默认门禁与 CI 保持一致。
# 独立 feature 隔离检查只在 feature/cfg/公共 API 变化时显式启用。
#
# 用法:
#   ./scripts/verify-all-crates.sh                  # 提交前门禁(CI 对齐)
#   ./scripts/verify-all-crates.sh --quick          # 迭代快检(default workspace tests)
#   ./scripts/verify-all-crates.sh --feature-matrix # 提交前门禁 + 独立 feature 编译
#
# 退出码:0=全绿,1=有失败或参数错误。

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

QUICK=0
FEATURE_MATRIX=0
for arg in "$@"; do
    case "$arg" in
        --quick) QUICK=1 ;;
        --feature-matrix) FEATURE_MATRIX=1 ;;
        *)
            printf 'Unknown argument: %s\n' "$arg" >&2
            exit 1
            ;;
    esac
done

if [[ $QUICK -eq 1 && $FEATURE_MATRIX -eq 1 ]]; then
    printf '%s\n' '--quick and --feature-matrix cannot be used together' >&2
    exit 1
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

fail() {
    printf "${RED}FAIL: %s${NC}\n" "$1"
    exit 1
}

ok() {
    printf "${GREEN}%s${NC}\n" "$1"
}

section() {
    printf "\n${YELLOW}=== %s ===${NC}\n" "$1"
}

run_logged() {
    local label="$1"
    shift
    local log
    log=$(mktemp)
    if "$@" >"$log" 2>&1; then
        rm -f "$log"
        ok "$label"
    else
        tail -100 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "$label"
    fi
}

if [[ $QUICK -eq 1 ]]; then
    section 'cargo test --workspace --locked'
    run_logged 'workspace default tests passed' \
        cargo test --workspace --locked
    exit 0
fi

section 'cargo fmt --all -- --check'
run_logged 'fmt passed' cargo fmt --all -- --check

section 'cargo clippy --workspace --all-targets --all-features'
run_logged 'all-feature clippy passed with zero warnings' \
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

section 'production panic API gate'
run_logged 'production targets contain no banned panic APIs' \
    cargo clippy --workspace --lib --bins --all-features --locked -- \
    -D clippy::unwrap_used \
    -D clippy::expect_used \
    -D clippy::panic \
    -D clippy::unreachable

section 'cargo test --workspace --all-targets --all-features'
run_logged 'all-feature workspace tests passed' \
    cargo test --workspace --all-targets --all-features --locked

section 'cargo check --workspace --lib --no-default-features'
run_logged 'minimal-feature workspace libraries compile' \
    cargo check --workspace --lib --no-default-features --locked

if [[ $FEATURE_MATRIX -eq 1 ]]; then
    section 'isolated echo_agent feature matrix'
    FEATURES=(sqlite subagent human-loop mcp lsp a2a git database rag chart web media)
    FAILED_FEATURES=()
    for feature in "${FEATURES[@]}"; do
        printf '  --features %-12s ... ' "$feature"
        if cargo check -p echo_agent --no-default-features --features "$feature" --locked \
            >/dev/null 2>&1; then
            printf "${GREEN}ok${NC}\n"
        else
            printf "${RED}FAIL${NC}\n"
            FAILED_FEATURES+=("$feature")
        fi
    done

    if [[ ${#FAILED_FEATURES[@]} -gt 0 ]]; then
        fail "isolated feature checks failed: ${FAILED_FEATURES[*]}"
    fi
    ok 'isolated feature matrix passed'
fi

section 'verification complete'
if [[ $FEATURE_MATRIX -eq 1 ]]; then
    ok 'CI-aligned gate and isolated feature matrix passed'
else
    ok 'CI-aligned workspace gate passed'
fi
