import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { PromptDiagnostics } from "../dist/index.js";

test("prompt diagnostics preserve record and count semantics", () => {
  const diagnostics = new PromptDiagnostics();
  diagnostics.record("system", "base");
  diagnostics.record("system", "overlay");
  assert.equal(diagnostics.count("system"), 2);
  assert.equal(diagnostics.count("missing"), 0);
});

test("prompt diagnostics mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/prompt_diagnostics_values"),
  );
  assert.equal(entries.length, 3);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
