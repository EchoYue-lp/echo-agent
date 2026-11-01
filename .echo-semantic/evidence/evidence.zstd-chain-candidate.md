---
schema_version: 1
kind: evidence
id: evidence.zstd-chain-candidate
observed_at: source:2ec5603be1a58029de731444394655a2d86af4a921218b37e652b842de3c8d58
source_refs:
- Cargo.toml
- Cargo.lock
- vendor/README.md
- vendor/registry-origins.toml
- vendor/polars-arrow-0.55.2/Cargo.toml
- vendor/polars-parquet-0.55.2/Cargo.toml
- scripts/generate-vendored-media-audit-lock.py
- scripts/tests/test_vendored_media_audit_lock.py
- echo-tools/tests/zstd_data_compat.rs
supports:
- rule.framework-layer-ownership
- evidence.framework-dependency-policy-repair
limitations:
- 当前只覆盖候选源码、锁、审计归因和 focused 依赖/消费者验证；full24、SDK consumer、远端 main/CI 与发布资格另行验收。
- safe7 到 safe8 的嵌套外部 crate 类型身份变化保留为明确兼容边界，不承诺旧显式类型赋值兼容。
---

## 支持的结论

该 Evidence 绑定 F340 现成依赖闭包、当前 candidate 锁与已执行的 task-scoped 依赖/消费者回执。

## 来源与范围

来源是 candidate 的 Cargo manifest/lock、vendor manifest/归因、唯一审计生成器、现有消费者测试及对应 raw/receipt；只覆盖本 Framework candidate。

## 已知缺口

完整 full24、SDK consumer、远端 main/CI、发布与 native host 资格仍未由本 Evidence 证明。

# Framework zstd 安全链候选（F517→b788）

## 事实与唯一来源

候选根是 F517 `51726454985fd6f064b21a7e9c539e22d494c987` 的后继 b788 `b78868787014ea89a12e3ec6362c89f2e8d1dbee`，父提交为 `2f8123246f51a56b6a247cdf18423df197390c50`。formal F340 的现成 zstd 修复位于提交 `e0cda8f95e6695a99c04ded2285057b5fa68d9f3`（其父为 `0e404185c8f252da3e335c35547e52249c74dc65`）；本候选只复用其中的 zstd vendor 闭包，不整批 cherry-pick，也不带入该提交的 A2A/LSP 退役、测试 profile 或其它源改动。

复用的两个官方 0.55.2 包共 445 个文件、2,779,352 bytes，逐文件 SHA 与 mode 均等于 F340 Git blob：

- `vendor/polars-arrow-0.55.2`，上游 commit `d7488c71ecfbc77790292ff5b365b991c08380ce`，原 crate SHA-256 `c5ab55460ca2553d1a2bbaf2046998cfbbb685bc18d1e168b4367b63301eb271`。
- `vendor/polars-parquet-0.55.2`，同一上游 commit，原 crate SHA-256 `d33a17979d2afa0309437478662cf996361a6e1768bf467db82bd4f4a9443064`。

两份 normalized manifest 唯一补丁都是 `dependencies.zstd.version = "0.14.3"`；Rust 源、`Cargo.toml.orig`、VCS 与许可成员保持 F340 blob 原样。根 `Cargo.toml` 仅增加两个 vendor exclude 与两条 `[patch.crates-io]` path 选择。

## 锁候选

原 F517 `Cargo.lock` SHA-256 为 `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c`；本次离线、精确命令生成的候选锁 SHA-256 为 `511eb9822839e252fcb5286602691e96a6277f586103553d126e3df31ff20053`：

```text
RUSTUP_HOME=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/root-rust-toolchain-20261029/rustup-home
RUSTUP_TOOLCHAIN=eko-native-1.99.1
RUSTC=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/root-rust-toolchain-20261029/rust-1.99.1-frozen/bin/rustc
RUSTDOC=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/root-rust-toolchain-20261029/rust-1.99.1-frozen/bin/rustdoc
CARGO_NET_OFFLINE=true cargo +eko-native-1.99.1 update --manifest-path <candidate>/Cargo.toml --offline -p zstd --precise 0.14.3
```

