import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  AgentSteerPhase,
  AgentSteerTurnOutcome,
  agentSteerStateAccepted,
  agentSteerStateDrained,
  agentSteerStatePhase,
  agentSteerStateTurnSettled,
  agentSteerStateWasDrained,
  agentSteerTurnOutcomeAsStr,
  agentSteerTurnOutcomeParse,
} from "../dist/index.js";

test("steering values preserve lifecycle and terminal semantics", () => {
  const accepted = agentSteerStateAccepted();
  const drained = agentSteerStateDrained();
  const settled = agentSteerStateTurnSettled(AgentSteerTurnOutcome.Completed, true);
  assert.equal(agentSteerStatePhase(accepted), AgentSteerPhase.Accepted);
  assert.equal(agentSteerStateWasDrained(accepted), false);
  assert.equal(agentSteerStatePhase(drained), AgentSteerPhase.Drained);
  assert.equal(agentSteerStateWasDrained(drained), true);
  assert.equal(settled.kind, AgentSteerPhase.TurnSettled);
  assert.equal(settled.outcome, AgentSteerTurnOutcome.Completed);
  assert.equal(agentSteerStateWasDrained(settled), true);
  assert.equal(agentSteerTurnOutcomeAsStr(AgentSteerTurnOutcome.Failed), "failed");
  assert.equal(agentSteerTurnOutcomeParse("cancelled"), AgentSteerTurnOutcome.Cancelled);
  assert.equal(agentSteerTurnOutcomeParse("unknown"), undefined);
});

test("steering value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/steering_values"),
  );
  assert.equal(entries.length, 13);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
