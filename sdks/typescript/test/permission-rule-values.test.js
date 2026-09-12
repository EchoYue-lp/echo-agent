import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { RuleSource, ruleSourceParse } from "../dist/index.js";

test("permission rule sources preserve canonical and alias parsing", () => {
  assert.equal(RuleSource.Session, "session");
  assert.equal(ruleSourceParse("local_settings"), RuleSource.LocalSettings);
  assert.equal(ruleSourceParse("manual"), RuleSource.UserSettings);
  assert.throws(() => ruleSourceParse("unknown"), /unknown permission rule source/);
});

test("permission rule source mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/permission_rule_values"),
  );
  assert.equal(entries.length, 10);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
