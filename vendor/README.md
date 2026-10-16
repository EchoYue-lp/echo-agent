# Media dependency source provenance

These are the original published media parents used by `echo_tools`, retained
as build dependencies while their ZIP constraints select only yanked releases.
They are excluded from first-party workspace membership. See
[ADR 0089](../docs/adr/0089-non-yanked-media-dependencies.md).

| Package | Original crate SHA-256 | Upstream commit | License |
| --- | --- | --- | --- |
| [rust_xlsxwriter 0.82.0](https://crates.io/crates/rust_xlsxwriter/0.82.0) | `d61a82de4e7b30fc427909f2c5aafaada88cc7ae8316edabae435f74341f9278` | `f36d8f74d679ec2fe94bcb987146bcc983084070` | MIT OR Apache-2.0 |
| [calamine 0.36.1](https://crates.io/crates/calamine/0.36.1) | `5fa68281b1a76b54a62156474adb06bb380a67e07dd60656e3217152b42183f3` | `0a24c2a9f1e38c0932c1299e633270dc730db505` | MIT |
| [docx-rs 0.4.22](https://crates.io/crates/docx-rs/0.4.22) | `7fdf00e8af6d0b3e92d4bbf9b76f773d8b84ea80f310324ad16cbdc2e653e02c` | `f04cf8b47a925b2b6fff850dcb75069aae89dab4` (`docx-core`) | MIT |

The `.crate` checksums match the official crates.io index. Rust source, required
README/changelog includes and license texts are retained. The docx-rs crate
omits its license file; `docx-rs-0.4.22/LICENSE` comes from the exact upstream
commit above, Git blob `bb2ae316cb1d1b43c6f7c7f2658ade9ede020e21`.
Original upstream fixtures, standalone examples, benches, development dependency
tables and packaging metadata that are unused when consumed as a dependency are
excluded. Inline source tests remain as upstream source, but these packages are
not independently run as workspace members.

Local deltas from those exact archives:

- Three `Cargo.toml` files select `zip = "=9.0.3"`, retain production dependency
  settings and remove unused development targets. Empty workspace declarations
  keep each bundled package independent during formatter metadata traversal.
- rust_xlsxwriter's `zlib` feature maps to `zip/deflate-flate2-zlib`, the ZIP 9
  equivalent of its prior zlib backend.
- Calamine `src/utils.rs` returns `Result<cache, ZipError>` from the private
  ZIP-path cache and propagates filename errors with `?`; `src/xlsx/mod.rs` and
  `src/xlsb/mod.rs` propagate that existing ZIP error. Its inline cache test
  unwraps the new result only after asserting success, using a default fallback.
- rust_xlsxwriter `src/rich_value.rs` and `src/xmlwriter.rs` explicitly spell six
  already-inferred private return lifetimes (`'_`); behavior and public API do
  not change. No docx-rs Rust source is modified.

This is 449 dependency files, with 436 retained archive files byte-for-byte
unchanged and thirteen differences: three manifests, five adapted Rust files, the
restored docx-rs license and four packaging-only EOF normalizations. Extra final
blank lines in calamine examples/README, docx-rs README, rust_xlsxwriter CHANGELOG
and LICENSE_MIT are removed for the existing Git whitespace check; complete
license terms and all production Rust remain intact. This README is additional maintenance documentation.
ZIP 9.0.3's filename iterator returns `Result`, but its current UTF-8/CP437
implementation maps all byte names; no reachable filename-item error fixture is
claimed. Both spreadsheet constructors preserve error propagation rather than
silently skipping an entry.

The generated targeted parent update also rebinds existing versions within
upstream ranges: hyper-util 0.1.20, quinn 0.11.11 and quinn-udp 0.5.15 select
socket2 0.6.5 instead of 0.5.10; tempfile 3.27.0 selects getrandom 0.4.3 instead
of 0.3.4; winapi-util 0.1.11 selects windows-sys 0.61.2 instead of 0.48.0.
These are permitted resolver choices, not all mandatory ZIP changes. The lock
is generated, with no hand-edited dependency edge or unrelated range constraint.

Preserve licenses, original checksums and this adaptation list on updates.
Remove the local source override after compatible official releases close the
non-yanked ZIP graph and pass actual media roundtrips and required build gates.
Git/path consumers inherit these sources. Cargo registry publication strips
local paths and would resolve the original registry parents again; registry
publication is not qualified by this repair. No npm/PyPI/Maven publication,
security exception or source-policy relaxation is introduced.

## Registry advisory coverage

Official RustSec scanners skip path/Git packages, so unchanged scanner settings
alone would lose the three parents' registry advisories. The additional
`registry-origins.toml` records their exact original versions, archive checksums
and bundled manifest locations. The canonical framework generator is shared by
consumers; do not copy a second implementation into SDK or CLI.

Using Python 3.11 or newer as a development audit tool:

```sh
python3 scripts/generate-vendored-media-audit-lock.py --output /tmp/media-origin.lock
cargo audit --deny warnings --file /tmp/media-origin.lock
```

The generator reads the actual framework lock by default. Independent consumers
pass `--lockfile /path/to/their/Cargo.lock` to scan their own graph. It requires
one `echo_tools` owner, one exact version of each declared parent, and the same
original path/Git SourceId for that owner and each parent. Unrelated forks,
missing/duplicate packages, version or provenance mismatches and overwriting the
input are errors. The output is a conservative audit-only projection: only the
three parents' source/checksum fields become their original crates.io identity;
all other packages, versions, fields and dependency edges (including ZIP 9)
remain unchanged. It is not a resolvable registry build graph or a replacement
for Cargo.lock. CI keeps the normal audit/deny checks and additionally runs the
same official `cargo audit --deny warnings` over this view. No vulnerability
matching rules or exceptions are added. The generated view is temporary and is
not committed. Python SDK runtime support is unaffected by this development tool.
