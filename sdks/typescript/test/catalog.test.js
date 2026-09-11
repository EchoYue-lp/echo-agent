import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { FacadeCatalog } from "../dist/catalog.js";

const catalogDocument = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../shared/facade-operation-catalog.json", import.meta.url)), "utf8"),
);
const parityManifest = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url)), "utf8"),
);

function expectedOperations() {
  return catalogDocument.routes
    .flatMap((route) => {
      const direct = route.operation && route.method
        ? [{ route, operation: route.operation, signature_digests: route.signature_digests }]
        : [];
      const family = route.method
        ? (route.operation_signatures ?? []).map((entry) => ({
            route,
            operation: entry.operation,
            signature_digests: entry.signature_digests,
          }))
        : [];
      return [...direct, ...family];
    })
    .map(({ route, operation, signature_digests }) => ({
      identity: `${route.family}:${operation}`,
      operation,
      family: route.family,
      method: route.method,
      route: route.route,
      signature: signature_digests[0],
      signature_digests,
      required_feature: route.required_feature ?? null,
      required_features: route.required_features ?? [],
      feature_semantics: route.feature_semantics ?? "default",
    }))
    .sort((left, right) => left.identity.localeCompare(right.identity));
}

test("canonical resolver selects source and family routes", () => {
  const catalog = new FacadeCatalog();
  assert.equal(catalog.resolve("echo_core::agent::Agent::name").method, "_echo_agent/facade/invoke");
  assert.equal(catalog.resolve("memory.store.put").method, "_echo_agent/memory/op");
  assert.equal(catalog.resolve("memory.store.put").family, "memory");
});

test("operations enumerate every canonical source and family operation", () => {
  const catalog = new FacadeCatalog();
  const expected = expectedOperations();
  const actual = catalog.operations();

  assert.equal(actual.length, expected.length);
  assert.equal(new Set(actual.map((operation) => operation.identity)).size, actual.length);
  assert.deepEqual(actual, expected);
});

test("families group the same canonical operation identities", () => {
  const catalog = new FacadeCatalog();
  const expected = expectedOperations();
  const grouped = catalog.families().flatMap((family) => family.operations);

  assert.equal(catalog.families().length, catalog.document.families.length);
  assert.equal(grouped.length, expected.length);
  assert.deepEqual(
    grouped.map((operation) => operation.identity).sort(),
    expected.map((operation) => operation.identity).sort(),
  );
  assert.equal(new Set(catalog.families().map((family) => family.family)).size, catalog.families().length);
});

test("every executable facade item has a completed TypeScript mapping", () => {
  assert.ok(parityManifest.entries.length > 0);
  for (const entry of parityManifest.entries) {
    if (entry.route.surface !== "intrinsic") {
      assert.equal(entry.languages.typescript.status, "done", entry.path);
    }
    assert.match(entry.languages.typescript.contract_test, /^sdk-parity\//u, entry.path);
  }
});
