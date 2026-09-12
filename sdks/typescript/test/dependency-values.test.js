import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DependencyKind, SkillSource } from "../dist/index.js";

test("dependency and skill source values preserve stable spellings", () => {
  assert.equal(DependencyKind.PythonPkg, "python_pkg");
  assert.equal(DependencyKind.NodeModule, "node_module");
  assert.equal(SkillSource.Mcp, "mcp");
});

test("dependency value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/dependency_values"),
  );
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