解析输出只报告：两个 `polars` registry package 转为既有 path patch、`zstd 0.13.3 → 0.14.3`、`zstd-safe 7.2.4 → 8.2.0`、`zstd-sys 2.0.16+zstd.1.5.7 → 2.1.2+zstd.1.5.7`。三条 zstd 节点的 registry checksum 分别更新为：zstd `db3ae9bf90af6e40197bef767eeefc46ad6397420fe873830bd85e5fbbfb3c70`、zstd-safe `ded520f16b29e1a9b771c722221b2b0a0398c2316d9bb3c42b458a5414d7434b`、zstd-sys `481ada9c543b2323cbb7bbdbb5249177fbf0188ed959ed2c15e4b22421c84ed3`。

## 非 zstd 的 `tempfile` 边

同一次官方 resolver 还把 `tempfile 3.27.0` 的锁边从 `getrandom 0.4.3` 改为 `getrandom 0.3.4`。这不是 zstd/zstd-safe/zstd-sys 的直接依赖：三条 zstd manifest/锁依赖均只到 `zstd-sys → cc/pkg-config`。

`tempfile 3.27.0` 的真实 registry manifest 明确写着：

```toml
[target.'cfg(any(unix, windows, target_os = "wasi"))'.dependencies.getrandom]
version = ">=0.3.0, <0.5"
optional = true
default-features = false
```

因此 0.3.4 与 0.4.3 都满足同一 optional、default-features=false 约束；`tempfile` 自身版本/checksum 未变，两个 `getrandom` package 节点及各自 checksum 也都保留。0.3.4 checksum 为 `899def5c37c4fd7b2664648c28120ecec138e4d395b459e5ca34f9cce2dd77fd`，0.4.3 checksum 为 `300e883d756b2e4ec94e02791f39b04b522276138852cfc41d9fb7e904106099`。

这条边是 Cargo 精确 resolver 的合法范围内选择，但不是 zstd 安全闭包的必要变更。当前 macOS 目标两版本都走 `libc` 分支；跨 wasm/UEFI 的依赖子图不同（0.3.4 使用 `r-efi 5.3.0`，0.4.3 使用 `r-efi 6.0.0`，且 0.4.3 另有可选 `rand_core 0.10.1`），所以候选不宣称跨平台行为等价，也不把这条边写成新的风险接受。是否保留该 resolver 选择由后续锁审复核决定；本候选未手工改回或静默接受。

## duplicate 基线与范围

F517 原 baseline 与 F340 baseline 均为 cargo-deny 0.20.2、49 组/107 个 name-version-source identity，组及 identity 集合逐项相同，policy SHA 均为 `d2847230cc85ea94f95f616d258ec212d8aa487397b91d0f65e393ee97e38db9`；baseline 文件自身的 `lock_sha256` 和历史验证文字随锁不同而不同。没有新增 ignore/allow、风险接受或 SDK/CLI pin。

初始候选阶段尚未运行 Cargo build/test、cargo metadata、cargo-deny、cargo audit、fmt、clean 或完整矩阵；随后已按受控顺序完成 metadata、cargo-deny exact49、五源 cargo-audit、fmt write/check 与 Zstd consumer test。Clippy 仍由独立 free-space guard 控制，完整矩阵尚未运行。01–22 实际成功制品已独立冷归档于 `../stage0-main-gate-artifact-preservation-20261101-r02/`，归档 SHA-256 为 `09e7207a7f52166ccd1d6e9054b81e59778100226d47d6211c9ac9d4b41c2790`，流式成员 SHA 回读通过。

## 审计与消费者验证闭包

