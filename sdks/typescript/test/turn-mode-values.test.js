import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { TurnMode, turnModeAsStr } from "../dist/index.js";

test("turn modes preserve stable stream flavors", () => {
  assert.equal(turnModeAsStr(TurnMode.Chat), "chat");
  assert.equal(turnModeAsStr(TurnMode.Execute), "execute");
});

test("turn mode mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/turn_mode_values"),
  );
  assert.equal(entries.length, 3);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
