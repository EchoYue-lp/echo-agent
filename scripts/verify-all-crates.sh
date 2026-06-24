#!/usr/bin/env bash
# 逐 crate 全量验证脚本
#
# 背景:echo-agent 根 Cargo.toml 是普通 package(非 workspace),根 crate 通过
# path 依赖引入子 crate。`cargo test --workspace` 只覆盖根 crate(echo_agent),
# 子 crate(echo_core/echo_execution/...)的测试**从不被 --workspace 编译**,
# 导致子 crate 测试的编译错误长期不可见(曾隐藏 dependency_probe 编译错误)。
#
# 本脚本逐个 `cargo test -p <crate>` 跑全部子 crate + 根 crate,任一失败即退出,
# 确保子 crate 测试不再被隐藏。提交前必跑。
#
# 用法:
#   ./scripts/verify-all-crates.sh            # 默认全量(test + clippy + fmt + feature 矩阵)
#   ./scripts/verify-all-crates.sh --quick    # 只跑 test,跳过 clippy/fmt/feature(快速迭代)
#   ./scripts/verify-all-crates.sh --no-clippy --no-feature  # 跳过指定项
#
# 退出码:0=全绿,1=有失败。

# 注意:不用 set -e,因为我们要精细处理 cargo/grep 的非零退出码(管道里 grep 无匹配
# 返回 1 是正常的,不该触发退出)。手动检查每步退出码。

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

QUICK=0
SKIP_CLIPPY=0
SKIP_FEATURE=0
for arg in "$@"; do
    case "$arg" in
        --quick) QUICK=1; SKIP_CLIPPY=1; SKIP_FEATURE=1 ;;
        --no-clippy) SKIP_CLIPPY=1 ;;
        --no-feature) SKIP_FEATURE=1 ;;
    esac
done

# 用 printf 而非 echo -e,兼容 sh/bash(脚本被 `sh ./` 调用时 echo -e 不识别)。
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
fail() { printf "${RED}❌ FAIL: %s${NC}\n" "$1"; exit 1; }
ok()   { printf "${GREEN}✅ %s${NC}\n" "$1"; }
section() { printf "\n${YELLOW}=== %s ===${NC}\n" "$1"; }

# 全部 crate:7 个子 crate + 根 crate(echo_agent)
CRATES=(
    echo_core
    echo_macros
    echo_execution
    echo_integration
    echo_tools
    echo_state
    echo_orchestration
    echo_agent
)

# ── 1. fmt ───────────────────────────────────────────────────
if [[ $QUICK -eq 0 ]]; then
    section "cargo fmt --all -- --check"
    if cargo fmt --all -- --check >/dev/null 2>&1; then
        ok "fmt 通过"
    else
        fail "fmt 有未格式化代码,跑 \`cargo fmt --all\` 修复"
    fi
fi

# ── 2. 逐 crate test(核心:防子 crate 测试被隐藏)─────────
section "逐 crate cargo test(防子 crate 测试被 --workspace 隐藏)"
FAILED_CRATES=()
for crate in "${CRATES[@]}"; do
    printf "  %-20s ... " "$crate"
    log=$(mktemp)
    # 先跑 cargo test,捕获完整输出和退出码(不用管道 grep,避免 grep 无匹配误判)
    if cargo test -p "$crate" >"$log" 2>&1; then
        # 退出码 0:进一步检查输出里有没有 FAILED(error\[ 不会让 cargo test 返回非零?实际会,但双保险)
        if grep -qE "FAILED|error\[|error:" "$log"; then
            printf "${RED}FAIL(有 FAILED/error)${NC}\n"
            grep -E "FAILED|error\[|error:" "$log" | head -10 | sed 's/^/      /'
            FAILED_CRATES+=("$crate")
        else
            passed=$(grep -oE "[0-9]+ passed" "$log" | awk '{s+=$1} END{print s+0}')
            printf "${GREEN}ok${NC} (%s passed)\n" "$passed"
        fi
    else
        printf "${RED}FAIL(退出码非零)${NC}\n"
        tail -20 "$log" | sed 's/^/      /'
        FAILED_CRATES+=("$crate")
    fi
    rm -f "$log"
done

if [[ ${#FAILED_CRATES[@]} -gt 0 ]]; then
    fail "以下 crate 测试失败: ${FAILED_CRATES[*]}"
fi
ok "全部 ${#CRATES[@]} 个 crate 测试通过"

# ── 3. clippy(逐 crate,因为 --all-targets 在非 workspace package 下只覆盖根 crate)
if [[ $SKIP_CLIPPY -eq 0 ]]; then
    section "逐 crate cargo clippy --all-targets -- -D warnings"
    CLIPPY_FAILED=()
    for crate in "${CRATES[@]}"; do
        printf "  %-20s ... " "clippy $crate"
        if cargo clippy -p "$crate" --all-targets -- -D warnings >/dev/null 2>&1; then
            printf "${GREEN}ok${NC}\n"
        else
            printf "${RED}FAIL${NC}\n"
            CLIPPY_FAILED+=("$crate")
        fi
    done
    [[ ${#CLIPPY_FAILED[@]} -eq 0 ]] || fail "clippy 失败的 crate: ${CLIPPY_FAILED[*]}"
    ok "clippy 全部通过(零警告)"
fi

# ── 4. feature 矩阵(AGENTS.md 强制)─────────────────────────
if [[ $SKIP_FEATURE -eq 0 ]]; then
    section "feature 矩阵编译(echo_agent 独立 feature)"
    FEATURES=(sqlite subagent human-loop mcp lsp a2a git database rag chart web media)
    FEAT_FAILED=()
    for feat in "${FEATURES[@]}"; do
        printf "  --features %-12s ... " "$feat"
        if cargo check -p echo_agent --no-default-features --features "$feat" >/dev/null 2>&1; then
            printf "${GREEN}ok${NC}\n"
        else
            printf "${RED}FAIL${NC}\n"
            FEAT_FAILED+=("$feat")
        fi
    done
    [[ ${#FEAT_FAILED[@]} -eq 0 ]] || fail "feature 矩阵编译失败的 feature: ${FEAT_FAILED[*]}"
    ok "feature 矩阵全部编译通过"
fi

section "全部验证通过"
ok "fmt + 逐 crate test + clippy + feature 矩阵 全绿"
echo "可安全提交。"
exit 0
