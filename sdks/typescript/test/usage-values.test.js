import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Usage } from "../dist/index.js";

test("usage preserves cache priority and effective totals", () => {
  const usage = new Usage({ promptTokens: 100n, completionTokens: 20n, cacheReadInputTokens: 30n });
  assert.equal(usage.cachedPromptTokens(), 30n);
  assert.equal(usage.effectivePromptTokens(), 130n);
  assert.equal(usage.effectiveTotalTokens(), 150n);
  assert.equal(usage.cacheHitRate(), 30 / 130);
  assert.equal(new Usage({ promptTokens: 2n, cacheReadInputTokens: null }).effectivePromptTokens(), 2n);
  assert.throws(() => new Usage({ promptTokens: 0x1_0000_0000n }), RangeError);
});

test("usage mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/usage_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
