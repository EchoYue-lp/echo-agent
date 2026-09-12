import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  SubagentCommandPhase,
  SubagentStatus,
  subagentCommandPhaseAsStr,
  subagentCommandPhaseParse,
  subagentStatusAsStr,
  subagentStatusParse,
} from "../dist/index.js";

test("Subagent values preserve stable phase and status semantics", () => {
  assert.equal(subagentCommandPhaseAsStr(SubagentCommandPhase.MailboxAccepted), "mailbox_accepted");
  assert.equal(subagentCommandPhaseParse("turn_settled"), SubagentCommandPhase.TurnSettled);
  assert.equal(subagentCommandPhaseParse("unknown"), undefined);
  assert.equal(subagentStatusAsStr(SubagentStatus.TimedOut), "timed_out");
  assert.equal(subagentStatusParse("completed"), SubagentStatus.Completed);
  assert.throws(() => subagentStatusParse("unknown"), /unknown Subagent status/);
});

test("Subagent value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/subagent_values"),
  );
  assert.equal(entries.length, 15);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
