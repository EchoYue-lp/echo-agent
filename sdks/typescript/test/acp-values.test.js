import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  ConnectionMode,
  ExtensionSettlement,
  ExtensionLeaseError,
  acpAdapterConfigDefault,
  acpAdapterConfigValidate,
  acpLedgerLimits,
  acpLedgerLimitsDefault,
  extensionSettlementIsAnswered,
} from "../dist/index.js";

test("ACP runtime values preserve settlement and bounded ledger semantics", () => {
  assert.equal(ConnectionMode.Standard, "standard");
  assert.equal(ExtensionSettlement.TimedOut, "timed_out");
  assert.equal(ExtensionLeaseError.ConcurrencyLimit, "extension concurrency limit reached");
  assert.equal(extensionSettlementIsAnswered(ExtensionSettlement.Answered), true);
  assert.equal(extensionSettlementIsAnswered(ExtensionSettlement.Cancelled), false);
  assert.deepEqual(acpLedgerLimitsDefault(), { maxEvents: 10_000n, maxBytes: 8_388_608n });
  assert.deepEqual(acpLedgerLimits(2n, 3n), { maxEvents: 2n, maxBytes: 3n });
  assert.throws(() => acpLedgerLimits(-1n, 1n), RangeError);
  const config = acpAdapterConfigDefault();
  acpAdapterConfigValidate(config);
  assert.equal(config.shutdownTimeout.seconds, "5");
  assert.throws(
    () => acpAdapterConfigValidate({ ...config, maxSessions: 0n }),
    /resource limits must be positive/,
  );
});

test("ACP runtime value mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/acp_values"),
  );
  assert.equal(entries.length, 13);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});

test("ACP adapter config mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/acp_config_values"),
  );
  assert.equal(entries.length, 13);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});

test("ACP lease error mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/acp_lease_values"),
  );
  assert.equal(entries.length, 5);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
