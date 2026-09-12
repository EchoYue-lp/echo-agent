import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { teamStrategyDebate, teamStrategyDescription, teamStrategyName } from "../dist/index.js";

test("team strategies preserve values and descriptions", () => {
  const strategy = teamStrategyDebate("judge", ["a", "b"]);
  assert.equal(teamStrategyName(strategy), "debate");
  assert.deepEqual(strategy.debaters, ["a", "b"]);
  assert.equal(teamStrategyDescription(strategy), "Debaters propose independently and a judge synthesizes");
});

test("team strategy mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/team_strategy_values"),
  );
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
