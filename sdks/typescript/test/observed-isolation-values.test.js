import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ObservedIsolation } from "../dist/index.js";

test("observed isolation preserves trim, default and Unicode bounds", () => {
  assert.equal(ObservedIsolation.new("  worktree  ").asStr(), "worktree");
  assert.equal(ObservedIsolation.new("   ").asStr(), "unknown");
  assert.equal(Array.from(ObservedIsolation.new("😀".repeat(600)).asStr()).length, 512);
});

test("observed isolation mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/observed_isolation_values"),
  );
  assert.equal(entries.length, 4);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
