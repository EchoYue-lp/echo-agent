#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

command -v node >/dev/null || { echo "error: node is required for the TypeScript SDK" >&2; exit 1; }
command -v npm >/dev/null || { echo "error: npm is required for the TypeScript SDK" >&2; exit 1; }
command -v uv >/dev/null || { echo "error: uv is required for the Python SDK" >&2; exit 1; }
command -v mvn >/dev/null || { echo "error: Maven is required for the Java SDK" >&2; exit 1; }
command -v cargo >/dev/null || { echo "error: Cargo is required for the Host smoke" >&2; exit 1; }

jq -e '.schema_version == 1 and .source_only == true and .typescript.node == ">=20" and .python.python == ">=3.10" and .java.jdk == "17"' \
  "$repo_root/sdks/shared/toolchain.json" >/dev/null || {
  echo "error: shared SDK toolchain contract is invalid" >&2
  exit 1
}
node_major=$(node -p 'Number(process.versions.node.split(".")[0])')
(( node_major >= 20 )) || { echo "error: Node.js 20 or newer is required" >&2; exit 1; }

if ! (cd "$repo_root/sdks/python" && uv run python -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)'); then
  echo "error: Python 3.10 or newer is required" >&2
  exit 1
fi

java_version=$(java -version 2>&1 | sed -n 's/.*version "\([^"]*\)".*/\1/p' | head -n 1)
java_major=${java_version%%.*}
[[ "$java_major" =~ ^[0-9]+$ ]] && (( java_major >= 17 )) || {
  echo "error: JDK 17 or newer is required" >&2
  exit 1
}

scripts/export-language-sdk-catalog.sh --check

external_incomplete=$(jq -r '
  [.entries[] | select(.sdk_scope == "external_contract") | .languages | to_entries[] | select(.value.status != "done")] | length
' "$repo_root/contracts/sdk/parity-manifest.json")
[[ "$external_incomplete" == "0" ]] || {
  echo "error: external SDK contracts contain incomplete language mappings" >&2
  exit 1
}
scope_counts=$(jq -r '
  [.entries[] | select(.canonical)]
  | [
      ([.[] | select(.sdk_scope == "external_contract")] | length),
      ([.[] | select(.sdk_scope == "host_or_rust_only")] | length),
      ([.[] | select(.sdk_scope == "language_intrinsic")] | length),
      ([.[] | select(.sdk_scope == "internal_helper")] | length),
      ([.[] | select(.sdk_scope == "deferred")] | length)
    ]
  | @tsv
' "$repo_root/contracts/sdk/parity-manifest.json")
[[ "$scope_counts" == $'5607\t1765\t780\t90\t1441' ]] || {
  echo "error: SDK scope counts drifted: $scope_counts" >&2
  exit 1
}
printf 'SDK scopes (canonical): external=%s host_or_rust_only=%s language_intrinsic=%s internal_helper=%s deferred=%s\n' \
  ${scope_counts//$'\t'/ }

cargo build -q -p echo-sdk-host --features sdk-facade-all --locked
host_state=$(mktemp -d "${TMPDIR:-/tmp}/echo-sdk-language-state.XXXXXX")
host_config=$(mktemp "${TMPDIR:-/tmp}/echo-sdk-language-config.XXXXXX")
jq --arg root "$host_state" '.sdk_profile.state_root = $root | .default_agent.agent.enable_memory = true | .default_agent.agent.memory_path = ($root + "/memory.jsonl")' \
  "$repo_root/echo-sdk-host/config.sdk.example.json" > "$host_config"
export ECHO_AGENT_SDK_HOST="$repo_root/target/debug/echo-agent-sdk-host"
export ECHO_AGENT_SDK_CONFIG="$host_config"

(cd "$repo_root/sdks/typescript" && npm ci && npm test && node dist-examples/examples/quickstart.js)
(cd "$repo_root/sdks/python" && \
  PYTHONPATH=src uv run --no-project --with ruff ruff check src tests examples && \
  PYTHONPATH=src uv run --no-project --with ruff ruff format --check src tests examples && \
  PYTHONPATH=src uv run --no-project --with pytest --with pytest-asyncio --with agent-client-protocol==0.12.1 pytest -q && \
  PYTHONPATH=src uv run python examples/quickstart.py)
(cd "$repo_root/sdks/java" && mvn -q test && mvn -q dependency:build-classpath -Dmdep.outputFile="$host_state/java.cp" && java -cp "target/classes:$(cat "$host_state/java.cp")" com.echoagent.sdk.Example "$ECHO_AGENT_SDK_HOST" "$ECHO_AGENT_SDK_CONFIG" "$repo_root/sdks/shared/facade-operation-catalog.json")

printf 'language SDK source checks passed\n'
