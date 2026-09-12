import test from "node:test";
import assert from "node:assert/strict";
import { decodeExtensionInvokeCall, EchoAgentClient, ExtensionStreamWriter } from "../dist/index.js";

const extension = { id: "ext-1", generation: "1", kind: "extension" };
const deadline = { seconds: "30", nanos: 0 };

test("typed bridge decoder preserves operation-discriminated inputs", () => {
  const call = decodeExtensionInvokeCall({
    extension,
    invocation_id: "inv-1",
    deadline,
    invocation: {
      operation: "store_put",
      input: {
        namespace: ["sdk"],
        key: "answer",
        value: { kind: "string", value: "ok" },
      },
    },
    stream: null,
  }, "store");

  assert.equal(call.extension, extension);
  assert.equal(call.invocation.operation, "store_put");
  assert.deepEqual(call.invocation.input.namespace, ["sdk"]);
});

test("typed bridge decoder rejects cross-kind and malformed reverse calls", () => {
  const base = {
    extension,
    invocation_id: "inv-1",
    deadline,
    invocation: {
      operation: "llm_chat",
      input: { messages: [] },
    },
  };

  assert.throws(() => decodeExtensionInvokeCall(base, "tool"), /not a tool extension operation/);
  assert.throws(() => decodeExtensionInvokeCall({ ...base, invocation_id: "" }, "llm_client"), /invocation_id/);
  assert.throws(() => decodeExtensionInvokeCall({ ...base, extension: { ...extension, kind: "run" } }, "llm_client"), /extension handle/);
});

test("critic typed bridge preserves the critique wire shape", () => {
  const call = decodeExtensionInvokeCall({
    extension,
    invocation_id: "critic-inv-1",
    deadline,
    invocation: {
      operation: "critic_critique",
      input: { task: "solve", answer: "42", context: "math" },
    },
  }, "critic");

  assert.equal(call.invocation.operation, "critic_critique");
  assert.deepEqual(call.invocation.input, { task: "solve", answer: "42", context: "math" });
});

test("critic typed bridge rejects unrelated operations and malformed inputs", () => {
  const base = {
    extension,
    invocation_id: "critic-inv-1",
    deadline,
    invocation: {
      operation: "critic_critique",
      input: { task: "solve", answer: "42", context: "math" },
    },
  };

  assert.throws(() => decodeExtensionInvokeCall({ ...base, invocation: { ...base.invocation, operation: "llm_chat" } }, "critic"), /not a critic extension operation/);
  assert.throws(() => decodeExtensionInvokeCall({ ...base, invocation: { operation: "critic_critique", input: { task: "solve" } } }, "critic"), /critique invocation input/);
});

test("registerCritic adapts callbacks through the generic extension registration", async () => {
  const calls = [];
  const sdk = {
    registerExtension(...args) {
      calls.push(args);
      return Promise.resolve("registered");
    },
  };
  const result = await EchoAgentClient.prototype.registerCritic.call(
    sdk,
    "critic-1",
    { kind: "critic", descriptor_version: 1, name: "typescript-critic" },
    async () => ({
      operation: "critic_critique",
      value: { score: 9.5, passed: true, feedback: "good", suggestions: [] },
    }),
  );

  assert.equal(result, "registered");
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], "critic");
  assert.equal(calls[0][1], "critic-1");
  const outcome = await calls[0][3]({
    extension,
    invocation_id: "critic-inv-2",
    deadline,
    invocation: {
      operation: "critic_critique",
      input: { task: "solve", answer: "42", context: "math" },
    },
  }, new AbortController().signal);
  assert.deepEqual(outcome, {
    outcome: "result",
    result: {
      operation: "critic_critique",
      value: { score: 9.5, passed: true, feedback: "good", suggestions: [] },
    },
  });
});

test("context compressor typed bridge preserves compression semantics", async () => {
  const calls = [];
  const sdk = {
    registerExtension(...args) {
      calls.push(args);
      return Promise.resolve("registered");
    },
  };
  const result = await EchoAgentClient.prototype.registerContextCompressor.call(
    sdk,
    "compressor-1",
    { kind: "context_compressor", descriptor_version: 1, name: "typescript-compressor" },
    async (call) => ({
      operation: "compressor_compress",
      value: {
        messages: call.invocation.input.messages.slice(-1),
        evicted: call.invocation.input.messages.slice(0, -1),
        checkpoint: null,
      },
    }),
  );

  assert.equal(result, "registered");
  assert.equal(calls[0][0], "context_compressor");
  assert.deepEqual(calls[0][2], {
    kind: "context_compressor",
    descriptor_version: 1,
    name: "typescript-compressor",
  });
  const outcome = await calls[0][3]({
    extension,
    invocation_id: "compress-inv-1",
    deadline,
    invocation: {
      operation: "compressor_compress",
      input: {
        messages: [
          { role: "user", content: { kind: "string", value: "hello" } },
          { role: "assistant", content: { kind: "string", value: "hi" } },
        ],
        token_limit: "4096",
        current_query: "summarize",
        focus_instructions: null,
        tokenizer: {
          resource: { id: "tokenizer-1", generation: "1", kind: "facade_resource" },
          owner_session_id: "session-1",
        },
      },
    },
  }, new AbortController().signal);
  assert.equal(outcome.result.operation, "compressor_compress");
  assert.equal(outcome.result.value.messages[0].role, "assistant");
});

test("context compressor decoder rejects noncanonical token limits", () => {
  assert.throws(() => decodeExtensionInvokeCall({
    extension,
    invocation_id: "compress-inv-invalid",
    deadline,
    invocation: {
      operation: "compressor_compress",
      input: { messages: [], token_limit: "04096" },
    },
  }, "context_compressor"), /canonical token_limit/);
});