本候选同时复用了 F340 的最小来源归因闭包，避免只靠 path package 让官方 scanner 跳过 Polars：`vendor/registry-origins.toml` 现声明原三个媒体父包与 `polars-arrow`/`polars-parquet` 两项（五项的 name/version/checksum/manifest）；`scripts/generate-vendored-media-audit-lock.py` 仍是唯一生成器，保留 `echo_tools` owner、版本/source/checksum、缺包/重复/错版/错来源、输入覆盖与只投影 source/checksum 的全部校验；`scripts/tests/test_vendored_media_audit_lock.py` 复用 F340 的五项、media-only、Arrow-only、缺包引用、错版本/source、重复与 hardlink 覆盖控制。现有 `.github/workflows/rust-ci.yml` 已调用该测试、生成器和临时 registry-origin audit，未新增第二条 CI 算法。

`echo-tools/tests/zstd_data_compat.rs` 复用 F340 现成消费者测试，覆盖 DataExport/DataRead/load_dataframe 的真实 Zstd Parquet、Unicode/null/数字保真、坏 Parquet 与不支持格式错误；`media` 下再覆盖 Excel 写入/读取到 Zstd Parquet，`statistics` 下保留 statistics consumer 断言。该测试尚未在当前候选运行。

## 当前候选实际门禁回执

Root 复核后，本候选仅把 `.cargo/cargo-deny-duplicate-baseline.json` 的 `lock_sha256` 从 F517 原锁 `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c` 机械绑定到候选锁 `511eb9822839e252fcb5286602691e96a6277f586103553d126e3df31ff20053`；policy `d2847230cc85ea94f95f616d258ec212d8aa487397b91d0f65e393ee97e38db9`、`ACCEPTED_BY_HUMAN`、49 组/107 identities 与历史字段逐项保持。绑定后 baseline 文件 SHA 为 `f4084e887ac399cecb7a33f89ae2312b94c1da892c90858b715b18c3c106b177`。

实际回执已保存：正确原 process-only RustSec route 下的 cargo-deny raw 745,838 bytes，SHA `0eebdcadc7a408b4a023d2ca7f3b1f70e460366fab53841f7c6449b0db8b8f2f`，自然退出 0；未改 checker 的 derived receipt exact49/107 为 `matches_exact_baseline=true`。五源 registry-origin audit raw 419 bytes，SHA `cf46d0e184e91f19845d7da997feab596d3dac4b3dae2f2846fc9c4d7c26b7a0`，自然退出 0，加载 1395 advisories、扫描 679 crate dependencies，无 signal/error/truncation。r01 错误 GIT_CONFIG_GLOBAL 导致的自然 1 原件继续保留，不被覆盖。

当前实际 focused 结果为 fmt write/check 自然 0、Zstd consumer test 3 passed；受影响 Clippy 在恢复资源前由 free-space guard 拒绝，未启动。当前只确认第一方公开 Rust 签名与协议未增删；safe7→safe8 的嵌套 `DecompressionContext::Zstd` / `zstd_safe::DCtx` 类型身份变化仍按 ADR 例外保留，不宣称所有第三方 Rust API 赋值兼容。既有 docs/zh、examples 与 website 没有新的 API/产品行为文本需要同步；`zstd_data_compat.rs` 已纳入 workspace all-target 适用测试，官网不复制依赖审计实现或临时锁。


## Focused 验证结果

baseline 机械绑定后，正确 process-only Git 配置下的 fmt write 与 fmt check 均自然 0、source drift null。`cargo test --manifest-path <candidate>/Cargo.toml -p echo_tools --all-features --test zstd_data_compat --locked` 实际自然 0，3 passed / 0 failed / 0 ignored；raw SHA `1aa7059b5c980aea1fd928767fc3f7397001e3ee823c87d7a8a832dd581f500a`，receipt SHA `aa4e5e0062ed163658b46b078877057642769b779fb3444d208d407fca0ca477`。

受影响 Clippy 命令在 run-command 的 fresh free >=5 GiB guard 处被拒绝，未启动 cargo/clippy：`free disk under5GiB`；当前 free 5,134,804 KiB（约 4.90 GiB），失败记录 `zstd-candidate-r03-clippy-guard-failure.json`。不绕过 guard、不 clean、不把该项写成通过。

## 资源修复与 warning 边界

