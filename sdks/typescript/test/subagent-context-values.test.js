import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SubagentContext } from "../dist/index.js";

test("Subagent context preserves empty and content semantics", () => {
  assert.equal(SubagentContext.empty().hasContent(), false);
  assert.equal(new SubagentContext([], [], false, "goal", null).hasContent(), true);
});

test("Subagent context mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/subagent_context_values"),
  );
  assert.equal(entries.length, 3);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
