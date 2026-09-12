import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  contentGuardDetected,
  contentGuardPass,
  contentGuardRedacted,
  contentGuardRejected,
  contentGuardResultIsRejected,
} from "../dist/index.js";

test("content guard values preserve variant payloads", () => {
  assert.equal(contentGuardPass().kind, "pass");
  assert.deepEqual(contentGuardDetected(["email"]).piiTypes, ["email"]);
  const rejected = contentGuardRejected(["phone"]);
  assert.equal(contentGuardResultIsRejected(rejected), true);
  assert.equal(contentGuardResultIsRejected(contentGuardRedacted("safe")), false);
});

test("content guard value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/content_guard_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
