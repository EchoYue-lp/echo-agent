import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ResourceLimits } from "../dist/index.js";

test("sandbox resource limits preserve default, strict and unrestricted policies", () => {
  assert.equal(ResourceLimits.default().cpuTimeSecs, 30n);
  assert.equal(ResourceLimits.default().memoryBytes, 256n * 1024n * 1024n);
  assert.equal(ResourceLimits.strict().maxProcesses, 8);
  assert.equal(ResourceLimits.unrestricted().network, true);
  assert.equal(ResourceLimits.unrestricted().cpuTimeSecs, undefined);
});

test("sandbox resource limit mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/sandbox_resource_values"),
  );
  assert.equal(entries.length, 4);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
