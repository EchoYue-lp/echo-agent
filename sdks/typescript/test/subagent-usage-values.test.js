import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LlmUsageStats } from "../dist/index.js";

test("Subagent usage accumulates and emits the canonical payload", () => {
  const stats = new LlmUsageStats();
  stats.record("model", 100n, 50n, 150n, 80n, 10n, true);
  stats.record("model", 200n, 60n, 260n, 150n, 20n, false);
  const payload = stats.toPayload("session");
  assert.equal(payload.prompt_tokens, "300");
  assert.equal(payload.call_count, "2");
  assert.equal(payload.usage_reported, true);
});

test("Subagent usage mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/subagent_usage_values"),
  );
  assert.equal(entries.length, 3);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
