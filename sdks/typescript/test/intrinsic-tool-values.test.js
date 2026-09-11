import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ToolCallParams, ToolResult } from "../dist/index.js";

test("ToolCallParams preserves Rust parameter typing and validation", () => {
  const params = ToolCallParams.fromParams({
    query: "echo",
    limit: 3,
    enabled: true,
    nested: { key: "value" },
  });
  assert.equal(params.getStr("query"), "echo");
  assert.equal(params.getNumber("limit"), 3);
  assert.equal(params.getBool("enabled"), true);
  assert.deepEqual(params.get("nested"), { key: "value" });
  assert.equal(params.has("missing"), false);
  assert.equal(params.len(), 4);
  assert.equal(params.isEmpty(), false);
  assert.doesNotThrow(() => params.validateRequired("query", "string"));
  assert.throws(() => params.validateRequired("query", "number"), /expected number, got string/);
  assert.throws(() => params.validateRequired("missing", "string"), /Missing required parameter/);
  assert.equal(ToolCallParams.fromValue("not-an-object").isEmpty(), true);
});

test("ToolResult constructors and immutable modifiers match Rust semantics", () => {
  const success = ToolResult.successJson({ answer: 42 })
    .withMeta("source", "test")
    .withMimeType("application/json")
    .withTruncated(true);
  assert.equal(success.kind.kind, "json");
  assert.equal(success.success, true);
  assert.equal(success.output, '{"answer":42}');
  assert.equal(success.data.kind, "map");
  assert.deepEqual(success.metadata, { source: "test" });
  assert.equal(success.truncated, true);
  const collision = ToolResult.successJson({ kind: "string", value: "foo" });
  assert.equal(collision.data.kind, "map");
  assert.deepEqual(collision.data.value.map((entry) => entry.key.value), ["kind", "value"]);

  const invalid = ToolResult.invalidArguments("query required").withOutput("bad input");
  assert.equal(invalid.success, false);
  assert.equal(invalid.error, "query required");
  assert.equal(invalid.failure.category, "invalid_arguments");
  assert.equal(invalid.failure.recovery, "correct_arguments");
  assert.equal(ToolResult.failure("unavailable", "offline").failure.recovery, "restore_then_retry");
  assert.equal(ToolResult.failure("timeout", "slow").failure.recovery, "verify_then_retry");
  assert.equal(ToolResult.failure("partial_side_effect", "partial").failure.recovery, "verify_then_retry");
  assert.equal(ToolResult.failure("transient", "retry").failure.recovery, "retry");
  assert.equal(ToolResult.failure("permanent", "stop").failure.recovery, "stop");
  assert.throws(() => ToolResult.failure("unknown", "bad"), /unknown tool failure category/);
  assert.doesNotThrow(() => ToolResult.success("ok").withFailure({
    category: "transient",
    recovery: "retry",
    side_effect: "none",
    retry_after_ms: "18446744073709551615",
    idempotency_key: "key-1",
    postcondition: "eventual success",
  }));
  assert.throws(() => ToolResult.success("ok").withFailure({
    category: "transient", recovery: "retry", side_effect: "none", retry_after_ms: "01",
  }), /canonical/);
  assert.throws(() => ToolResult.success("ok").withFailure({
    category: "transient", recovery: "retry", side_effect: "none", postcondition: 42,
  }), /postcondition must be text/);
  assert.equal(ToolResult.success("ok").success, true);
  assert.equal(ToolResult.error("failed").kind.error_code, "tool_error");
});

test("language-local tool value routes have completed TypeScript mappings", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.route.route === "intrinsic:language-local-wire-helper");
  assert.equal(entries.length, 28);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
