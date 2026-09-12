import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { TaskTerminalStatus, taskTerminalStatusAsStr } from "../dist/index.js";

test("task terminal statuses preserve stable hook spellings", () => {
  assert.equal(taskTerminalStatusAsStr(TaskTerminalStatus.TimedOut), "timed_out");
  assert.equal(TaskTerminalStatus.Skipped, "skipped");
});

test("task terminal value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/task_terminal_values"),
  );
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
