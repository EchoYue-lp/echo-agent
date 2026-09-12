import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ProviderCapabilities } from "../dist/index.js";

test("provider capabilities preserve default dialect snapshots", () => {
  assert.equal(ProviderCapabilities.fromProviderName("anthropic").namedSseEvents, true);
  assert.equal(ProviderCapabilities.fromProviderName(" anthropic ").namedSseEvents, false);
  assert.equal(ProviderCapabilities.fromProviderName("ollama").ndjsonStreaming, true);
  assert.equal(ProviderCapabilities.fromProviderName("custom").toolSupport, true);
  assert.equal(ProviderCapabilities.anthropic().tokenizerName, "claude");
  assert.equal(ProviderCapabilities.openaiCompatible().supportsToolChoiceNone, true);
});

test("provider capability mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/provider_capabilities_values"));
  assert.equal(entries.length, 4);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
