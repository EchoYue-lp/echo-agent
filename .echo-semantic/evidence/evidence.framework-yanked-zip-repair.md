---
schema_version: 1
id: evidence.framework-yanked-zip-repair
kind: evidence
observed_at: source:d05de8aeb8655e4bc78aa98764cb6e04421860a891a734821c24327d5f3559e3
source_refs:
- Cargo.toml
- Cargo.lock
- echo-tools/Cargo.toml
- echo-tools/tests/zip9_media_compat.rs
- vendor/README.md
- vendor/registry-origins.toml
- scripts/generate-vendored-media-audit-lock.py
- scripts/tests/test_vendored_media_audit_lock.py
- .github/workflows/rust-ci.yml
- vendor/calamine-0.36.1/Cargo.toml
- vendor/calamine-0.36.1/src/utils.rs
- vendor/calamine-0.36.1/src/xlsx/mod.rs
- vendor/calamine-0.36.1/src/xlsb/mod.rs
- vendor/rust_xlsxwriter-0.82.0/Cargo.toml
- vendor/rust_xlsxwriter-0.82.0/src/rich_value.rs
- vendor/rust_xlsxwriter-0.82.0/src/xmlwriter.rs
- vendor/docx-rs-0.4.22/Cargo.toml
- docs/adr/0089-non-yanked-media-dependencies.md
- vendor/docx-rs-0.4.22/src/documents/image_collector.rs
- vendor/docx-rs-0.4.22/src/reader/run.rs
- vendor/docx-rs-0.4.22/src/xml/writer.rs
- vendor/rust_xlsxwriter-0.82.0/src/serializer.rs
supports:
- behavior.workspace-composition
- rule.framework-layer-ownership
limitations:
- Candidate media compatibility and independent metadata resolution are not full workspace gates, SDK consumer compilation or exact merged-main CI.
- Standard Git HTTPS fetch failed; local audit/deny used caches independently verified before and after against the live official GitHub API HEAD and tree, without stale/no-yanked/new-ignore flags. Exact main CI must still run its original online commands.
- Current ZIP9 name decoding is total over UTF8/CP437 bytes; the fallible iterator error propagation is source-checked, not claimed executed by a fabricated input.
- Registry publication is not qualified; normal path stripping would select the original registry parent constraints.
---

# ZIP撤回依赖修复的有限证据

## 支持的结论

官方index无可用2/8兼容版本，三个父包published/main仍旧ZIP范围。保持三父包版本与原生产实现，仅适配3manifest、calamine3私有结果传播文件、xlsxwriter2私有生命周期文件及补回原VCS docx许可证。449依赖文件共5585085B，其中436原字节不变；精来源/十三差异/五额外现有version重绑及退出条件由vendor/README与ADR0089维护。

## 来源与范围

ignored yanked-zip-dependencies目录保存原875 CI、本地audit红、官方index、校验archive、真实candidate source/lock和1758输入封存。candidate compat1是API编译101/0case；compat2两case真0但六lifetime warnings；最薄修复后compat3与final包装compat4各两case0/0warn，完整1205B及750B，输入changed[]。实际ExcelWriteTool→ExcelReadTool保Unicode/42.5/bool，docx pack→WordReadTool保Unicode。最终三parent与正式vendor逐字一致，正式测试仅改用每case独立TempDir和media cfg。

Candidate独立consumer metadata2为单member，framework为9members，均只zip9.0.3且锁不变；不宣称真实SDK构建。最终candidate formatter0/0B，empty workspace明确第三方包不属于firstparty工作空间。原archive reviewer对候选source/兼容/必要性最终C0/I0/M0，未扩为audit/fullgate/remote或新正式快照的最终审查。

## 已知缺口

正式focused1实际9107B/exit0，两case通过/0fail/0ignore/0编译warning，1760非sem源码before/after changed[]。新测试使用独立TempDir，未依赖tasklocal环境。formal-audit1实际143B/exit0（--deny warnings --no-fetch），formal-deny1实际721168B/exit0（workspace/all-features/locked/offline），advisories/bans/licenses/sources均ok，保49条duplicate policy warnings/其它warning0；不是SDK duplicate特例或零工具warning。两cache在官方API前后精等HEAD edb58f5974b83eb8302332821cb6d389777aa351/tree9cadd9fc13d75527fa5fd3f14376ed3a1a5e3b63且Gitstatus空，1760源输入不变。Root用相同精库对原875锁实际exit1仍拒两yank，未隐藏类别。

新锁完整六门禁与17条件feature、最终必要性/语义/独审及精mainCI仍待，Findingopen。RustSec网络失败单独保留，不改audit/deny配置与正常mainCI命令。官网/examples不需新行为说明（公共媒体API不变），原examples仍纳全all-target门禁；registry发布排除。




Registry-origin coverage: independent source review confirmed cargo-audit0.22.2/rustsec0.33.0 Database filtering and cargo-deny0.20.2 skip path/Git sources. A canonical framework generator plus exact original provenance now supplements the unchanged build lock and original audit/deny commands. Only three parent source/checksum fields are projected; actual owner SourceId, versions and every other field/edge are checked. CI uses explicit Python3.11 development tooling, not a Python SDK runtime change.

Eight persistent guard tests passed for path/Git graph preservation, wrong fork/missing source, missing/duplicate parent, version mismatch, invalid checksum/duplicate provenance, missing/duplicate owner and input/hardlink overwrite rejection. The actual679-package origin view passed official audit (318B/0),1763 source inputs unchanged and current official API/cache HEAD/tree matched before/after. A real local Git consumer resolved one member, all three parent Git SourceIds and registry zip9. Its own actual lock passed the same canonical generator and official audit (219B/0); initial three Git-fixture setup failures are retained, not counted as behavior tests or builds.

Official scanner matching controls used only the real historical4eb quinn-proto0.11.16 checksum/version: path/Git controls exited0 and registry-origin views each exited1 with1774B/1773B and five real RustSec advisories, including RUSTSEC-2026-0342/0339/0338/0340. This does not claim any of the current three media parents has a CVE. The same view function on the real current media graph exited0/322B. No CVE matching rules, stale flags or exceptions were introduced. Final source/metadata review, full6+17 and exact merged-main CI remain pending.


Independent final bounded source/necessity review passed C0/I0/M0 at freeze4; see audit.framework-yanked-zip-source-review. Full gates/mainCI remain pending.


Final import whitespace check rejected four upstream extra EOF blank lines. Only those terminal newlines were removed from the two README files, CHANGELOG and LICENSE_MIT; license terms and every production Rust byte are unchanged from the reviewed source. The final vendor import has13 differences and436 byte-identical original files; the earlier candidate9/440 statement remains historical. This is packaging normalization, not an audit waiver or a new media behavior.
