import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  CommandCellArtifactStatus,
  CommandCellTerminalCause,
  commandCellArtifactStatusAsStr,
  commandCellTerminalCauseAsStr,
} from "../dist/index.js";

test("command cell terminal/artifact values preserve spellings", () => {
  assert.equal(commandCellTerminalCauseAsStr(CommandCellTerminalCause.OutputDrainFailed), "output_drain_failed");
  assert.equal(commandCellArtifactStatusAsStr(CommandCellArtifactStatus.BelowThreshold), "below_threshold");
});

test("command cell terminal/artifact mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/command_cell_status_values"),
  );
  assert.equal(entries.length, 15);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
