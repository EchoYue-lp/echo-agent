import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ruleMatcherMatches, ruleMatcherMatchesMatcherStr, ruleMatcherParse, ruleMatcherDisplay } from "../dist/index.js";

test("permission rule matchers preserve parse/display/matching semantics", () => {
  const pattern = ruleMatcherParse("pattern:Bash(rm:*)");
  assert.equal(ruleMatcherMatches(pattern, "Bash(rm:rf)", []), true);
  assert.equal(ruleMatcherMatches(ruleMatcherParse("pattern:*"), "anything", []), true);
  assert.equal(ruleMatcherMatches(ruleMatcherParse("pattern:Bash(rm:?f)"), "Bash(rm:rf)", []), true);
  assert.equal(ruleMatcherMatches(ruleMatcherParse("pattern:Bash(*:*)"), "Bash(git:status)", []), true);
  assert.equal(ruleMatcherMatchesMatcherStr(pattern, "Bash(rm:*)"), true);
  assert.equal(ruleMatcherDisplay(ruleMatcherParse("perm:read")), "permission:read");
  assert.throws(() => ruleMatcherParse("unknown"), /unsupported permission matcher/);
});

test("permission rule matcher mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/permission_rule_matcher"),
  );
  assert.equal(entries.length, 9);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
