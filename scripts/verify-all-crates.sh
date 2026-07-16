#!/usr/bin/env bash
# echo-agent workspace 全量验证脚本
#
# 根 Cargo.toml 同时是 echo_agent package 与 workspace root。workspace 显式包含
# 7 个子 crate,因此 `cargo check/test/clippy --workspace` 会覆盖全部 8 个成员,
# 并统一使用根 Cargo.lock 与根 target 目录。
#
# 本脚本是提交前统一门禁,任一步失败即退出。
#
# 用法:
#   ./scripts/verify-all-crates.sh            # 默认全量(fmt + workspace 默认/all-features/feature 矩阵)
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

# ── 1. fmt ───────────────────────────────────────────────────
if [[ $QUICK -eq 0 ]]; then
    section "cargo fmt --all -- --check"
    if cargo fmt --all -- --check >/dev/null 2>&1; then
        ok "fmt 通过"
    else
        fail "fmt 有未格式化代码,跑 \`cargo fmt --all\` 修复"
    fi
fi

# ── 2. workspace check ───────────────────────────────────────
if [[ $QUICK -eq 0 ]]; then
    section "cargo check --workspace --locked"
    log=$(mktemp)
    if cargo check --workspace --locked >"$log" 2>&1; then
        rm -f "$log"
        ok "workspace check 通过"
    else
        tail -40 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace check 失败"
    fi
fi

# ── 3. workspace test ────────────────────────────────────────
section "cargo test --workspace --locked"
log=$(mktemp)
if cargo test --workspace --locked >"$log" 2>&1; then
    passed=$(grep -oE "[0-9]+ passed" "$log" | awk '{s+=$1} END{print s+0}')
    rm -f "$log"
    ok "workspace 全成员测试通过(${passed} passed)"
else
    tail -60 "$log" | sed 's/^/    /'
    rm -f "$log"
    fail "workspace 测试失败"
fi

# ── 4. workspace clippy ──────────────────────────────────────
if [[ $SKIP_CLIPPY -eq 0 ]]; then
    section "cargo clippy --workspace --all-targets --locked -- -D warnings"
    log=$(mktemp)
    if cargo clippy --workspace --all-targets --locked -- -D warnings >"$log" 2>&1; then
        rm -f "$log"
        ok "workspace clippy 通过(零警告)"
    else
        tail -60 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace clippy 失败"
    fi
fi

# ── 5. feature 矩阵(AGENTS.md 强制)─────────────────────────
if [[ $SKIP_FEATURE -eq 0 ]]; then
    section "feature 矩阵编译(echo_agent 独立 feature)"
    FEATURES=(sqlite subagent human-loop mcp lsp a2a git database rag chart web media)
    FEAT_FAILED=()
    for feat in "${FEATURES[@]}"; do
        printf "  --features %-12s ... " "$feat"
        if cargo check -p echo_agent --no-default-features --features "$feat" --locked >/dev/null 2>&1; then
            printf "${GREEN}ok${NC}\n"
        else
            printf "${RED}FAIL${NC}\n"
            FEAT_FAILED+=("$feat")
        fi
    done
    [[ ${#FEAT_FAILED[@]} -eq 0 ]] || fail "feature 矩阵编译失败的 feature: ${FEAT_FAILED[*]}"
    ok "feature 矩阵全部编译通过"

    section "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
    log=$(mktemp)
    if cargo clippy --workspace --all-targets --all-features --locked -- -D warnings >"$log" 2>&1; then
        rm -f "$log"
        ok "workspace all-features clippy 通过(零警告)"
    else
        tail -80 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace all-features clippy 失败"
    fi

    section "生产目标 panic API 门禁"
    log=$(mktemp)
    if cargo clippy --workspace --lib --bins --all-features --locked -- \
        -D clippy::unwrap_used \
        -D clippy::expect_used \
        -D clippy::panic \
        -D clippy::unreachable >"$log" 2>&1; then
        rm -f "$log"
        ok "workspace 生产目标无 unwrap/expect/panic/unreachable"
    else
        tail -80 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace 生产目标 panic API 门禁失败"
    fi

    section "cargo test --workspace --all-targets --all-features --locked"
    log=$(mktemp)
    if cargo test --workspace --all-targets --all-features --locked >"$log" 2>&1; then
        passed=$(grep -oE "[0-9]+ passed" "$log" | awk '{s+=$1} END{print s+0}')
        rm -f "$log"
        ok "workspace all-features 全目标测试通过(${passed} passed)"
    else
        tail -100 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace all-features 全目标测试失败"
    fi

    section "cargo check --workspace --lib --no-default-features --locked"
    log=$(mktemp)
    if cargo check --workspace --lib --no-default-features --locked >"$log" 2>&1; then
        rm -f "$log"
        ok "workspace 最小 feature 编译通过"
    else
        tail -60 "$log" | sed 's/^/    /'
        rm -f "$log"
        fail "workspace 最小 feature 编译失败"
    fi
fi

section "全部验证通过"
ok "fmt + workspace 默认/all-features check/test/clippy + panic API 门禁 + echo_agent feature 矩阵 全绿"
echo "可安全提交。"
exit 0
