# ADR 0100：由 Framework 维护 Polars 的 zstd 安全依赖补丁

## 状态

2026-10-27 用户明确采用本方案，继续正式设计与修复。本文记录已采用的架构方向；生产补丁、正式消费者 pin、完整依赖门禁与交付验收仍须分别取得证据，不能由 ADR 状态推定完成。

## 背景与参考

Framework 的 `data` / `statistics` 与 SDK 的完整 facade 消费 Polars。已发布的 `polars-arrow`、`polars-parquet` 0.55.2 约束 zstd 0.13；只更新锁不能选择 zstd 0.14.3。原链为 `zstd 0.13.3 → zstd-safe 7.2.4 → zstd-sys 2.0.16+zstd.1.5.7`。SDK 原锁的官方扫描在既有政策下仍报告四项漏洞和十项 unsound 告警，不能通过减少 data feature 或增加 ignore 解决。

本方案复用 [ADR 0089](0089-non-yanked-media-dependencies.md) 的发布包归属与唯一审计投影，保留其三个媒体依赖的相对 path 行为。Polars 属于另一种 Cargo 选择边界：[Cargo 官方覆盖规则](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html)规定 `[patch]` 仅由消费 workspace 根生效，依赖仓库中的 patch 不会自动继承；[Git 依赖规则](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#specifying-dependencies-from-git-repositories)允许同一精确 revision 中的包被独立发现。因此 Framework 根使用两条 path patch，独立 SDK 根必须显式使用两条同 revision 的 Git patch。

隔离实验已证明两根完整依赖图可解析、真实 Arrow IPC 与 Parquet 压解三项通过，并在同一既有 RustSec 数据库上令十四项 zstd 告警消失。这不是正式来源、完整 Host、三语言、fresh 数据库或完整 yanked 资格；索引查询失败的原件与限制保留。

## 决策

### 唯一源码所有者与最小差异

Framework 在 `vendor/` 唯一维护两个官方发布包，版本及名称保持 0.55.2，仅把各自 normalized `Cargo.toml` 的 `dependencies.zstd.version` 从 0.13 改为 0.14.3，保留 optional、features 与其余声明。两包共 445 个发布成员，除这两个 manifest 外保持原字节，包括 Rust 源码、`Cargo.toml.orig`、VCS 归属和许可证；不复制反向父依赖闭包，不改名、不建立版本 shim。

| 发布包                | 原 `.crate` SHA-256                                                | 原上游 commit                              | 许可证             |
| --------------------- | ------------------------------------------------------------------ | ------------------------------------------ | ------------------ |
| polars-arrow 0.55.2   | `c5ab55460ca2553d1a2bbaf2046998cfbbb685bc18d1e168b4367b63301eb271` | `d7488c71ecfbc77790292ff5b365b991c08380ce` | MIT AND Apache-2.0 |
| polars-parquet 0.55.2 | `d33a17979d2afa0309437478662cf996361a6e1768bf467db82bd4f4a9443064` | 同上                                       | MIT AND Apache-2.0 |

原 archive checksum 证明发布来源，不证明修改后的整包仍与原 archive 相同。维护文档必须同时保存原包身份、逐成员校验及两处真实补丁。两包排除于第一方 workspace 成员；不为格式工具遍历而改写其它发布成员。

### 五个来源与同一审计权威

扩展现有 `vendor/registry-origins.toml`，保留 schema 1 和每项 `name`、`version`、`checksum`、`manifest`，准确包含原三个媒体父包及新增两个 Polars 父包。继续由 `scripts/generate-vendored-media-audit-lock.py` 拥有唯一算法，不在 SDK 或应用复制。

五项声明必须始终完整校验，但应投影集合取决于真实消费锁。原三个媒体父包继续必需；对于每个新增 Polars 父包，只要锁中存在该包记录，或任一实际 dependency 引用它，就必须纳入并核对恰一条精确版本及同源记录。只有该 Polars 包记录与全部依赖引用均不存在、确实未进入本消费图时才不投影；不得把被依赖引用的缺包静默跳过，也不加 CLI 专用 flag 或第二算法。该规则逐项判断，允许未来合法的 Arrow-only 消费图，不以三项或五项计数代替真实图检查。

当前 Framework/SDK 完整数据锁中 Arrow 和 Parquet 分别有 15 / 4 条父引用，正式修复后的这两个场景必须投影五项；当前 CLI 856 包锁完全没有两包及其引用，仍投影原媒体三项。完整五项声明与实际三项投影并不冲突，不能因扩大来源声明而阻断无 data 的 GUI 消费者。

生成器继续核对唯一身份、精确版本、原 checksum、vendor manifest 归属，以及应投影集合中每个 path/Git 父包与唯一 `echo_tools` 的同源关系；拒绝无关 fork、混合 revision、重复或应有却缺失的包、错误版本、非法 manifest 路径及覆盖输入。审计视图仅把应投影包的 source/checksum 转为原 crates.io 身份，保留真实锁由修订 manifest 解析出的全部依赖边和其它字段。旧三源生成器即使返回 0，也不证明数据图中的新增两包已被扫描。视图不用于构建、不提交为 Cargo.lock，也不替代正常 audit/deny。

### SDK 与公共边界

SDK 不复制 vendor，也不新增协议类型。取得 Framework 的精确、可恢复修订后，Host pin、SDK 根两条 patch、来源断言及独立锁/基线绑定必须共同指向同一修订，保持真实 canonical Git URL。正式合同生成与消费者验收不能仅靠改字符串断言通过。

`polars_parquet::parquet::compression::DecompressionContext::Zstd` 的 payload 仍是 `zstd_safe::DCtx`，但 safe7 → safe8 改变其 Rust crate 类型身份；显式持有旧 safe7 类型的第三方调用方不获原赋值兼容承诺。沿用原 `Unset`、`BasicDecompressor` 和已有私有 context 缓存，不增加 wrapper 或第二缓存。Framework 的 `load_dataframe -> DataFrame`、`is_numeric(&DataType)` 以及实际工具仍须编译和运行验证，不能称整个 Rust public API 等价。

SDK 当前保留的 5,352 项 accepted 身份、431 条路由及 18 个 data 操作由原合同控制。完整 Rust inventory 继续是遥测，不因这个补丁扩大跨语言接受范围；只有正式生成器确认的真实差异才能更新产物。接受集、source digest 与 pin 各有既有权威，不另建来源表。

### 修订恢复与交付资格

在当前禁止 push/merge 的授权下，可以先形成经门禁的 Framework 本地提交，保存 Git 对象、精确 tree 和可回读恢复材料，作为本地消费者候选。若验证采用任务级 Git 传输或自有 Cargo cache，必须保留实际对象来源、命令和 SHA，限本进程、不改全局；canonical URL/revision 与取得对象的传输来源分别记实。临时 `file://`、path 编译或探针提交 `7448e551...` 不能充当正式 SDK 来源。

本地对象可恢复不表示 GitHub 已发布、无本地补充的冷拉取可用或精确 main/CI 通过。若现有来源门禁无法接受受控本地候选，保留阻断，不删除断言或捏造远端回执；远端发布与冷恢复仍待相应授权。

## 候选方案与取舍

| 候选                                          | 取舍                                                                             |
| --------------------------------------------- | -------------------------------------------------------------------------------- |
| 等待上游发布兼容新 zstd 的 Polars             | 后续可移除本地补丁，但当前不能完成已知安全闭包修复                               |
| 只升级 zstd 锁记录                            | 父包版本约束不满足，无法形成真实解析图                                           |
| 删除 data/statistics、增加 ignore 或版本 shim | 缩小能力或隐藏原始身份，不符合已批准范围                                         |
| SDK 复制 vendor 或完整反向依赖闭包            | 增加源码权威和维护漂移；两个根显式选择同一 Framework 包已足够                    |
| Framework 两份最小补丁、SDK 根显式同源选择    | 已验证可行，源码所有者唯一、差异可审；接受新增 vendor 维护与 root patch 配置成本 |

## 影响、验收、失败与维护

正式实现须证明完整五项声明、按真实消费图投影及错误拒绝：Framework/SDK 完整数据图必须五项；真实 CLI 锁保持三项成功；完整图删除仍被引用的 Arrow/Parquet、重复/错版本/错来源均失败。随后分别验证真实锁图、完整标准依赖检查、原精确 duplicate 身份合同、实际 IPC/Parquet/context 恢复、Framework data/statistics 与示例、同 pin SDK 合同/Host/三语言消费者。probe 的 3/3 只能保留为可行性证据；五个 parent 的 checksum/yanked 补证不能替代整个锁图的检查。任何新风险、缺失组、来源或接受合同漂移均继续阻断，不能自动扩大风险接受。

失败时保留原日志、源码/锁/pin 的配对切点和必要制品，再在无读取者后清理准确自有 target；恢复只针对本任务文件，不覆盖其它工作或 SDK 既有候选。不能只回退 manifest 而留下新锁、来源断言或已宣告通过的验收。无 Cargo 产物的文档阶段不做形式清理。

本次没有新增用户功能或工具 schema；实现阶段仍须检查并同步所属依赖说明、原 data/media 示例与官网来源说明，不能宣称独立 root 自动继承 patch 或 registry 发布已可用。上游正式版本能形成安全闭包后，移除本地覆盖仍须重新验证来源、消费者及标准依赖门禁。
