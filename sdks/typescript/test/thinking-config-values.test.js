import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ThinkingConfig } from "../dist/index.js";
import { ThinkingLevel } from "../dist/thinking.js";

test("thinking config preserves provider projections and parsing", () => {
  assert.equal(ThinkingConfig.medium().kind, "level");
  assert.equal(ThinkingConfig.parseSpec("auto"), undefined);
  assert.equal(ThinkingConfig.parseSpec("4000").kind, "budget_tokens");
  assert.equal(ThinkingConfig.parseSpec("high").level, ThinkingLevel.High);
  assert.equal(ThinkingConfig.toReasoningEffort(ThinkingConfig.Disabled), "minimal");
  assert.equal(ThinkingConfig.toAnthropicEffort(ThinkingConfig.Disabled), undefined);
  assert.equal(ThinkingConfig.toAnthropicBudget(ThinkingConfig.medium(), 10_000), 5_000);
  assert.equal(ThinkingConfig.toAnthropicBudget(ThinkingConfig.BudgetTokens(20_000), 10_000), 9_999);
  assert.equal(ThinkingConfig.toEnableThinking(ThinkingConfig.Level(ThinkingLevel.Minimal)), false);
  assert.equal(ThinkingConfig.toGlmThinkingType(ThinkingConfig.Level(ThinkingLevel.High)), "enabled");
  assert.equal(ThinkingConfig.toGlmReasoningEffort(ThinkingConfig.BudgetTokens(50_000)), "max");
  assert.throws(() => ThinkingConfig.parseSpec("bogus"), TypeError);
});

test("thinking config mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/thinking_config_values"),
  );
  assert.equal(entries.length, 14);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
