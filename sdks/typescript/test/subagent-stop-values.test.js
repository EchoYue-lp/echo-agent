import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SubagentStopStatus, subagentStopStatusAsStr } from "../dist/index.js";

test("Subagent stop statuses preserve stable hook spellings", () => {
  assert.equal(subagentStopStatusAsStr(SubagentStopStatus.TimedOut), "timed_out");
  assert.equal(SubagentStopStatus.Completed, "completed");
});

test("Subagent stop value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/subagent_stop_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
