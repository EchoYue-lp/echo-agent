import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  HookEvent,
  HookEventCategory,
  hookEventAsStr,
  hookEventCategory,
  hookEventFromName,
  hookEventIsToolEvent,
  hookEventSupportsMatcher,
} from "../dist/index.js";

test("hook events preserve names, categories and matcher semantics", () => {
  assert.equal(HookEvent.ALL.length, 31);
  assert.equal(hookEventAsStr(HookEvent.PreToolUse), "PreToolUse");
  assert.equal(hookEventFromName("TaskCompleted"), HookEvent.TaskCompleted);
  assert.equal(hookEventFromName("missing"), undefined);
  assert.equal(hookEventCategory(HookEvent.PreToolUse), HookEventCategory.Tool);
  assert.equal(hookEventCategory(HookEvent.StopFailure), HookEventCategory.Error);
  assert.equal(hookEventCategory(HookEvent.RulePromoted), HookEventCategory.Evolution);
  assert.equal(hookEventIsToolEvent(HookEvent.PermissionDenied), true);
  assert.equal(hookEventIsToolEvent(HookEvent.SessionStart), false);
  assert.equal(hookEventSupportsMatcher(HookEvent.StopFailure), true);
});

test("hook event mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/hook_event_values"),
  );
  assert.equal(entries.length, 45);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