资源窗口恢复后，受影响 `echo_tools` all-targets/all-features Clippy 自然退出 0，但 raw `c4c2e65d94eff070a8ffc9678551b81882c62a4e84164295869a0ec95bd5ec34` 含两处 `rust-objcopy` SIGABRT warning（`polars-arrow` build、`echo_macros` lib），所以不计 W0。原因是该批 focused env 尚未投影 frozen LLVM 的 `DYLD_LIBRARY_PATH`；原 warning raw/receipt 与实际 zstd test 制品均已冷保全。

按窄范围恢复授权执行 `cargo clean --dry-run -p echo_tools --profile dev --target-dir <原 warm T>`，Cargo 以 101 拒绝，原因是 target 缺失/无效 `CACHEDIR.TAG`；未伪造 TAG、未执行 clean。保全目录为 `zstd-candidate-r03-warning-preservation-r01`，manifest SHA `2b3a296d9b9e35e08d60ff4601122449db4cfe7f3f153723df8cbf3043b829f9`。后续若 Root 修复自有 target 标签并取得新窗口，只重建该包缓存再复核，不能把当前 Clippy 0 冒充无 warning。

## 正确 DYLD 环境重建结果

按授权仅移除坏 executable 后，使用 frozen LLVM 的 `DYLD_LIBRARY_PATH=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/root-rust-toolchain-20261029/rust-1.99.1-frozen/lib` 重建并重跑：`zstd_data_compat` 实际 3 passed / 0 failed、raw SHA `3a5d227923cd75d8aadbab8bf78492333836bf905b6cd5b9703913fe0ad78a47`，全文无 warning；`echo_tools` all-targets/all-features Clippy `-D warnings` 自然 0、raw SHA `3b8580c9c4463051c1d172c1b90375c39540f136da7d24246ff1771a04497e92`，全文无 warning，source drift 均为 null。成功 test/Clippy 的程序、FP、`.d` 与 raw/receipt 已保全于 `zstd-candidate-r07-success-preservation-r02`，manifest SHA `9a462d17b1bd30fd9abfc6d4117f0f0e31306a9b20c40d39c0b43062bf2f8c56`。

之前未带 DYLD 的 3-pass/W1 与 Clippy 0/W2 原件继续保留；不能用旧 raw 替代本次无 warning 回执。


## Vendor Rust 成员清单（固定上游来源）

以下清单逐项绑定本次借用的 0.55.2 上游 Rust 成员；它们保留原 source/vendor bytes，唯一 manifest 差异是 zstd 版本选择。该清单只用于 literal path coverage，不建立独立语义 Asset，也不宣称逐函数审计：

