import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { hookActionKind, hookActionValidate, hookCommand, hookHttp, hookPermission } from "../dist/index.js";

test("hook actions preserve tagged values and configuration validation", () => {
  const command = hookCommand("echo ok");
  assert.equal(hookActionKind(command), "command");
  hookActionValidate(command);
  hookActionValidate(hookPermission("ask"));
  hookActionValidate(hookHttp("http://localhost:8080/hook"));
  assert.throws(() => hookActionValidate(hookPermission("maybe")));
});

test("hook action mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/hook_action_values"));
  assert.equal(entries.length, 30);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
