import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ModelInputModality } from "../dist/index.js";

test("modality defaults preserve Rust order and spellings", () => {
  assert.equal(ModelInputModality.Text, "text");
  assert.deepEqual(ModelInputModality.textOnly(), ["text"]);
  assert.deepEqual(ModelInputModality.allSupported(), ["text", "image", "audio", "video"]);
});

test("modality mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/model_input_modality_values"));
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