- `vendor/polars-arrow-0.55.2/build.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/from.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/mutable_values.rs`
- `vendor/polars-arrow-0.55.2/src/array/binary/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/binview/view.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/from.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/boolean/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/typed_iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/dictionary/value_map.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/binary.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/binary_view.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/dictionary.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/fixed_size_binary.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/fixed_size_list.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/list.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/map.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/null.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/primitive.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/struct_.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/union.rs`
- `vendor/polars-arrow-0.55.2/src/array/equal/utf8.rs`
- `vendor/polars-arrow-0.55.2/src/array/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_binary/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/fixed_size_list/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/indexable.rs`
- `vendor/polars-arrow-0.55.2/src/array/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/list/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/map/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/map/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/map/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/map/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/null.rs`
- `vendor/polars-arrow-0.55.2/src/array/physical_binary.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/from_natural.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/primitive/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/specification.rs`
- `vendor/polars-arrow-0.55.2/src/array/static_array.rs`
- `vendor/polars-arrow-0.55.2/src/array/static_array_collect.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/builder.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/struct_/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/array/total_ord.rs`
- `vendor/polars-arrow-0.55.2/src/array/union/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/union/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/union/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/union/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/ffi.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/from.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/mod.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/array/utf8/mutable_values.rs`
- `vendor/polars-arrow-0.55.2/src/array/values.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/aligned.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/assign_ops.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/bitmap_ops.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/bitmask.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/builder.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/immutable.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/mod.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/mutable.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/proptest.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/chunk_iterator/chunks_exact.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/chunk_iterator/merge.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/chunk_iterator/mod.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/chunks_exact_mut.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/fmt.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/mod.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/slice_iterator.rs`
- `vendor/polars-arrow-0.55.2/src/bitmap/utils/zip_validity.rs`
- `vendor/polars-arrow-0.55.2/src/compute/aggregate/memory.rs`
- `vendor/polars-arrow-0.55.2/src/compute/aggregate/mod.rs`
- `vendor/polars-arrow-0.55.2/src/compute/arity.rs`
- `vendor/polars-arrow-0.55.2/src/compute/arity_assign.rs`
- `vendor/polars-arrow-0.55.2/src/compute/bitwise.rs`
- `vendor/polars-arrow-0.55.2/src/compute/concatenate.rs`
- `vendor/polars-arrow-0.55.2/src/compute/decimal.rs`
- `vendor/polars-arrow-0.55.2/src/compute/mod.rs`
- `vendor/polars-arrow-0.55.2/src/compute/temporal.rs`
- `vendor/polars-arrow-0.55.2/src/compute/utils.rs`
- `vendor/polars-arrow-0.55.2/src/datatypes/field.rs`
- `vendor/polars-arrow-0.55.2/src/datatypes/mod.rs`
- `vendor/polars-arrow-0.55.2/src/datatypes/physical_type.rs`
- `vendor/polars-arrow-0.55.2/src/datatypes/reshape.rs`
- `vendor/polars-arrow-0.55.2/src/datatypes/schema.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/array.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/bridge.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/generated.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/mmap.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/mod.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/schema.rs`
- `vendor/polars-arrow-0.55.2/src/ffi/stream.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/read/deserialize.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/read/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/read/nested.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/read/schema.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/read/util.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/write/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/write/schema.rs`
- `vendor/polars-arrow-0.55.2/src/io/avro/write/serialize.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/append/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/compression.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/endianness.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/binary.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/binview.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/dictionary.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/fixed_size_binary.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/fixed_size_list.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/list.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/map.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/null.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/primitive.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/struct_.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/union.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/array/utf8.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/common.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/deserialize.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/error.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/file.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/flight.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/read_basic.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/reader.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/schema.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/read/stream.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/common.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/common_sync.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/schema.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/binary.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/binview.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/fixed_size_binary.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/fixed_sized_list.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/list.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/map.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/primitive.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/struct_.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/serialize/union.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/stream.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write/writer.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/array/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/array/primitive.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/footer.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/message.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/ipc/write2/schema.rs`
- `vendor/polars-arrow-0.55.2/src/io/iterator.rs`
- `vendor/polars-arrow-0.55.2/src/io/mod.rs`
- `vendor/polars-arrow-0.55.2/src/io/write_owned.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/default_arrays.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/fixed_size_list.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/list.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/null.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/slice.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/array/utf8.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/bit_util.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/conversion.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/error.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/index.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/is_valid.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/set.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/sort_partition.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/sorted_join/inner.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/sorted_join/left.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/sorted_join/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/string.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/take_agg/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/take_agg/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/take_agg/var.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/kernels/time.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/prelude.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/time_zone.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/trusted_len/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/trusted_len/mod.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/trusted_len/push_unchecked.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/trusted_len/rev.rs`
- `vendor/polars-arrow-0.55.2/src/legacy/utils.rs`
- `vendor/polars-arrow-0.55.2/src/lib.rs`
- `vendor/polars-arrow-0.55.2/src/mmap/array.rs`
- `vendor/polars-arrow-0.55.2/src/mmap/mod.rs`
- `vendor/polars-arrow-0.55.2/src/offset.rs`
- `vendor/polars-arrow-0.55.2/src/pushable.rs`
- `vendor/polars-arrow-0.55.2/src/record_batch.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/binary.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/binview.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/boolean.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/dictionary.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/equal.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/fixed_size_binary.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/fixed_size_list.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/list.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/map.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/mod.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/null.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/primitive.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/struct_.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/union.rs`
- `vendor/polars-arrow-0.55.2/src/scalar/utf8.rs`
- `vendor/polars-arrow-0.55.2/src/temporal_conversions.rs`
- `vendor/polars-arrow-0.55.2/src/trusted_len.rs`
- `vendor/polars-arrow-0.55.2/src/types/aligned_bytes.rs`
- `vendor/polars-arrow-0.55.2/src/types/bit_chunk.rs`
- `vendor/polars-arrow-0.55.2/src/types/index.rs`
- `vendor/polars-arrow-0.55.2/src/types/mod.rs`
- `vendor/polars-arrow-0.55.2/src/types/native.rs`
- `vendor/polars-arrow-0.55.2/src/types/offset.rs`
- `vendor/polars-arrow-0.55.2/src/util/macros.rs`
- `vendor/polars-arrow-0.55.2/src/util/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binary/dictionary.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binary/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binary/plain.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/optional.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/optional_masked.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/predicate.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/required.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/binview/required_masked.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/boolean.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/categorical.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/optional.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/optional_masked_dense.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/predicate.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/required.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/dictionary_encoded/required_masked_dense.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/fixed_size_binary.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/nested_utils.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/null.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/float.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/integer.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/plain/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/plain/predicate.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/primitive/plain/required.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/simple.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/utils/array_chunks.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/utils/filter.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/deserialize/utils/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/expr.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/schema/convert.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/schema/metadata.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/schema/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/read/statistics.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binary/basic.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binary/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binary/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binview/basic.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binview/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/binview/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/boolean/basic.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/boolean/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/boolean/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/dictionary.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/file.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/fixed_size_binary/basic.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/fixed_size_binary/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/fixed_size_binary/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/nested/dremel/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/nested/dremel/tests.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/nested/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/pages.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/primitive/basic.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/primitive/mod.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/primitive/nested.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/row_group.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/schema.rs`
- `vendor/polars-parquet-0.55.2/src/arrow/write/utils.rs`
- `vendor/polars-parquet-0.55.2/src/lib.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/bloom_filter/hash.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/bloom_filter/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/bloom_filter/read.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/bloom_filter/split_block.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/compression.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/bitpacked/decode.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/bitpacked/encode.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/bitpacked/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/bitpacked/pack.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/bitpacked/unpack.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/byte_stream_split/decoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/byte_stream_split/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_bitpacked/decoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_bitpacked/encoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_bitpacked/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_byte_array/decoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_byte_array/encoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_byte_array/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_length_byte_array/decoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_length_byte_array/encoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/delta_length_byte_array/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/hybrid_rle/bitmap.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/hybrid_rle/encoder.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/hybrid_rle/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/hybrid_rle/proptest.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/plain_byte_array.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/uleb128.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/encoding/zigzag_leb128.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/error.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/handwritten_thrift/file_metadata_thrift.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/handwritten_thrift/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/handwritten_thrift/parquet_macros.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/handwritten_thrift/parquet_thrift.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/column_chunk_metadata.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/column_descriptor.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/column_order.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/compact.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/file_metadata.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/file_metadata_serde.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/row_metadata.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/schema_descriptor.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/metadata/sort.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/page/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/parquet_bridge.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/column/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/compression.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/levels.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/metadata.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/page/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/page/reader.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/page/stream.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/read/stream.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/io_message/from_message.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/io_message/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/io_thrift/from_thrift.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/io_thrift/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/io_thrift/to_thrift.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/basic_type.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/converted_type.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/parquet_type.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/physical_type.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/schema/types/spec.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/statistics/binary.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/statistics/boolean.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/statistics/fixed_len_binary.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/statistics/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/statistics/primitive.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/types.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/column_chunk.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/compression.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/dyn_iter.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/file.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/indexes/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/indexes/serialize.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/indexes/write.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/mod.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/page.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/row_group.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/statistics.rs`
- `vendor/polars-parquet-0.55.2/src/parquet/write/stream.rs`
