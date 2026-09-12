import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ruleBehaviorParse, ruleBehaviorToDecision } from "../dist/index.js";

test("permission rule behavior preserves parse defaults and decisions", () => {
  assert.deepEqual(ruleBehaviorToDecision(ruleBehaviorParse("allow")), { kind: "allow" });
  assert.deepEqual(ruleBehaviorToDecision(ruleBehaviorParse("deny")), {
    kind: "deny",
    reason: "denied by rule",
  });
  assert.deepEqual(ruleBehaviorToDecision(ruleBehaviorParse("ask")), {
    kind: "ask",
    suggestions: ["allow", "deny"],
  });
  assert.throws(() => ruleBehaviorParse("unknown"), /unknown permission rule behavior/);
});

test("permission rule behavior mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/permission_rule_behavior"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
