import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { jsonRpcNotification, jsonRpcRequest } from "../dist/index.js";

test("JSON-RPC values preserve MCP constructors", () => {
  assert.deepEqual(jsonRpcRequest("tools/list"), { jsonrpc: "2.0", id: null, method: "tools/list", params: null });
  assert.deepEqual(jsonRpcNotification("notifications/initialized", { ok: true }), {
    jsonrpc: "2.0", method: "notifications/initialized", params: { ok: true },
  });
});

test("JSON-RPC value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/jsonrpc_values"),
  );
  assert.equal(entries.length, 4);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
