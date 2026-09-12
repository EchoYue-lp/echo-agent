import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { executionUsageDurationMillis } from "../dist/index.js";

test("execution usage duration defaults to zero when absent", () => {
  assert.equal(executionUsageDurationMillis({}), 0n);
  assert.equal(executionUsageDurationMillis({ duration_ms: "42" }), 42n);
});

test("execution usage mapping is complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/execution_usage_values"),
  );
  assert.equal(entries.length, 1);
  assert.equal(entries[0].languages.typescript.status, "done");
});
