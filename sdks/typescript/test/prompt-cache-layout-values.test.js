import test from "node:test";
import assert from "node:assert/strict";
import { PromptCacheLayout, toWireValue } from "../dist/index.js";

const message = (role, text) => ({ role, content: toWireValue(text) });

test("PromptCacheLayout projects Rust cache segments and ranges", () => {
  const layout = PromptCacheLayout.fromMessages([
    message("system", "You are Echo Agent"),
    message("system", "[Canonical context - restored]"),
    message("user", "hello"),
    message("assistant", "hi"),
    message("user", "[runtime_context: turn 1]"),
    message("user", "[runtime_context: hook]"),
  ], [{ name: "lookup", description: "", parameters: toWireValue({}), tool_type: "function" }]);

  assert.equal(layout.system.length, 1);
  assert.equal(layout.canonical.length, 1);
  assert.equal(layout.history.length, 2);
  assert.equal(layout.runtimeContext.length, 2);
  assert.equal(layout.tools.length, 1);
  const ranges = layout.segmentRanges();
  assert.deepEqual([
    [ranges.system.start, ranges.system.end],
    [ranges.canonical.start, ranges.canonical.end],
    [ranges.history.start, ranges.history.end],
    [ranges.runtimeContext.start, ranges.runtimeContext.end],
  ], [[0n, 1n], [1n, 2n], [2n, 4n], [4n, 6n]]);
});

test("PromptCacheLayout leaves canonical empty when no marker exists", () => {
  const layout = PromptCacheLayout.fromMessages([
    message("system", "S"),
    message("user", "hello"),
  ], []);
  assert.equal(layout.canonical.length, 0);
  assert.equal(layout.history.length, 1);
  assert.equal(layout.segmentRanges().canonical.isEmpty(), true);
});
