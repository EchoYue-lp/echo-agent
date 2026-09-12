import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SubagentCommandIdentity } from "../dist/index.js";

test("Subagent command identities preserve validation and attempt projection", () => {
  const identity = new SubagentCommandIdentity("run", "task", "exec", 2n, 0, "command");
  assert.equal(identity.attemptIdentity().executionId, "exec");
  assert.throws(() => new SubagentCommandIdentity("", "task", "exec", 2n, 0, "command"));
  assert.throws(() => new SubagentCommandIdentity("run", "task", "exec", 0n, 0, "command"));
});

test("Subagent command identity mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/subagent_command_identity_values"),
  );
  assert.equal(entries.length, 6);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
