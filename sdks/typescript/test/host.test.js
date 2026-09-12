import test from "node:test";
import assert from "node:assert/strict";
import { EchoAgentClient } from "../dist/index.js";

test("source SDK reaches a real Host when configured", { skip: !process.env.ECHO_AGENT_SDK_HOST }, async () => {
  let sdk;
  try {
    sdk = await EchoAgentClient.spawn({
      hostCommand: process.env.ECHO_AGENT_SDK_HOST,
      args: ["--config", process.env.ECHO_AGENT_SDK_CONFIG],
      onStderr: (chunk) => process.stderr.write(chunk),
    });
    const registration = await sdk.registerLlmClient(
      "typescript-smoke-llm",
      {
        kind: "llm_client",
        descriptor_version: 1,
        model_name: "typescript-smoke-model",
        supports_streaming: false,
      },
      async (call) => {
        assert.equal(call.invocation.operation, "llm_chat");
        assert.ok(Array.isArray(call.invocation.input.messages));
        assert.ok(call.invocation.input.messages.length > 0);
        return {
          operation: "llm_chat",
          value: {
            message: {
              role: "assistant",
              content: { kind: "string", value: "typescript-smoke" },
            },
            finish_reason: "stop",
            raw: { kind: "map", value: [] },
          },
        };
      },
    );
    const agent = await sdk.createAgent();
    const session = await agent.createSession();
    assert.equal(await session.invoke("echo_core::agent::Agent::name"), "echo-agent");
    await session.invoke("memory.store.put", [["sdk"], "typescript", "ok"]);
    const stored = await session.invoke("memory.store.get", [["sdk"], "typescript"]);
    assert.equal(stored.value, "ok");
    const telemetry = await sdk.call("telemetry.status", undefined);
    assert.equal(typeof telemetry.initialized, "boolean");
    const classifiedOutcome = await sdk.classifyTurnOutcome({
      kind: "variant",
      value: {
        type_id: "echo_sdk_protocol::methods::AgentEventWire",
        variant: "final_answer",
        fields: [{ name: "text", value: { kind: "string", value: "classified" } }],
      },
    });
    assert.deepEqual(classifiedOutcome, {
      kind: "variant",
      value: {
        type_id: "echo_orchestration::runtime::turn_driver::TurnOutcome",
        variant: "completed",
        fields: [],
      },
    });
    const nonTerminalOutcome = await sdk.classifyTurnOutcome({
      kind: "variant",
      value: {
        type_id: "echo_sdk_protocol::methods::AgentEventWire",
        variant: "token",
        fields: [{ name: "text", value: { kind: "string", value: "partial" } }],
      },
    });
    assert.equal(nonTerminalOutcome, null);
    const updates = session.updates();
    const firstUpdate = updates[Symbol.asyncIterator]().next();
    const prompt = await session.prompt("hello");
    assert.equal(prompt.stopReason, "end_turn");
    const update = await firstUpdate;
    assert.equal(update.done, false);
    assert.equal(update.value.sessionId, session.acpSessionId);
    const run = await session.startRun({ mode: "chat", text: "run smoke" });
    const observedEvents = (async () => {
      const items = [];
      for await (const item of run.events) {
        items.push(item);
      }
      return items;
    })();
    const waited = await run.wait();
    assert.equal(waited.settled, true);
    const events = await observedEvents;
    assert.ok(events.length > 0);
    assert.equal(events.at(-1).envelope.payload.event_type, "final_answer");
    assert.ok(events.every((item) => item.stream.id === run.stream.id));
    const runState = await run.get();
    assert.equal(runState.status, "completed");
    assert.equal(await run.status(), "completed");
    assert.equal(await run.outcomeStatus(), "completed");
    const usage = await run.usage();
    assert.equal(typeof usage.duration_ms, "string");
    assert.match(usage.duration_ms, /^\d+$/u);
    assert.ok(usage.tokens_used === null || typeof usage.tokens_used === "string");
    assert.ok(usage.iterations === null || typeof usage.iterations === "string");
    await registration.close();
    await session.close();
    await agent.close();
  } finally {
    await sdk?.close();
  }
});