test("agent component registration preserves component operation discriminators", async () => {
  const calls = [];
  const sdk = {
    registerExtension(...args) {
      calls.push(args);
      return Promise.resolve("registered");
    },
  };
  await EchoAgentClient.prototype.registerAgentComponent.call(
    sdk,
    "audit-1",
    { kind: "agent_component", descriptor_version: 1, component: "audit_logger", name: "audit" },
    async (call) => ({
      operation: "agent_component_call",
      value: {
        component: call.invocation.input.component,
        result: {
          operation: call.invocation.input.call.operation,
        },
      },
    }),
  );
  const outcome = await calls[0][3]({
    extension,
    invocation_id: "component-inv-1",
    deadline,
    invocation: {
      operation: "agent_component_call",
      input: {
        component: "audit_logger",
        call: {
          operation: "audit_log",
          input: { event: { kind: "map", value: [] } },
        },
      },
    },
  }, new AbortController().signal);
  assert.equal(calls[0][0], "agent_component");
  assert.equal(outcome.result.value.result.operation, "audit_log");
});

test("agent component decoder rejects invalid discriminated variants", () => {
  const call = {
    extension,
    invocation_id: "component-invalid",
    deadline,
    invocation: {
      operation: "agent_component_call",
      input: {
        component: "audit_logger",
        call: { operation: "audit_log", input: {} },
      },
    },
  };
  assert.throws(
    () => decodeExtensionInvokeCall(call, "agent_component"),
    /audit_log input does not match/,
  );
  assert.throws(
    () => decodeExtensionInvokeCall({
      ...call,
      invocation: {
        ...call.invocation,
        input: { component: "run_store", call: { operation: "audit_log", input: { event: { kind: "null" } } } },
      },
    }, "agent_component"),
    /kind\/operation is invalid/,
  );
});

test("agent component decoder covers extended variants and unit inputs", async () => {
  const unit = decodeExtensionInvokeCall({
    extension,
    invocation_id: "component-unit",
    deadline,
    invocation: {
      operation: "agent_component_call",
      input: {
        component: "sandbox_executor",
        call: { operation: "sandbox_is_available" },
      },
    },
  }, "agent_component");
  assert.deepEqual(unit.invocation.input.call.input, {});

  const calls = [];
  const sdk = {
    registerExtension(...args) {
      calls.push(args);
      return Promise.resolve("registered");
    },
  };
  await EchoAgentClient.prototype.registerAgentComponent.call(
    sdk,
    "embedder-1",
    { kind: "agent_component", descriptor_version: 1, component: "embedder", name: "embedder" },
    async () => ({
      operation: "agent_component_call",
      value: {
        component: "embedder",
        result: { operation: "embedder_embed", value: { vector: [0.25, 0.75] } },
      },
    }),
  );
  const outcome = await calls[0][3]({
    extension,
    invocation_id: "component-embed",
    deadline,
    invocation: {
      operation: "agent_component_call",
      input: {
        component: "embedder",
        call: { operation: "embedder_embed", input: { text: "hello" } },
      },
    },
  }, new AbortController().signal);
  assert.deepEqual(outcome.result.value.result.value.vector, [0.25, 0.75]);
});

test("streaming Agent components preserve nested and outer discriminators", () => {
  const stream = { id: "component-stream", generation: "1", kind: "stream" };
  const call = decodeExtensionInvokeCall({
    extension,
    invocation_id: "component-stream-invocation",
    deadline,
    stream,
    invocation: {
      operation: "agent_component_call_stream",
      input: {
        component: "workflow",
        call: { operation: "workflow_run_stream", input: { input: "start" } },
      },
    },
  }, "agent_component");
  assert.equal(call.invocation.operation, "agent_component_call_stream");
  assert.equal(call.invocation.input.call.operation, "workflow_run_stream");
  assert.equal(call.stream.id, "component-stream");
});

test("SkillLoadPolicy calls and component stream terminals stay typed", async () => {
  const policy = decodeExtensionInvokeCall({
    extension,
    invocation_id: "skill-policy",
    deadline,
    invocation: {
      operation: "agent_component_call",
      input: {
        component: "skill_load_policy",
        call: {
          operation: "skill_load_allows",
          input: {
            descriptor: {
              name: "review",
              description: "Review code",
              location: { encoding: "utf8", path: "/tmp/review/SKILL.md" },
              license: null,
              compatibility: null,
              metadata: {},
              source: null,
              allowed_tools: [],
              shell: null,
              paths: [],
              triggers: [],
              hooks: null,
              sandbox: null,
              depends_on: [],
            },
          },
        },
      },
    },
  }, "agent_component");
  assert.equal(policy.invocation.input.call.operation, "skill_load_allows");

  const notifications = [];
  const writer = new ExtensionStreamWriter({
    notify(method, params) {
      notifications.push([method, params]);
      return Promise.resolve();
    },
  }, { id: "stream-typed", generation: "1", kind: "stream" });
  await writer.agentComponentChunk({
    component: "workflow",
    event: { event: "node_start", node_name: "start", step_index: "0" },
  });
  await writer.agentComponentComplete({
    component: "workflow",
    terminal: { result: "done", total_steps: "1", elapsed: { seconds: "0", nanos: 1 } },
  });
  assert.equal(notifications[0][1].event, "chunk");
  assert.equal(notifications[0][1].value.value.event.event, "node_start");
  assert.equal(notifications[1][1].event, "complete");
  assert.equal(notifications[1][1].value.value.terminal.result, "done");
});
