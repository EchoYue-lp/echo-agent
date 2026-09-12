import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SegmentRange } from "../dist/index.js";

test("segment ranges preserve saturating half-open length", () => {
  assert.equal(new SegmentRange(2n, 5n).len(), 3n);
  assert.equal(new SegmentRange(5n, 2n).len(), 0n);
  assert.equal(new SegmentRange(5n, 2n).isEmpty(), true);
});

test("segment range mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/segment_range_values"),
  );
  assert.equal(entries.length, 3);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
