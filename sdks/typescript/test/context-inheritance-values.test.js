import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  contextInheritanceForMode,
  contextInheritanceForkDefault,
  contextInheritanceFreshDefault,
} from "../dist/index.js";

test("context inheritance preserves Rust defaults", () => {
  assert.deepEqual(contextInheritanceFreshDefault(), {
    inheritTools: null, inheritHistory: null, inheritMemory: false, injectMetadata: {},
  });
  assert.deepEqual(contextInheritanceForkDefault(), {
    inheritTools: null, inheritHistory: 2n, inheritMemory: true, injectMetadata: {},
  });
  assert.deepEqual(contextInheritanceForMode("team"), {
    inheritTools: [], inheritHistory: 2n, inheritMemory: false, injectMetadata: {},
  });
});

test("context inheritance mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/context_inheritance_values"),
  );
  assert.equal(entries.length, 11);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
