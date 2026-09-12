import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  A2AMessage,
  A2AArtifact,
  A2AError,
  A2AStreamResponse,
  A2ATask,
  A2ATaskParams,
  A2ATaskRequest,
  A2ATaskResponse,
  A2ATaskStatus,
  AgentCard,
  AgentProvider,
  AgentSkill,
  TaskState,
  TaskArtifactUpdateEvent,
  TaskStatusUpdateEvent,
  taskStateCanTransitionTo,
  taskStateIsTerminal,
} from "../dist/index.js";

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

test("A2A value constructors preserve text, status, provider and skill semantics", () => {
  const message = A2AMessage.userText("hello");
  assert.equal(message.role, "user");
  assert.equal(message.textContent(), "hello");
  assert.equal(A2AMessage.agentText("answer").role, "agent");
  const status = A2ATaskStatus.withMessage(TaskState.Working, message);
  assert.equal(status.state, TaskState.Working);
  assert.equal(status.message?.textContent(), "hello");
  assert.match(status.timestamp, /^\d{4}-\d{2}-\d{2}T/);
  const provider = AgentProvider.new("Echo").withUrl("https://example.test");
  assert.equal(provider.organization, "Echo");
  assert.equal(provider.url, "https://example.test");
  const skill = AgentSkill.new("search", "Search docs").withExamples(["rust"]).withTags(["docs"]);
  assert.equal(skill.id, "search");
  assert.deepEqual(skill.examples, ["rust"]);
  assert.deepEqual(skill.tags, ["docs"]);
  assert.throws(() => skill.tags.push("mutate"), TypeError);
  assert.throws(() => message.parts.push({ type: "text", text: "mutate" }), TypeError);
  assert.throws(() => A2AMessage.userText(null), /message text/);
});

test("A2A Agent Card builder preserves local value semantics", () => {
  const skill = AgentSkill.new("search", "Search docs");
  const card = AgentCard.builder("eko", "https://example.test")
    .description("Local agent")
    .version("1.0.0")
    .provider(AgentProvider.new("Echo"))
    .skill(skill)
    .inputModes(["text/plain"])
    .outputModes(["text/plain", "application/json"])
    .streaming()
    .pushNotifications()
    .build();
  assert.equal(card.name, "eko");
  assert.equal(card.description, "Local agent");
  assert.equal(card.url, "https://example.test");
  assert.equal(card.skills[0], skill);
  assert.deepEqual(card.defaultOutputModes, ["text/plain", "application/json"]);
  assert.equal(card.capabilities.streaming, true);
  assert.equal(card.capabilities.pushNotifications, true);
  assert.throws(() => card.skills.push(skill), TypeError);
});

test("A2A Agent Card identities have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/a2a_agent_card"));
  assert.equal(entries.length, 14);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});

test("A2A artifact and error values preserve wire fields and immutability", () => {
  const artifact = A2AArtifact.new([{ type: "text", text: "chunk" }], {
    name: "answer",
    index: 2,
    append: true,
  });
  assert.equal(artifact.name, "answer");
  assert.equal(artifact.index, 2n);
  assert.equal(artifact.append, true);
  assert.equal(artifact.parts[0].type, "text");
  assert.throws(() => artifact.parts.push({ type: "text", text: "mutate" }), TypeError);
  const error = A2AError.new(-32001, "missing");
  assert.deepEqual({ code: error.code, message: error.message }, { code: -32001, message: "missing" });
  assert.throws(() => A2AError.new(2147483648, "overflow"), /i32/);
  assert.throws(() => A2AArtifact.new([], { append: "yes" }), /append/);
  assert.throws(() => A2AArtifact.new([], { index: 1n << 64n }), /usize/);
});

test("A2A wire values have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/a2a_wire_values"));
  assert.equal(entries.length, 8);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});

test("A2A stream values preserve event and response semantics", () => {
  const status = TaskStatusUpdateEvent.new("task-1", A2ATaskStatus.new(TaskState.Working));
  const artifact = TaskArtifactUpdateEvent.new("task-1", A2AArtifact.new([{ type: "text", text: "chunk" }]), true);
  const wrapper = { type: "status", event: status };
  const response = A2AStreamResponse.new("1", wrapper);
  assert.equal(status.taskId, "task-1");
  assert.equal(status.isFinal, false);
  assert.equal(artifact.isFinal, true);
  assert.equal(response.jsonrpc, "2.0");
  assert.equal(response.result?.type, "status");
  wrapper.type = "artifact";
  assert.equal(response.result?.type, "status");
  assert.throws(() => { response.result.type = "artifact"; }, TypeError);
  assert.throws(() => A2AStreamResponse.new("bad", { type: "status", event: {} }), /status event/);
});

test("A2A stream values have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/a2a_stream_values"));
  assert.equal(entries.length, 18);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});

test("A2A task envelopes preserve nested value semantics", () => {
  const message = A2AMessage.userText("hello");
  const params = A2ATaskParams.new(message, "task-1", "session-1");
  const request = A2ATaskRequest.new("request-1", "tasks/send", params);
  const task = A2ATask.new("task-1", A2ATaskStatus.new(TaskState.Working), "session-1", [message]);
  const response = A2ATaskResponse.new("request-1", task);
  assert.equal(request.jsonrpc, "2.0");
  assert.equal(request.params.message.textContent(), "hello");
  assert.equal(task.history[0], message);
  assert.equal(response.result?.id, "task-1");
  assert.throws(() => task.history.push(message), TypeError);
});

test("A2A task envelope values have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/a2a_task_envelopes"));
  assert.equal(entries.length, 15);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
