import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SkillValidationReport } from "../dist/index.js";

test("skill validation reports preserve violation gate semantics", () => {
  assert.equal(new SkillValidationReport("skill").isValid(), true);
  assert.equal(new SkillValidationReport("skill", ["missing name"], []).isValid(), false);
});

test("skill validation mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/skill_validation_values"),
  );
  assert.equal(entries.length, 2);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
