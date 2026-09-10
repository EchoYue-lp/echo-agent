#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
catalog="$repo_root/contracts/sdk/facade-operation-catalog.json"
output="$repo_root/sdks/shared/facade-operation-catalog.json"
digest_output="$repo_root/sdks/shared/contract-digests.json"
schema="$repo_root/contracts/sdk/schema/echo-agent-extension-v1.schema.json"
source_contract="$repo_root/contracts/sdk/source-contract.json"

test -f "$catalog"
test -f "$schema"
test -f "$source_contract"
render() {
  jq '{schema_version, extension_protocol_version, total_items,
      routes: [.routes[] | {
        family, method, operation, handler_operation, route, signature_digests,
        operation_signatures,
        required_feature, required_features, feature_semantics
      }],
      families: [.families[] | {family, methods, operations, required_feature}]}' \
    "$catalog"
}

if [[ "${1:-}" == "--check" ]]; then
  rendered=$(mktemp "${TMPDIR:-/tmp}/echo-sdk-catalog.XXXXXX")
  trap 'rm -f "$rendered"' EXIT
  render > "$rendered"
  cmp -s "$rendered" "$output" || {
    printf 'DRIFT: %s is not generated from %s\n' "$output" "$catalog" >&2
    exit 1
  }
  contract_digest="sha256:$(shasum -a 256 "$schema" | awk '{print $1}')"
  source_digest=$(jq -r '.aggregate_digest' "$source_contract")
  expected_digests=$(printf '{"contract_digest":"%s","source_contract_digest":"%s"}\n' "$contract_digest" "$source_digest")
  actual_digests=$(cat "$digest_output")
  [[ "$expected_digests" == "$actual_digests" ]] || {
    printf 'DRIFT: %s is not generated from current contract artifacts\n' "$digest_output" >&2
    exit 1
  }
  printf 'ok: %s matches canonical facade catalog\n' "$output"
  exit 0
fi

render > "$output"
contract_digest="sha256:$(shasum -a 256 "$schema" | awk '{print $1}')"
source_digest=$(jq -r '.aggregate_digest' "$source_contract")
printf '{"contract_digest":"%s","source_contract_digest":"%s"}\n' \
  "$contract_digest" "$source_digest" > "$digest_output"

printf 'exported %s\n' "$output"
