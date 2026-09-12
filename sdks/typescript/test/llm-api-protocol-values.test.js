import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  LlmApiProtocol,
  llmApiProtocolEndpointPath,
  llmApiProtocolFromEndpoint,
  llmApiProtocolTryFromEndpoint,
} from "../dist/index.js";

test("endpoint helpers preserve Rust protocol detection", () => {
  assert.equal(llmApiProtocolEndpointPath(LlmApiProtocol.ChatCompletions), "chat/completions");
  assert.equal(llmApiProtocolFromEndpoint("https://api.openai.com/v1/responses?trace=true"), LlmApiProtocol.Responses);
  assert.equal(llmApiProtocolFromEndpoint("https://api.anthropic.com/v1/messages"), LlmApiProtocol.Anthropic);
  assert.equal(llmApiProtocolFromEndpoint("https://gateway.example/v1/chat/completions"), LlmApiProtocol.ChatCompletions);
  assert.equal(llmApiProtocolTryFromEndpoint("https://gateway.example/v1"), undefined);
  assert.equal(llmApiProtocolFromEndpoint("https://gateway.example/v1?upstream=https://api.anthropic.com/v1/"), LlmApiProtocol.ChatCompletions);
});

test("protocol mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/llm_api_protocol_values"));
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
