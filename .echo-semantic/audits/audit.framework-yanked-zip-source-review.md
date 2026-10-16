---
schema_version: 1
id: audit.framework-yanked-zip-source-review
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: source:d05de8aeb8655e4bc78aa98764cb6e04421860a891a734821c24327d5f3559e3
finding_refs:
- finding.framework-yanked-zip-ci
challenges:
  original-media-source-and-registry-advisory-coverage:
    revision: source:d05de8aeb8655e4bc78aa98764cb6e04421860a891a734821c24327d5f3559e3
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
Finding remains open for full six/17 and exact mainCI; no new exception or
source allowlist is accepted. The earlier PR181 resolved Finding is unchanged.

## 残余风险

The projected lock is a conservative upstream-version scanner input, not a
registry-resolvable build graph. Registry publication is not qualified. The
49 cargo-deny duplicate warnings follow the original framework warn policy,
not an SDK exception or a claim of zero warnings in every tool. Local scans
use independently live-API-verified official caches because Git HTTPS fetch
failed; exact new-main CI still runs its original online commands.

## 未检查项

New complete workspace/independent feature gates, new merged-mainCI and actual
SDK/CLI consumer delivery remain separate. Current code provenance and focused
checks do not establish those outcomes or a complete imported-code security audit.


The later four-file EOF-only import normalization removes exactly four terminal newline bytes, preserving complete license terms and all production Rust. It is a bounded packaging identity increment; it does not claim new runtime evidence or replace the source/coverage review above.
