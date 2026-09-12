import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ThinkingLevel } from "../dist/thinking.js";
import { ThinkingProtocol, resolveThinkingProfile, ThinkingProfile } from "../dist/index.js";

test("thinking profiles preserve provider/model dialect selection", () => {
  assert.equal(ThinkingProfile.unknown().supportsManualControl(), false);
  assert.equal(resolveThinkingProfile("openai", "gpt-5.6-sol").protocol, ThinkingProtocol.OpenaiReasoningEffort);
  assert.equal(resolveThinkingProfile("anthropic", "claude-opus-4.6", "anthropic").protocol, ThinkingProtocol.AnthropicEffort);
  assert.equal(resolveThinkingProfile("anthropic", "claude-opus-4-6", "anthropic").protocol, ThinkingProtocol.AnthropicEffort);
  assert.equal(resolveThinkingProfile("anthropic", "claude-opus-4.6.7", "anthropic").protocol, ThinkingProtocol.None);
  assert.equal(resolveThinkingProfile("zhipu", "glm-5-2").protocol, ThinkingProtocol.GlmReasoningEffort);
  assert.equal(resolveThinkingProfile("ollama", "qwen3-32b").protocol, ThinkingProtocol.OllamaThink);
  assert.equal(resolveThinkingProfile("dashscope", "deepseek-v4-pro").protocol, ThinkingProtocol.EnableThinkingFlag);
  assert.equal(resolveThinkingProfile("custom", "unknown").levels.length, 0);
  assert.equal(ThinkingProfile.new(ThinkingProtocol.OpenaiReasoningEffort, [ThinkingLevel.High]).supportsManualControl(), true);
  assert.equal(ThinkingProfile.new(ThinkingProtocol.ModelManaged, [ThinkingLevel.High]).supportsManualControl(), false);
  assert.equal(ThinkingProfile.new(ThinkingProtocol.AnthropicAdaptive, [ThinkingLevel.High]).supportsManualControl(), false);
});

test("thinking profile mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/thinking_profile_values"));
  assert.equal(entries.length, 5);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
