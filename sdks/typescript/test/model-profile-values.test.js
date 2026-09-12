import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  ModelProfile,
  ModelProfileResolver,
  ThinkingProtocol,
  inferContextWindow,
  modelProfileOverride,
} from "../dist/index.js";

test("model profiles preserve Rust provider and model policy semantics", () => {
  const openai = ModelProfile.fromProviderName("gpt-5.6-sol", "openai");
  assert.equal(openai.supportsReasoning, true);
  assert.equal(openai.maxOutputTokens, 16_384);
  assert.equal(openai.contextWindow, 1_050_000);
  assert.equal(openai.tokenizerName, "o200k_base");
  assert.equal(openai.thinkingProtocol, ThinkingProtocol.OpenaiReasoningEffort);
  assert.equal(openai.supportsImages, true);
  assert.equal(ModelProfile.fromProviderName("o3-mini", "openai").supportsImages, false);
  assert.equal(inferContextWindow(" moonshot ", "kimi-k2.7-code"), 256_000);
  assert.equal(inferContextWindow("openai", "gpt-5.5"), undefined);
});

test("model profile resolver applies provider defaults before exact overrides", () => {
  assert.deepEqual(modelProfileOverride({ excludedTools: ["shell", "shell"] }).excludedTools, ["shell"]);
  const profile = ModelProfileResolver.new()
    .registerProviderDefault(" OpenAI ", modelProfileOverride({
      supportsParallelToolCalls: false,
      contextWindow: 99,
      excludedTools: ["shell"],
    }))
    .registerExact("openai", "gpt-5.6-sol", modelProfileOverride({
      supportsParallelToolCalls: true,
      supportsStructuredOutput: false,
      promptSuffix: "exact",
      excludedTools: ["browser"],
    }))
    .resolve("openai", "gpt-5.6-sol", ModelProfile.fromProviderName("gpt-5.6-sol", "openai").capabilities);
  assert.equal(profile.supportsParallelToolCalls, true);
  assert.equal(profile.contextWindow, 99);
  assert.equal(profile.capabilities.structuredOutput, false);
  assert.equal(profile.promptSuffix, "exact");
  assert.deepEqual([...profile.excludedTools].sort(), ["browser", "shell"]);
});

test("model profile mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/model_profile_values"));
  assert.equal(entries.length, 32);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
