import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  guardDecisionBlock,
  guardDecisionIsBlocked,
  guardDecisionPass,
  guardDecisionTransform,
  guardDecisionWarn,
} from "../dist/index.js";

test("guard decisions preserve variant payloads", () => {
  assert.equal(guardDecisionPass().kind, "pass");
  assert.equal(guardDecisionIsBlocked(guardDecisionBlock("unsafe")), true);
  assert.deepEqual(guardDecisionWarn(["one"]).reasons, ["one"]);
  assert.deepEqual(guardDecisionTransform("safe", ["redacted"]), {
    kind: "transform",
    content: "safe",
    reasons: ["redacted"],
  });
});

test("guard decision mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/guard_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
