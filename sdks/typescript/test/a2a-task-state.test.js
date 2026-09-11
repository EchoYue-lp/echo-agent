import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  A2AMessage,
  A2ATaskStatus,
  AgentCard,
  AgentProvider,
  AgentSkill,
  TaskState,
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
