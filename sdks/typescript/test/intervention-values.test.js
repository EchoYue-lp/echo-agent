import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { InterventionResult } from "../dist/index.js";

test("intervention result factories preserve local decision fields", () => {
  assert.deepEqual(InterventionResult.allow(), { block: false, cancel: false });
  assert.equal(InterventionResult.block("reason").blockReason, "reason");
  assert.equal(InterventionResult.inject("context").injectedContext, "context");
  assert.equal(InterventionResult.cancel().cancel, true);
  assert.deepEqual(InterventionResult.modifyArgs({ key: "value" }).modifiedArgs, { key: "value" });
  assert.throws(() => InterventionResult.block("  "), TypeError);
});

test("intervention result mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/intervention_values"),
  );
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
