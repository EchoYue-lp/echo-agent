import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ResponseFormat } from "../dist/index.js";

test("response formats preserve Rust tagged values", () => {
  assert.equal(ResponseFormat.isJson(ResponseFormat.text()), false);
  assert.equal(ResponseFormat.isJson(ResponseFormat.jsonObject()), true);
  const value = ResponseFormat.jsonSchema("answer", { type: "object" });
  assert.equal(ResponseFormat.isJson(value), true);
  assert.deepEqual(value, { type: "json_schema", jsonSchema: { name: "answer", schema: { type: "object" }, strict: true } });
});

test("response format mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/response_format_values"));
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
