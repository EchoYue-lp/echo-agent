# 第三方依赖源码与审计归属

Framework 唯一维护下列官方发布包的有界补丁。第三方包排除于第一方 workspace 成员；名称和发布版本不变，原 archive checksum 用于归属，修改后源码树由实际 Git revision、成员清单和补丁证明，不能冒充仍等于原发布归档。

| 包                                                                        | 原 `.crate` SHA-256                                                | 原上游 commit                                                       | 许可证             |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------- | ------------------ |
| [rust_xlsxwriter 0.82.0](https://crates.io/crates/rust_xlsxwriter/0.82.0) | `d61a82de4e7b30fc427909f2c5aafaada88cc7ae8316edabae435f74341f9278` | `f36d8f74d679ec2fe94bcb987146bcc983084070`                          | MIT OR Apache-2.0  |
| [calamine 0.36.1](https://crates.io/crates/calamine/0.36.1)               | `5fa68281b1a76b54a62156474adb06bb380a67e07dd60656e3217152b42183f3` | `0a24c2a9f1e38c0932c1299e633270dc730db505`                          | MIT                |
| [docx-rs 0.4.22](https://crates.io/crates/docx-rs/0.4.22)                 | `7fdf00e8af6d0b3e92d4bbf9b76f773d8b84ea80f310324ad16cbdc2e653e02c` | `f04cf8b47a925b2b6fff850dcb75069aae89dab4`（docx-core）             | MIT                |
| [polars-arrow 0.55.2](https://crates.io/crates/polars-arrow/0.55.2)       | `c5ab55460ca2553d1a2bbaf2046998cfbbb685bc18d1e168b4367b63301eb271` | `d7488c71ecfbc77790292ff5b365b991c08380ce`（crates/polars-arrow）   | MIT AND Apache-2.0 |
| [polars-parquet 0.55.2](https://crates.io/crates/polars-parquet/0.55.2)   | `d33a17979d2afa0309437478662cf996361a6e1768bf467db82bd4f4a9443064` | `d7488c71ecfbc77790292ff5b365b991c08380ce`（crates/polars-parquet） | MIT AND Apache-2.0 |

## 原媒体补丁

[ADR 0089](../docs/adr/0089-non-yanked-media-dependencies.md) 记录三个媒体父包对已撤回 ZIP 版本的修复。保留生产源码、include 所需 README/changelog 和许可；docx-rs 发布包缺失的 LICENSE 取自上表精确上游 commit、Git blob `bb2ae316cb1d1b43c6f7c7f2658ade9ede020e21`。未用于依赖消费的 fixtures、独立 examples、benches、dev-dependency 表与包装元数据未纳入；源码内联测试保留，第三方包不作为独立 workspace 成员运行。

三个媒体包的原有本地差异：

- 三个 manifest 选择 `zip = "=9.0.3"`，保留生产设置，删除未用开发 target；空 workspace 声明保持格式工具遍历时独立。
- `rust_xlsxwriter` 的 zlib feature 映射到 ZIP 9 的 `deflate-flate2-zlib`；六个既有私有返回引用显式写出 `'_`，消除此前 registry 依赖隐藏的编译警告。
- Calamine 私有文件名缓存返回 `Result<_, ZipError>`；XLSX/XLSB 原错误路径传播迭代器错误，不跳过条目。当前 ZIP 的 UTF-8/CP437 实现映射所有字节名，不宣称已有可触发文件名条目错误的夹具。
- docx-rs 生产 Rust 未改。

原媒体补丁共 449 个依赖文件：436 个原归档成员逐字节保持，十三处差异为三个 manifest、五个适配 Rust 文件、补回的一份许可，以及四处包装文件 EOF 空行归一；完整许可条文与生产内容保留。既有原生 XLSX/Word 工具回归验证实际写读。

此前 ZIP 定向更新由 Cargo 在上游允许范围内重绑过同版本边：hyper-util 0.1.20、quinn 0.11.11、quinn-udp 0.5.15 选择 socket2 0.6.5；tempfile 3.27.0 选择 getrandom 0.4.3；winapi-util 0.1.11 选择 windows-sys 0.61.2。这些并非都由 ZIP 强制要求，历史选择不能替代当前真实锁图。

## Polars 的 zstd 补丁

[ADR 0100](../docs/adr/0100-zstd-safe-chain-vendoring.md) 记录当前选择。两个官方包共 445 个发布成员，仅各自 normalized `Cargo.toml` 的 `dependencies.zstd.version` 从 `0.13` 改为 `0.14.3`；optional/features、所有 Rust、`Cargo.toml.orig`、VCS 和许可原件保持原字节与文件模式。根 workspace 显式排除两包，以两条 path patch 统一选择它们，不复制反向父依赖闭包或重命名发布身份。

实际链为 `zstd 0.14.3 → zstd-safe 8.2.0 → zstd-sys 2.1.2+zstd.1.5.7`。本次从当前精确锁定向更新 zstd 的自然 Cargo 结果另将 tempfile 3.27.0 的 getrandom 边从 0.4.3 改为 0.3.4；其上游约束为 `>=0.3.0, <0.5`、optional、default-features=false，两版本原有节点/checksum不变。这是已审的允许范围解析选择，并非 zstd 修复强制要求或新的风险接受；未手改锁边或给父包另加约束。原生成差异与相关运行回执分别保存。

公开嵌套 `DecompressionContext::Zstd` 的 `DCtx` 从 zstd-safe 7 改为 8，crate 类型身份不同；不承诺旧显式类型赋值兼容。原 `Unset`、`BasicDecompressor` 与私有 context 缓存继续使用；Framework 的 DataFrame/DataType、真实数据工具及 SDK 保留消费者仍须按实际源码验证，不以相同签名文本推定全部行为等价。

### 独立消费 root

Cargo 的 `[patch.crates-io]` 仅由消费 workspace 根生效，依赖中的 patch 不自动传播。Framework 自身用相对 path patch；SDK 或其它 Git 消费 root 启用数据能力时，必须显式把这两个包选择到与 Framework 相同的 canonical Git URL 和精确 revision，例如：

```toml
[patch.crates-io]
polars-arrow = { git = "https://github.com/EchoYue-lp/echo-agent.git", rev = "<与 Framework 依赖相同的精确修订>" }
polars-parquet = { git = "https://github.com/EchoYue-lp/echo-agent.git", rev = "<与 Framework 依赖相同的精确修订>" }
```

此处是消费配置形状，实际修订必须可恢复并经相应来源门禁；本地候选、临时 file URL 或本机 cache 不证明 GitHub 已发布或冷恢复通过。原三个媒体父包通过 Framework 相对 path 依赖传递，不能把这条结论扩张为五包都自动继承。SDK 不保存第二份 vendor，接受合同、Host 与三语言验收由 SDK 独立负责。

## 唯一 registry-origin 审计投影

官方 scanner 会跳过 path/Git 包，因而 `registry-origins.toml` 完整登记五项原 name/version/checksum/manifest。唯一生成器仍是 Framework 的 `scripts/generate-vendored-media-audit-lock.py`；消费方传入其真实锁，不复制第二算法。Python 3.11+ 只是开发审计工具要求，不改变 Python SDK 运行要求。

声明始终准确为五项；原媒体三项继续必需。每个 Polars 包只要有实际 package 记录或 dependency 引用，就必须有恰一条精确同源记录并投影；只有两者均无才视为未进入图。当前完整数据图是五项，未启用 Polars 的 CLI 实际图为三项，合法 Arrow-only 图仍沿同一规则。被引用缺包、重复、错版、与唯一 echo_tools 不同源、非法归属或输入/硬链接覆盖均失败，不按数量相同放行。

在预先准备的本仓临时目录输出审计视图：

```sh
mkdir -p .echo-semantic/local/dependency-audit
python3 scripts/generate-vendored-media-audit-lock.py --output .echo-semantic/local/dependency-audit/registry-origin.lock
cargo audit --deny warnings --file .echo-semantic/local/dependency-audit/registry-origin.lock
```

独立消费者同时传 `--lockfile <其真实Cargo.lock>`。输出只把应投影包的 source/checksum 改为原 registry 身份，保持真实解析依赖边、版本、其它包和字段；不补不存在的包，不回写真实锁，不作为可解析的 registry 构建图或提交到 Git。正常 audit/deny/yanked/license/source 与原精确 duplicate 合同仍需各自运行；不新增 ignore、scanner 或风险豁免。索引错误、局部 advisory-only 和历史 probe 通过不能代表完整依赖资格。

## 更新与退出

更新时保存原 archive/许可/精确差异和来源，针对真实消费图重验。上游兼容版本能提供安全、非 yanked 的闭包后可移除对应覆盖，但须重新验证原工具、依赖门禁、独立消费者和适用 workspace/feature/example。

Cargo registry 包装会剥离本地 path，不能据此宣称 registry 发布已具备资格。本轮不包含 crate/npm/PyPI/Maven 发布、官网部署或任何新的审计例外。完整门禁、实际消费者、精确 main/CI 和远端冷恢复分别验收，不由文档或 pin 改动自动继承。
