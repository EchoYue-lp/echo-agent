import test from "node:test";
import assert from "node:assert/strict";
import { RunHandle } from "../dist/index.js";

const runHandle = {
  id: "run-1",
  generation: "1",
  kind: "run",
};

test("RunHandle receipt helpers call canonical operations with the Run handle", async () => {
  const calls = [];
  const run = Object.create(RunHandle.prototype);
  run.client = {
    call: async (...args) => {
      calls.push(args);
      switch (args[0]) {
        case "echo_orchestration::runtime::turn_driver::TurnReceipt::status":
          return "completed";
        case "echo_orchestration::runtime::turn_driver::TurnOutcome::status":
          return "completed";
        case "echo_orchestration::runtime::turn_driver::TurnReceipt::usage":
          return {
            duration_ms: "18446744073709551615",
            tokens_used: "9223372036854775808",
            iterations: null,
          };
        default:
          throw new Error(`unexpected operation: ${args[0]}`);
      }
    },
  };
  run.wire = runHandle;

  assert.equal(await run.status(), "completed");
  assert.equal(await run.outcomeStatus(), "completed");
  assert.deepEqual(await run.usage(), {
    duration_ms: "18446744073709551615",
    tokens_used: "9223372036854775808",
    iterations: null,
  });
  assert.deepEqual(calls, [
    ["echo_orchestration::runtime::turn_driver::TurnReceipt::status", runHandle, []],
    ["echo_orchestration::runtime::turn_driver::TurnOutcome::status", runHandle, []],
    ["echo_orchestration::runtime::turn_driver::TurnReceipt::usage", runHandle, []],
  ]);
});

test("RunHandle receipt helpers reject a closed handle", async () => {
  const run = Object.create(RunHandle.prototype);
  run.closed = true;
  run.client = { call: async () => "completed" };
  run.wire = runHandle;

  assert.throws(() => run.status(), /run handle is closed/);
  assert.throws(() => run.outcomeStatus(), /run handle is closed/);
  assert.throws(() => run.usage(), /run handle is closed/);
});
