import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ThinkingLevel, thinkingLevelParse } from "../dist/index.js";

test("ThinkingLevel parses Rust aliases and preserves wire values", () => {
  assert.equal(ThinkingLevel.None, "none");
  assert.equal(thinkingLevelParse(" OFF "), ThinkingLevel.None);
  assert.equal(thinkingLevelParse("normal"), ThinkingLevel.Medium);
  assert.equal(thinkingLevelParse("xhigh"), ThinkingLevel.Xhigh);
  assert.equal(thinkingLevelParse("unknown"), undefined);
});

test("ThinkingLevel mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/thinking_level"));
  assert.equal(entries.length, 9);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
