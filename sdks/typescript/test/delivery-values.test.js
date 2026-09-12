import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  DeliveryOutcome,
  DeliveryPhase,
  deliveryOutcomeAsStr,
  deliveryPhaseAsStr,
} from "../dist/index.js";

test("delivery values preserve stable snake-case spellings", () => {
  assert.equal(deliveryOutcomeAsStr(DeliveryOutcome.OutcomeUnknown), "outcome_unknown");
  assert.equal(deliveryPhaseAsStr(DeliveryPhase.EffectStarted), "effect_started");
  assert.equal(deliveryPhaseAsStr(DeliveryPhase.TurnSettled), "turn_settled");
});

test("delivery value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/delivery_values"),
  );
  assert.equal(entries.length, 16);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
