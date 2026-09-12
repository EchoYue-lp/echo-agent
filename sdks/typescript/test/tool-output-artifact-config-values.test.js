import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ToolOutputArtifactConfigValue } from "../dist/index.js";

test("artifact config preserves defaults and immutable builders", () => {
  const config = ToolOutputArtifactConfigValue.new("/tmp/artifacts", "temporary_1h")
    .withThresholdBytes(0n)
    .withMaxAgeSecs(60n);
  assert.equal(config.thresholdBytes, 1n);
  assert.equal(config.maxAgeSecs, 60n);
  assert.equal(ToolOutputArtifactConfigValue.default().maxAgeSecs, 3_600n);
});

test("artifact config mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/tool_output_artifact_config_values"),
  );
  assert.equal(entries.length, 7);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
