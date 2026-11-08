---
schema_version: 1
id: audit.framework-yanked-zip-source-review
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 7110a27e7b0e9841dcfa0d4cca1cdbc053d96177
finding_refs:
- finding.framework-yanked-zip-ci
challenges:
  original-media-source-and-registry-advisory-coverage:
    revision: 7110a27e7b0e9841dcfa0d4cca1cdbc053d96177
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
    - docs/adr/0089-non-yanked-media-dependencies.md
    evidence_refs:
    - evidence.framework-yanked-zip-repair
  post-merge-source-adoption-and-exact-main-ci:
    revision: 51726454985fd6f064b21a7e9c539e22d494c987
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
    - docs/adr/0089-non-yanked-media-dependencies.md
    evidence_refs:
    - evidence.framework-yanked-zip-repair
---

# ZIP source-adaptation independent review

## 审查范围

The independent archive reviewer checked the official index/archive checksums,
licenses, complete retained vendor identity and nine precise upstream compatibility/provenance deltas;
then the formal persistent media test, maintenance documents, source-origin
scanner closure and bounded semantic history pins. Final freeze4 has466 paths
and SHA4a3083a47faaa655f1e8b052992dca1d45647a295022e93849a3a72256b8522b.
This later Audit records that review; it was not part of the reviewed466 bytes.

## 已检查故障假设

A root-only patch might miss independent consumers; malformed private filename
adaptation might skip errors; path source might expose hidden warnings; imported
code might omit license/include requirements; generated solver edges might be
misstated; and official scanners might silently omit registry-origin advisories.
Wrong SourceId, missing/duplicate/version/checksum and output/input alias guards
were checked together with complete non-target lock-field preservation.

## 实际实现路径与证据

Formal source/compatibility/necessity review returned C0/I0/M0. The scanner
coverage Important was first confirmed at both official entry filters and then
closed with the additional same-scanner origin projection. The reviewer
independently recomputed actual679-package framework and376-package Git-consumer
views: only three source/checksum pairs differ; all other package blocks are
byte-identical. Eight persistent guards, actual media two-case0/noWarn, formal
original audit/deny0 and supplemental origin audits0 were read. Real historical
quinn advisory controls demonstrate path/Git0 versus registry-origin1 with the
same five official IDs; no media CVE was invented. Detailed scope/receipts are
in evidence.framework-yanked-zip-repair; raw remains ignored.

## 问题记录

Current bounded source and advisory-coverage increments pass C0/I0/M0. The task
Finding remains open for exact mainCI; all local six/17 now pass; no new exception or
source allowlist is accepted. The earlier PR181 resolved Finding is unchanged.

## 残余风险

The projected lock is a conservative upstream-version scanner input, not a
registry-resolvable build graph. Registry publication is not qualified. The
49 cargo-deny duplicate warnings follow the original framework warn policy,
not an SDK exception or a claim of zero warnings in every tool. Local scans
use independently live-API-verified official caches because Git HTTPS fetch
failed; exact new-main CI still runs its original online commands.

## 未检查项

Exact new merged-mainCI and actual SDK/CLI consumer delivery remain separate.
Local full23 does not establish those outcomes or a complete imported-code security audit.


The later four-file EOF-only import normalization removes exactly four terminal newline bytes, preserving complete license terms and all production Rust. It is a bounded packaging identity increment; it does not claim new runtime evidence or replace the source/coverage review above.


## Full execution evidence increment

The current evidence binds actual7110 execution: prescribed6+independent17 all0, complete raw SHA/bytes, all1763 input maps identical, compiler diagnostics0. Main Rust results3622/0/3 retain the original three opt-in tests and exclude the two repeated child-process result rows. New records do not rereview449 imported files or infer exact mainCI/consumer success. The independent final evidence review is limited to these execution/source-identity records and the Issue188 backlink; source and accepted contracts are unchanged.


## Post-merge qualification — independent rereview

The top-level revision and original challenge retain the actual7110 review and
execution epoch. Only the new named challenge observes the later exact main
`51726454985fd6f064b21a7e9c539e22d494c987`; the Evidence's original observation
is not refreshed. The pending-main paragraphs above are historical, while the
new source-adoption/CI section is the review input for this increment.

The writer's checks found PR190 merged as9886502, exactly29 semantic-only
candidate-to-squash differences, retained ZIP repair sources on the later main,
and its actual lock/workflow successors. Main→c3b changes only the two contract
documents. Read-only remote queries found run39984523075 completed/success with
all seven same-head jobs, including dependency-policy; Issue188/189 remain open.
The independent review must challenge a mistaken whole-source equivalence,
relabeling of old6+17 as current execution, wrong CI head/status, and accidental
closure/risk acceptance of the unavailable-history Finding.

Independent reviewer `/root/review_framework_adr` reviewed the three-file
post-merge increment read-only on 2026-10-24 and returned PASS, C0/I0/M0. The
review input was frozen patch SHA-256
`d4967ea0917e4bf2f824de7b9248d9264584f57f0a942e55db15b5e226cc4344`
on base `c3b56b4ee8a73565e6a2a621b9a7ddfb644ed3aa`; that patch still kept the
Finding open. The reviewer independently checked the Git adoption/ancestry,
lock and retained repair sources, and queried PR190 and the exact-main run and
jobs, including successful dependency-policy scanning steps. The reviewer
confirmed the original three-link chain and byte-identical Issue189 Finding.
The conclusion explicitly supports local resolved settlement after this real
rereview is recorded and the normal semantic checks pass. This record is added
after that review, not misrepresented as part of the reviewed patch; the final
state/record increment receives a separate read-only confirmation before commit.
The candidate ledger's earlier PASS0 is a different review and is not reused as
the formal rereview. External Issue188 remains open and requires a separate action.
No new local Rust/scanner run, real SDK/CLI consumer acceptance, registry
publication, comprehensive imported-code audit or historical-tree recovery is
claimed. The current contribution changes only the three existing ZIP objects.
