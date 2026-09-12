import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ThinkingProtocol, thinkingProtocolEmitsField } from "../dist/index.js";

test("thinking protocols preserve wire names and field emission semantics", () => {
  assert.equal(ThinkingProtocol.OpenaiReasoningEffort, "openai_reasoning_effort");
  assert.equal(thinkingProtocolEmitsField(ThinkingProtocol.None), false);
  assert.equal(thinkingProtocolEmitsField(ThinkingProtocol.ModelManaged), false);
  assert.equal(thinkingProtocolEmitsField(ThinkingProtocol.AnthropicAdaptive), false);
  assert.equal(thinkingProtocolEmitsField(ThinkingProtocol.OllamaThink), true);
});

test("thinking protocol mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/thinking_protocol_values"),
  );
  assert.equal(entries.length, 13);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
