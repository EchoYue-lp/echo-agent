# Dependency policy repair for PR #181

The dependency-policy job in [run 38417628760](https://github.com/EchoYue-lp/echo-agent/actions/runs/38417628760/job/115366181706) failed on the reviewed framework candidate `4eb005c15b9c0036b73e409026c668c02f0a9575`. Its locked `quinn-proto 0.11.16` triggered five RustSec vulnerabilities, and `serde_yaml_ng 0.10.0` pulled the unmaintained `unsafe-libyaml 0.2.11`. The repair upgrades the affected packages and keeps the existing audit/deny policies intact.

## Dependency choice and scope

| Previous dependency | Repair | Reason |
| --- | --- | --- |
| `quinn-proto 0.11.16` | Lock to `0.11.18` | Meets the patched ranges of RUSTSEC-2026-0338/0339/0340/0341/0342. Version 0.11.18 also avoids the outgoing-datagram accounting regression specific to 0.11.17, described in [RUSTSEC-2026-0343](https://rustsec.org/advisories/RUSTSEC-2026-0343). |
| `serde_yaml_ng 0.10.0` → `unsafe-libyaml 0.2.11` | `yaml_serde 0.10.7` → `libyaml-rs 0.3.0` | YAML Organization's maintained fork preserves the Serde YAML API and documents Cargo package renaming as its migration path. The archived/unmaintained package leaves the production lock. |

The five manifests (root, core, execution, integration and learning) retain the code name through `serde_yaml_ng = { package = "yaml_serde", version = "0.10.7" }`. Existing imports and `Value`, `Mapping`, tagged values, parsing, serialization and errors continue through the same source-level paths; production Rust sources do not change. This is a framework serialization implementation dependency, not an EKO policy or another state authority.

The implementation was selected from primary published sources. [`serde_yaml_ng`](https://crates.io/crates/serde_yaml_ng) still has 0.10.0 as its latest published release. [`serde_yaml2`](https://github.com/zim32/serde_yaml2) uses yaml-rust2 but exports only its serializer/deserializer and wrapper modules; it does not supply the existing `Value`/`Mapping` API and therefore is not a direct package-alias replacement. [`yaml_serde`](https://github.com/yaml/yaml-serde) explicitly documents compatibility and package renaming. Its [`libyaml-rs`](https://github.com/yaml/libyaml-rs) backend is a maintained fork of the C-to-unsafe-Rust translation; this repair does **not** claim to replace it with a wholly safe-Rust parser.

The lock delta is deliberately bounded: remove `quinn-proto 0.11.16`, `serde_yaml_ng 0.10.0` and `unsafe-libyaml 0.2.11`; add `quinn-proto 0.11.18`, `yaml_serde 0.10.7` and `libyaml-rs 0.3.0`; update the five workspace YAML dependency edges. All other registry-package entries are byte-identical to the original lock. The initial Cargo resolver also rewrote five unrelated package edges; these were restored, and the minimal lock subsequently passed full `--locked` metadata, graph and test resolution.

No RustSec ignore, audit warning threshold, license exception, source policy or CI command changes. The four previously documented audit/deny exceptions remain owned by maintainers with their existing 2026-11-16 review date. Duplicate versions remain `warn` according to the existing deny configuration; the full deny log retains fifty duplicate-version graph diagnostics and exits successfully. These are dependency policy diagnostics, not Rust compiler warnings.

## Verification and evidence

The [evidence receipt](evidence/dependency-repair-20261013/receipt.json) binds the six source hashes, complete before/after files, exact lock delta, primary package sources, native test logs and dependency-policy outputs.

Git HTTP fetching of the advisory database failed locally. The database was instead recovered using GitHub's official Git-tree and authenticated blob APIs at exact commit `29d1a60c3361576d366b1740bba2254dc6173434` (2026-10-13 13:53:07 UTC). All 1329 file Git hashes match that commit, including both crate and Rust standard-library advisories; the scanner loads all 1310 advisories, matching the CI database size. No synthetic database commit was created and no stale-database option was used. The original lock reproduces five vulnerabilities and one denied maintenance warning against this same complete database. The repaired lock passes:

```sh
cargo audit --deny warnings --no-fetch --db <verified-advisory-source>
cargo deny --offline --locked --all-features --workspace \
  --config <original-policy-with-only-db-path-pointing-at-verified-cache> check
```

The temporary deny configuration changes only the location of its advisory cache; repository `deny.toml` and `.cargo/audit.toml` are unchanged. `--no-fetch`/`--offline` avoid repeating the failing HTTP Git transport after exact source acquisition; they do not filter advisories, suppress warnings or allow stale data. Audit exits zero, and all four deny checks (`advisories`, `bans`, `licenses`, `sources`) report `ok`. The normal unchanged CI commands still need a fresh remote run on the delivered revision.

A separate old/new API comparison runs seven `Value` documents (UTF-8 mapping, sequence/scalars, anchors/aliases, merge-key representation, omitted versus explicit null, explicit scalar tags, composite keys), four typed/tagged enum variants, and four malformed-input cases. Serialization bytes and roundtrips match; malformed inputs preserve error text and locations. Its source, lock and raw output are archived. This comparison is evidence tooling, not another production YAML implementation.

Existing native tests run with `--locked --all-features`, jobs 2, debug information disabled and incremental compilation disabled, using the existing shared target cache:

| Focused suite | Passed | Failed / ignored |
| --- | ---: | ---: |
| execution `skills::` (skill/frontmatter parsing, validation, writing and hook definitions) | 197 | 0 / 0 |
| integration `lsp::config` (YAML language-server configuration) | 3 | 0 / 0 |
| root `workflow::loader` (YAML workflow loading) | 6 | 0 / 0 |
| root `evolution::` (memory/draft frontmatter, generation and persistence) | 230 | 0 / 0 |
| learning `semantic_baseline_contract` (typed YAML baseline and history contract) | 3 | 0 / 0 |
| root `plugin::prepared` (plugin components and YAML skill loading) | 20 | 0 / 0 |

These six disjoint filters total 459 passing tests. `cargo fmt --all -- --check`, all-feature locked metadata and dependency-tree checks also pass. The maintained YAML backend is the only selected YAML backend across all five declared consumers. `cargo tree --all-features --target all -i quinn-proto` selects no consumers in the current feature graph, but the package remains in the lock and is still covered by audit; lack of a selected macOS path does not exempt a locked vulnerable package from repair.

This is focused repair evidence, not renewed full-workspace/main acceptance. The dependency change invalidates the previous exact-snapshot gates; the coordinator must complete fresh framework gates, remote dependency CI, independent review and delivery reconciliation. SDK final pin/build/three-language evidence must use the resulting delivered framework revision, rather than borrowing the earlier Host binary.

No accepted SDK operation, wire field, lifecycle, cancellation, recovery or task authority changes. This dependency-only replacement does not change example or website behavior; existing examples keep their source APIs and belong to the final full gate. Learning prose can continue to refer to the preserved Cargo alias. An ADR for a new runtime architecture is not applicable because the implementation boundary, product semantics and API shape remain unchanged.


## 2026-10-23 Tokio I/O-safety successor

A later fresh official database update introduced [RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html) for Tokio's safe `AsyncFd` constructors. The warning is not accepted by the duplicate-version decision. The official patched range includes Tokio 1.53.3; its unyanked registry checksum is `ce3335fa71841cda333a58d7615b03901380ecf09d59b3296d21f8bbac0dde4e`. The targeted patch changes only Tokio 1.53.1 to 1.53.3: all other 678 package blocks, dependency edges, manifests and features remain unchanged. Full locked metadata accepts the graph. Original unrelated resolver edge rewrites are excluded.

Framework lock SHA changes from `7607c1458036adc04e64858215e21d45f7f8ae1ffa992ad82fba97e7a72aa528` to `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c`. Independent finite review confirms the official patch/checksum and unchanged accepted package identities. Both new actual-lock and canonical registry-origin audits fetch the official database (`84d943a6deeb8cc6c6f632531aaf4f1c62275239`, 1,351 advisories), report zero vulnerabilities and zero warnings, and exit0. Fresh cargo-deny0.20.2 naturally exits0 with all four checks and exactly the original49 accepted duplicate groups. The exact baseline and AGENTS lock identity are rebound after this review; no new diagnostic is accepted and no policy or scanner is weakened. Earlier HTTPS transport failures are retained separately from these successes.

The complete patched-lock runtime gates and precise successor main CI remain separate execution evidence. Tokio's patch includes runtime, synchronization and timer fixes, so old-lock tests are not renamed as new-lock qualification. Product API, architecture, example calls and website behavior remain unchanged; examples are covered by the full workspace gate, and the website source manifest binds the eventual real merged framework revision.
