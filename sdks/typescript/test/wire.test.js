import test from "node:test";
import assert from "node:assert/strict";
import {
  fromWireValue,
  toWireValue,
  wireBytes,
  wireDuration,
  wireI64,
  wireTimestamp,
  wireU64,
  wireUtf8Path,
} from "../dist/wire.js";

test("handles are encoded as handles, not as enum-like values", () => {
  const handle = { id: "session-1", generation: "1", kind: "session" };
  assert.deepEqual(toWireValue(handle), { kind: "handle", value: handle });
  assert.deepEqual(fromWireValue(toWireValue(handle)), handle);
});

test("large integers stay textual on the extension wire", () => {
  assert.deepEqual(toWireValue("9223372036854775808"), {
    kind: "string",
    value: "9223372036854775808",
  });
});

test("bigints cover the complete signed and unsigned wire ranges", () => {
  assert.deepEqual(toWireValue(9223372036854775808n), {
    kind: "u64",
    value: "9223372036854775808",
  });
  assert.deepEqual(toWireValue(-(1n << 63n)), {
    kind: "i64",
    value: "-9223372036854775808",
  });
  assert.throws(() => toWireValue(1n << 64n), RangeError);
  assert.throws(() => toWireValue(-(1n << 63n) - 1n), RangeError);
});

test("nested maps and lists round-trip through the closed wire algebra", () => {
  const value = { answer: ["a", { count: "18446744073709551615" }] };
  assert.deepEqual(fromWireValue(toWireValue(value)), value);
});

test("typed and unknown variants retain their discriminator", () => {
  const handle = { id: "run-1", generation: "7", kind: "run" };
  const variant = {
    kind: "variant",
    value: {
      type_id: "echo.RunState",
      variant: "active",
      fields: [{ name: "run", value: { kind: "handle", value: handle } }],
    },
  };
  const value = {
    kind: "unknown",
    value: { type_tag: "future.run-state", payload: variant },
  };
  assert.deepEqual(toWireValue(variant), variant);
  assert.deepEqual(fromWireValue(variant), variant);
  assert.deepEqual(fromWireValue(toWireValue(value)), value);
  assert.deepEqual(fromWireValue({ kind: "path", value: { encoding: "utf8", path: "/tmp/a" } }), {
    kind: "path",
    value: { encoding: "utf8", path: "/tmp/a" },
  });

  // A typed value can carry fields named like a handle in its own object
  // shape; it must remain a WireValue instead of being reinterpreted.
  const recordWithHandleFields = {
    kind: "record",
    id: "record-id",
    generation: "1",
    value: { type_id: "echo.Record", fields: [] },
  };
  assert.deepEqual(toWireValue(recordWithHandleFields), recordWithHandleFields);
});

test("scalar helpers emit canonical lossless shapes", () => {
  assert.deepEqual(wireU64(18446744073709551615n), {
    kind: "u64",
    value: "18446744073709551615",
  });
  assert.deepEqual(wireI64(-(1n << 63n)), {
    kind: "i64",
    value: "-9223372036854775808",
  });
  assert.deepEqual(wireBytes(new Uint8Array([0, 255])), {
    kind: "bytes",
    value: { base64: "AP8" },
  });
  assert.deepEqual(wireUtf8Path("/tmp/a"), {
    kind: "path",
    value: { encoding: "utf8", path: "/tmp/a" },
  });
  assert.deepEqual(wireDuration(2n, 3), {
    kind: "duration",
    value: { seconds: "2", nanos: 3 },
  });
  assert.deepEqual(wireTimestamp(-1n, 4), {
    kind: "timestamp",
    value: { unix_seconds: "-1", nanos: 4 },
  });
  assert.throws(() => wireU64(1n << 64n), RangeError);
  assert.throws(() => wireUtf8Path("relative"), TypeError);
});

test("decoding rejects malformed handles and integer text", () => {
  const malformedHandles = [
    { id: "", generation: "1", kind: "session" },
    { id: "  ", generation: "1", kind: "session" },
    { id: "session-1", generation: "01", kind: "session" },
    { id: "session-1", generation: "18446744073709551616", kind: "session" },
    { id: "session-1", generation: "1", kind: "unknown" },
    { id: "session-1", generation: "1" },
    { id: "session-1", generation: 1, kind: "session" },
    { id: "a".repeat(257), generation: "1", kind: "session" },
  ];
  for (const handle of malformedHandles) {
    assert.throws(() => toWireValue(handle), TypeError);
    assert.throws(() => fromWireValue({ kind: "handle", value: handle }), TypeError);
  }

  assert.equal(fromWireValue({ kind: "u64", value: "0" }), "0");
  assert.equal(fromWireValue({ kind: "u64", value: "18446744073709551615" }), "18446744073709551615");
  assert.equal(fromWireValue({ kind: "i64", value: "-9223372036854775808" }), "-9223372036854775808");
  assert.throws(() => fromWireValue({ kind: "u64", value: 1 }), TypeError);
  assert.throws(() => fromWireValue({ kind: "i64", value: 1 }), TypeError);
  assert.throws(() => fromWireValue({ kind: "u64", value: "01" }), TypeError);
  assert.throws(() => fromWireValue({ kind: "u64", value: "-1" }), RangeError);
  assert.throws(() => fromWireValue({ kind: "i64", value: "-0" }), TypeError);
  assert.throws(() => fromWireValue({ kind: "i64", value: "9223372036854775808" }), RangeError);
  assert.throws(() => fromWireValue({ kind: "u64" }), TypeError);
});

test("integer helpers reject unsafe numbers and non-canonical strings", () => {
  assert.throws(() => wireU64(Number.MAX_SAFE_INTEGER + 1), RangeError);
  assert.throws(() => wireI64(1.5), RangeError);
  assert.throws(() => wireU64("+1"), TypeError);
  assert.throws(() => wireI64(" 1"), TypeError);
  assert.throws(() => wireU64("-0"), TypeError);
});
