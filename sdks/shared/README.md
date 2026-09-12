# Shared SDK contract inputs

`facade-operation-catalog.json` and `contract-digests.json` are generated from
the canonical Rust contracts. Do not edit them by hand. Regenerate with
`../scripts/export-language-sdk-catalog.sh` after an intentional contract
change; `scripts/check-sdk-contracts.sh` and `scripts/check-language-sdks.sh`
both fail on drift.

Caller-provided runtime requirements are recorded in
[`toolchain.json`](toolchain.json): Node.js 20+, Python 3.10+, and JDK 17.
These are compatibility requirements only; the repository does not install or
bundle any runtime.

Caller-provided runtime requirements are recorded in
[`toolchain.json`](toolchain.json): Node.js 20+, Python 3.10+, and JDK 17.
These are compatibility requirements only; the repository does not install or
bundle any runtime.
