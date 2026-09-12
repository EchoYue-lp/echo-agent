import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  localRfc3339Deserialize,
  localRfc3339Serialize,
  nowLocal,
  nowMillis,
  nowSecs,
  optionLocalRfc3339Deserialize,
  optionLocalRfc3339Serialize,
  toLocal,
} from "../dist/index.js";

test("time helpers preserve instant and null serialization semantics", () => {
  const before = Date.now();
  assert.ok(nowMillis() >= before);
  assert.ok(nowSecs() > 1_700_000_000);
  const source = "2026-07-09T01:50:48.876Z";
  const local = localRfc3339Serialize(source);
  assert.match(local, /[+-]\d{2}:\d{2}$/u);
  assert.equal(localRfc3339Deserialize(local).getTime(), new Date(source).getTime());
  assert.equal(toLocal(source), local);
  assert.equal(optionLocalRfc3339Serialize(null), null);
  assert.equal(optionLocalRfc3339Deserialize(null), null);
  assert.match(nowLocal(), /[+-]\d{2}:\d{2}$/u);
  assert.throws(() => localRfc3339Deserialize("bad"), TypeError);
});

test("time helper mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/time_values"),
  );
  assert.equal(entries.length, 8);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
