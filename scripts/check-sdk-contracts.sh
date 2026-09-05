#!/usr/bin/env bash

# Read-only SDK contract drift check (design §20.2).
#
# Regenerates every contract artifact in memory (facade inventory, parity
# manifest, extension schema, fixtures) and fails on any drift against the
# committed copies, then runs the artifact-level consistency tests that do
# not need a nightly toolchain.
#
# Prerequisites (NOT auto-installed by this script; design §16/§20.5):
#   rustup toolchain install <toolchain from contracts/sdk/toolchain.json>
#
# Regenerate after intentional changes with:
#   cargo run -p echo-sdk-protocol --bin export_schema --locked -- --update

set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=$(python3 -c "import json;print(json.load(open('contracts/sdk/toolchain.json'))['rustdoc']['toolchain'])")
if ! rustup which --toolchain "$toolchain" rustdoc >/dev/null 2>&1; then
  echo "error: rustdoc toolchain $toolchain is not installed." >&2
  echo "       install it with: rustup toolchain install $toolchain" >&2
  exit 1
fi

cargo run -q -p echo-sdk-protocol --bin export_schema --locked -- --check

cargo test -q -p echo-sdk-protocol \
  --test facade_inventory \
  --test acp_baseline \
  --test extension_contract \
  --locked
