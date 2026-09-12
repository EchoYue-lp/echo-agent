import test from "node:test";
import assert from "node:assert/strict";
import { EchoAgentClient } from "../dist/index.js";

const operation = "echo_orchestration::runtime::turn_driver::TurnOutcome::classify";
const finalAnswer = {
  kind: "variant",
  value: {
    type_id: "echo_sdk_protocol::methods::AgentEventWire",
    variant: "final_answer",
    fields: [{ name: "text", value: { kind: "string", value: "done" } }],
  },
};
const tokenEvent = {
  kind: "variant",
  value: {
    type_id: "echo_sdk_protocol::methods::AgentEventWire",
    variant: "token",
    fields: [{ name: "text", value: { kind: "string", value: "partial" } }],
  },
};

test("classifyTurnOutcome delegates with no receiver and preserves Variant/Null", async () => {
  const calls = [];
  const sdk = {
    invoke: async (...args) => {
      calls.push(args);
      return args[2][0] === finalAnswer
        ? {
            kind: "variant",
            value: {
              type_id: "echo_orchestration::runtime::turn_driver::TurnOutcome",
              variant: "completed",
              fields: [],
            },
          }
        : null;
    },
  };

  const classified = await EchoAgentClient.prototype.classifyTurnOutcome.call(sdk, finalAnswer);
  const nonTerminal = await EchoAgentClient.prototype.classifyTurnOutcome.call(sdk, tokenEvent);

  assert.deepEqual(classified, {
    kind: "variant",
    value: {
      type_id: "echo_orchestration::runtime::turn_driver::TurnOutcome",
      variant: "completed",
      fields: [],
    },
  });
  assert.equal(nonTerminal, null);
  assert.deepEqual(calls, [
    [operation, undefined, [finalAnswer]],
    [operation, undefined, [tokenEvent]],
  ]);
});
