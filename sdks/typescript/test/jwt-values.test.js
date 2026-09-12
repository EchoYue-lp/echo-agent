import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { JwtClaims, JwtConfig } from "../dist/index.js";

test("JWT config and claims preserve local value semantics", () => {
  const config = JwtConfig.hs256("secret").withIssuer("echo-agent").withAudience("a2a");
  assert.equal(config.isEnabled(), true);
  assert.match(config.toString(), /verification_key: \[redacted\]/);
  assert.equal(JwtConfig.disabled().isEnabled(), false);
  assert.equal(new JwtClaims(null, "subject").subject(), "subject");
  assert.throws(() => JwtConfig.rs256("invalid"), /invalid RSA public key/);
});

test("JWT mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/jwt_values"),
  );
  assert.equal(entries.length, 9);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
