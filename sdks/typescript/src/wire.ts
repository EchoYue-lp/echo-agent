import type { JsonValue, WireHandle, WireValue } from "./types.js";

const MAX_SAFE_U64 = BigInt(Number.MAX_SAFE_INTEGER);
const MAX_U64 = (1n << 64n) - 1n;
const MIN_I64 = -(1n << 63n);
const HANDLE_KINDS: ReadonlySet<string> = new Set([
  "agent",
  "session",
  "run",
  "stream",
  "task_run",
  "plan_task",
  "subagent",
  "extension",
  "facade_resource",
]);
const WIRE_VALUE_KINDS: ReadonlySet<string> = new Set([
  "null",
  "bool",
  "string",
  "i64",
  "u64",
  "f64",
  "bytes",
  "duration",
  "timestamp",
  "path",
  "handle",
  "list",
  "map",
  "record",
  "variant",
  "unknown",
]);

function integerText(value: bigint | number | string, signed: boolean): string {
  if (typeof value === "number" && !Number.isSafeInteger(value)) {
    throw new RangeError("integer exceeds the safe JavaScript wire range; pass a string or bigint");
  }
  let parsed: bigint;
  try {
    parsed = typeof value === "bigint" ? value : BigInt(value);
  } catch {
    throw new TypeError("wire integer must be a canonical decimal value");
  }
  const upper = signed ? (1n << 63n) - 1n : MAX_U64;
  if (signed ? parsed < MIN_I64 || parsed > upper : parsed < 0n || parsed > upper) {
    throw new RangeError("integer is outside the i64/u64 wire range");
  }
  const text = parsed.toString();
  if (typeof value === "string" && value !== text) {
    throw new TypeError("wire integer string must be canonical decimal text");
  }
  return text;
}

export function wireU64(value: bigint | number | string): WireValue {
  return { kind: "u64", value: integerText(value, false) };
}

export function wireI64(value: bigint | number | string): WireValue {
  return { kind: "i64", value: integerText(value, true) };
}

export function wireBytes(value: Uint8Array): WireValue {
  return { kind: "bytes", value: { base64: Buffer.from(value).toString("base64").replace(/=+$/u, "") } };
}

export function wireUtf8Path(path: string): WireValue {
  if (path.includes("\0") || !(path.startsWith("/") || path.startsWith("\\\\") || /^[A-Za-z]:[\\/]/u.test(path))) {
    throw new TypeError("wire UTF-8 path must be absolute and NUL-free");
  }
  return { kind: "path", value: { encoding: "utf8", path } };
}

export function wireDuration(seconds: bigint | number | string, nanos: number): WireValue {
  if (!Number.isInteger(nanos) || nanos < 0 || nanos >= 1_000_000_000) {
    throw new RangeError("wire duration nanos must be in [0, 1_000_000_000)");
  }
  return { kind: "duration", value: { seconds: integerText(seconds, false), nanos } };
}

export function wireTimestamp(unixSeconds: bigint | number | string, nanos: number, rfc3339?: string): WireValue {
  if (!Number.isInteger(nanos) || nanos < 0 || nanos >= 1_000_000_000) {
    throw new RangeError("wire timestamp nanos must be in [0, 1_000_000_000)");
  }
  return {
    kind: "timestamp",
    value: { unix_seconds: integerText(unixSeconds, true), nanos, ...(rfc3339 === undefined ? {} : { rfc3339 }) },
  };
}

export function toWireValue(value: unknown): WireValue {
  if (value === null || value === undefined) return { kind: "null" };
  if (typeof value === "boolean") return { kind: "bool", value };
  if (typeof value === "string") return { kind: "string", value };
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TypeError("wire numbers must be finite");
    if (Number.isInteger(value)) {
      if (!Number.isSafeInteger(value) || BigInt(value) > MAX_SAFE_U64) {
        throw new RangeError("integer exceeds the safe JavaScript wire range; pass a string");
      }
      return value < 0
        ? { kind: "i64", value: String(value) }
        : { kind: "u64", value: String(value) };
    }
    return { kind: "f64", value };
  }
  if (typeof value === "bigint") {
    if (value < MIN_I64 || value > MAX_U64) {
      throw new RangeError("integer is outside the i64/u64 wire range");
    }
    return value < 0n
      ? { kind: "i64", value: value.toString() }
      : { kind: "u64", value: value.toString() };
  }
  if (Array.isArray(value)) return { kind: "list", value: value.map(toWireValue) };
  if (isWireHandle(value)) return { kind: "handle", value };
  if (isWireValue(value)) return value;
  if (isRecord(value) && hasWireHandleFields(value)) {
    throw new TypeError("wire handle id, generation and kind must be valid");
  }
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).map(([key, entry]) => ({
      key: { kind: "string", value: key } as WireValue,
      value: toWireValue(entry),
    }));
    return { kind: "map", value: entries };
  }
  throw new TypeError(`unsupported wire value type: ${typeof value}`);
}

export function fromWireValue(value: unknown): unknown {
  if (!isRecord(value) || typeof value.kind !== "string") return value;
  switch (value.kind) {
    case "null": return null;
    case "bool": case "string": case "f64": return value.value;
    case "i64": return decodeInteger(value, true);
    case "u64": return decodeInteger(value, false);
    case "handle": return parseWireHandle(value.value);
    case "list": return Array.isArray(value.value) ? value.value.map(fromWireValue) : value.value;
    case "map": {
      if (!Array.isArray(value.value)) return value.value;
      const object: Record<string, unknown> = {};
      for (const entry of value.value) {
        if (!isRecord(entry) || !isRecord(entry.key) || entry.key.kind !== "string") continue;
        object[String(entry.key.value)] = fromWireValue(entry.value);
      }
      return object;
    }
    // Preserve typed and future additive variants with their discriminator;
    // dropping the wrapper would make unknown values impossible to round-trip.
    default: return value;
  }
}

export function isWireHandle(value: unknown): value is WireHandle {
  return isRecord(value)
    && typeof value.id === "string"
    && value.id.trim().length > 0
    && Array.from(value.id).length <= 256
    && typeof value.generation === "string"
    && isCanonicalIntegerText(value.generation, false)
    && typeof value.kind === "string"
    && HANDLE_KINDS.has(value.kind);
}

export function parseWireHandle(value: unknown): WireHandle {
  if (!isWireHandle(value)) {
    throw new TypeError("wire handle id, generation and kind must be valid");
  }
  return value;
}

export function isWireValue(value: unknown): value is WireValue {
  if (!isRecord(value) || typeof value.kind !== "string") return false;
  if (!WIRE_VALUE_KINDS.has(value.kind)) return !hasWireHandleFields(value);
  return value.kind === "null" || "value" in value;
}

function hasWireHandleFields(value: Record<string, unknown>): boolean {
  return "id" in value || "generation" in value;
}

function isCanonicalIntegerText(value: unknown, signed: boolean): value is string {
  if (typeof value !== "string") return false;
  try {
    integerText(value, signed);
    return true;
  } catch {
    return false;
  }
}

function decodeInteger(value: Record<string, unknown>, signed: boolean): string {
  if (typeof value.value !== "string") {
    throw new TypeError("wire integer value must be canonical decimal text");
  }
  return integerText(value.value, signed);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function asJson(value: unknown): JsonValue {
  return value as JsonValue;
}
