import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { RetryPolicy } from "../dist/index.js";

test("retry policy preserves exponential backoff and immutable configuration", () => {
  const policy = RetryPolicy.new(5, 100).maxDelay(800).jitter(false);
  assert.equal(policy.delayFor(0), 0);
  assert.equal(policy.delayFor(1), 100);
  assert.equal(policy.delayFor(2), 200);
  assert.equal(policy.delayFor(4), 800);
  assert.equal(RetryPolicy.noRetry().delayFor(1), 0);
  assert.throws(() => RetryPolicy.new(-1, 100), RangeError);
});

test("retry policy mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/retry_policy_values"),
  );
  assert.equal(entries.length, 9);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
