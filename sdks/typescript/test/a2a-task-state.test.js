import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { TaskState, taskStateCanTransitionTo, taskStateIsTerminal } from "../dist/index.js";

test("A2A TaskState matches the Rust terminal and transition table", () => {
  assert.equal(taskStateIsTerminal(TaskState.Submitted), false);
  assert.equal(taskStateIsTerminal(TaskState.Completed), true);
  assert.equal(taskStateIsTerminal(TaskState.Failed), true);
  assert.equal(taskStateIsTerminal(TaskState.Canceled), true);
  assert.equal(taskStateCanTransitionTo(TaskState.Submitted, TaskState.Working), true);
  assert.equal(taskStateCanTransitionTo(TaskState.Working, TaskState.InputRequired), true);
  assert.equal(taskStateCanTransitionTo(TaskState.InputRequired, TaskState.Working), true);
  assert.equal(taskStateCanTransitionTo(TaskState.Completed, TaskState.Working), false);
  assert.equal(taskStateCanTransitionTo(TaskState.Submitted, TaskState.Completed), false);
  assert.throws(() => taskStateIsTerminal("unknown"), /unknown A2A task state/);
});

test("A2A TaskState intrinsic identities have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/a2a_task_state"));
  assert.equal(entries.length, 10);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
