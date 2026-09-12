import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LlmTimeouts, TokenBudget, TokenBudgetConfig } from "../dist/index.js";

test("token budget and timeout policies preserve allocation semantics", () => {
  const budget = TokenBudget.new(100_000);
  assert.equal(budget.systemPromptBudget(), 10_000);
  assert.equal(budget.toolDefinitionsBudget(), 5_000);
  assert.equal(budget.conversationBudget(), 65_000);
  const allocation = budget.allocate(5_000, 2_000, 75_000);
  assert.equal(allocation.ok(), false);
  assert.equal(allocation.needsCompression(), true);
  assert.equal(allocation.conversationExcess, 2_000);
  assert.equal(budget.withAllocations(0.05, 0.05, 0.05, 0.05).conversationBudget(), 80_000);
  assert.throws(() => budget.withAllocations(0.8, 0.3, 0, 0), RangeError);
  assert.equal(TokenBudgetConfig.disabled().enabled, false);
  assert.equal(TokenBudgetConfig.enabled().withTotalWindow(10_000).build(1_000).totalWindow, 10_000);
  assert.equal(LlmTimeouts.default().withoutIdleTimeout().idleTimeout(), undefined);
  assert.equal(LlmTimeouts.default().withOverallTimeout(0).overallTimeout(), undefined);
});

test("token budget mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/token_budget_values"),
  );
  assert.equal(entries.length, 35);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
