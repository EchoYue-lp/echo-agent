# ADR 0089: Keep media builds on a non-yanked ZIP dependency

## Status

Accepted on 2026-10-16 for the dependency-policy failure at framework main
`875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c` (CI run 38849170013).

## Context and alternatives

The existing XLSX writer, XLSX/XLSB reader and DOCX implementation depend on
zip 2.4.2 or 8.6.0. The official crates.io index marks both releases and all
compatible releases as yanked; zip 9.0.3 is the available non-yanked release.
The current published and upstream-main media parents still select older ZIP
majors. A lock-only update cannot satisfy their constraints. A root-only Cargo
patch would not propagate to independent Git consumers of this framework.
Ignoring yanks, changing audit policy or adding a version shim would conceal the
failure rather than repair the dependency closure.

## Decision

Keep the exact published parent versions as checksum-attributed source under
`vendor/`, use relative workspace path dependencies inherited by `echo_tools`,
and select zip 9.0.3. Exclude these third-party packages from the framework
workspace and make their manifests standalone for Cargo's formatter traversal.
Preserve their production source/includes and licenses; remove dependency-unused
upstream development targets. The complete provenance and local adaptation list
is in [vendor/README.md](../../vendor/README.md).

Calamine's private path-cache helper propagates zip 9's fallible filename iterator
through its existing ZIP error conversions. Six private rust_xlsxwriter return
lifetimes are made explicit to remove warnings previously hidden for registry
dependencies. Its optional zlib backend uses zip 9's corresponding feature name.
First-party media tool APIs, results and feature meanings remain unchanged.

## Consequences and maintenance

This source override belongs to the framework build boundary, so Git consumers
resolve the same media implementation without a second root patch. No runtime
state authority, accepted SDK operation, source allowlist or audit exception is
added. Official scanners skip path/Git packages; an additional temporary
registry-origin view restores the three published parents’ conservative
name/version advisory coverage with the same official scanner. It binds each
parent’s original SourceId to the actual `echo_tools` owner, refuses mismatches,
and projects only source/checksum. The actual lock and normal audit/deny gates
remain intact. Python 3.11+ is development audit tooling, not a framework or
Python SDK runtime dependency. A targeted Cargo update also rebinds five existing-version dependency
edges permitted by upstream ranges; they are recorded in the provenance note
and are not all required by ZIP itself.

Maintain the archive provenance and small local delta when updating these
sources. Remove the override once official releases provide a non-yanked closure
and pass the media, dependency and workspace checks. Cargo registry packaging
removes local paths and would fall back to the original parent releases, so this
change does not make registry publication ready or change release scope.

## Verification and documentation scope

Persistent integration tests exercise actual XLSX write/read and DOCX pack/read
with Unicode and numeric content. Independent consumer resolution and full local
workspace/feature gates plus exact merged-main CI remain separate requirements.
Public examples, SDK contracts and website instructions do not change because
media entry points and behavior do not change; existing examples remain in the
workspace all-target gate. No raw diagnostic archives belong in Git.

## 2026-11-05：Calamine官方安全版本的维护后继

用户已明确批准采用calamine0.38.0并生成新锁候选逐项复评，未预先接受新风险。
本轮延续上面的Framework唯一vendor owner、相对path消费、Zip9.0.3和唯一registry-origin
算法，更新calamine发布版本／路径／真实archive checksum，不新增第一方工具API或工具schema。
依赖本身新增了Reader的样式默认方法及属性／样式类型；本轮不宣称整个第三方Rust API字节等同。
0.38自带旧私有文件名缓存的Zip9错误传播，以及PR762 CFB修复；本地只保留饱和计数、
精确Zip约束、去未用dev targets与独立workspace适配。

新quick-xml0.42和真实resolver边变化按精确锁复评，原duplicate版本集合和CLI已接受
风险不自动扩张。Framework／CLI新锁已经Cargo生成；SDK精pin未切、App未重建。
旧0.36局部回补的2P及旧风险失败仍按原纪元保留。详细来源、验证与待决范围见
[vendor说明](../../vendor/README.md)的本日后继，最终门禁与发行资格另验。
