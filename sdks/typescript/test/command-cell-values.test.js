import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CommandCellPhase, commandCellPhaseAsStr, commandCellPhaseIsTerminal } from "../dist/index.js";

test("command cell phases preserve stable spelling and terminal semantics", () => {
  assert.equal(commandCellPhaseAsStr(CommandCellPhase.LaunchFailed), "launch_failed");
  assert.equal(commandCellPhaseIsTerminal(CommandCellPhase.Running), false);
  assert.equal(commandCellPhaseIsTerminal(CommandCellPhase.Succeeded), true);
});

test("command cell phase mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/command_cell_values"),
  );
  assert.equal(entries.length, 10);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
