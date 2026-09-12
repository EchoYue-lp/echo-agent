import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  PermissionMode,
  permissionModeAllowsWrite,
  permissionModeParse,
  permissionModeRequiresInteraction,
  permissionModeUsesClassifier,
} from "../dist/index.js";

test("permission modes preserve aliases and helper semantics", () => {
  assert.equal(permissionModeParse("autoedit"), PermissionMode.AcceptEdits);
  assert.equal(permissionModeParse("ask"), PermissionMode.Default);
  assert.equal(permissionModeAllowsWrite(PermissionMode.AcceptEdits), true);
  assert.equal(permissionModeRequiresInteraction(PermissionMode.StrictConfirm), true);
  assert.equal(permissionModeUsesClassifier(PermissionMode.Auto), true);
  assert.throws(() => permissionModeParse("unknown"), /invalid permission mode/);
});

test("permission mode helper mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/permission_mode_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
