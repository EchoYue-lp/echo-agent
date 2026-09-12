import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { EventId, EventIdentity, StreamId } from "../dist/index.js";

test("event identities preserve validation, UUID-backed constructors and immutable updates", () => {
  assert.equal(EventId.new("evt-1").asStr(), "evt-1");
  assert.equal(StreamId.new("stream-1").toString(), "stream-1");
  assert.throws(() => EventId.new("  "), TypeError);
  const identity = EventIdentity.new("stream-1", "turn-1")
    .withRunId("run-1")
    .withMessageId("message-1")
    .withExecutionId("exec-1")
    .withConversationId("conversation-1")
    .withParentEventId("event-0");
  assert.equal(identity.streamId.asStr(), "stream-1");
  assert.equal(identity.turnId, "turn-1");
  assert.equal(identity.parentEventId, "event-0");
  assert.equal(EventIdentity.forRun("run-2").executionId, "run-2");
  assert.equal(
    EventIdentity.forChat("conversation-2", "turn-2", "message-2", "run-2").messageId,
    "message-2",
  );
  assert.equal(
    EventIdentity.fromRuntimeContext({ run_id: "run-3", execution_id: "exec-3" }).turnId,
    "exec-3",
  );
  assert.equal(
    EventIdentity.fromInvocation({ runtime: { conversation_id: "conversation-3" } }).conversationId,
    "conversation-3",
  );
});

test("event identity mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/event_identity_values"),
  );
  assert.equal(entries.length, 29);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
