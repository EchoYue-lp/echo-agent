import type {
  JsonValue,
  ToolFailure,
  ToolFailureCategory,
  ToolResultData,
  ToolResultContent,
  ToolResultKind,
  WireValue,
} from "./types.js";
import { toWireValue, wireU64 } from "./wire.js";

const DEFAULT_STREAM_CHUNK_BYTES = 16 * 1024;
const UTF8_ENCODER = new TextEncoder();
const FAILURE_CATEGORIES: ReadonlySet<string> = new Set([
  "invalid_arguments",
  "unavailable",
  "timeout",
  "cancelled",
  "transient",
  "permanent",
  "partial_side_effect",
]);
const RECOVERY_ACTIONS: ReadonlySet<string> = new Set([
  "correct_arguments",
  "retry",
  "restore_then_retry",
  "verify_then_retry",
  "stop",
]);
const SIDE_EFFECTS: ReadonlySet<string> = new Set(["none", "possible", "confirmed"]);

/** JSON value exposed by the Rust `ParamValue` projection. */
export type ParamValue = JsonValue;

/**
 * Type-safe view over a tool call's JSON object.
 *
 * Rust stores the raw JSON and a recursively typed `ParamValue` map. The
 * TypeScript view keeps the same distinction while using the language's
 * native JSON values and immutable snapshots.
 */
export class ToolCallParams {
  public readonly raw: JsonValue;
  private readonly parsed: ReadonlyMap<string, ParamValue>;

  private constructor(raw: JsonValue) {
    this.raw = raw;
    const parsed = new Map<string, ParamValue>();
    if (isJsonObject(raw)) {
      for (const [key, value] of Object.entries(raw)) parsed.set(key, value);
    }
    this.parsed = parsed;
  }

  public static fromValue(value: JsonValue): ToolCallParams {
    return new ToolCallParams(value);
  }

  public static fromParams(params: Readonly<Record<string, JsonValue>>): ToolCallParams {
    if (!isJsonObject(params)) throw new TypeError("params must be a JSON object");
    return new ToolCallParams({ ...params });
  }

  public getStr(key: string): string | undefined {
    const value = this.parsed.get(key);
    return typeof value === "string" ? value : undefined;
  }

  public getNumber(key: string): number | undefined {
    const value = this.parsed.get(key);
    return typeof value === "number" ? value : undefined;
  }

  public getBool(key: string): boolean | undefined {
    const value = this.parsed.get(key);
    return typeof value === "boolean" ? value : undefined;
  }

  public get(key: string): ParamValue | undefined {
    return this.parsed.get(key);
  }

  public validateRequired(key: string, expectedType: string): void {
    const value = this.parsed.get(key);
    if (value === undefined) throw new TypeError(`Missing required parameter: ${key}`);
    const actual = paramTypeName(value);
    if (actual !== expectedType) {
      throw new TypeError(`Parameter '${key}': expected ${expectedType}, got ${actual}`);
    }
  }

  public has(key: string): boolean {
    return this.parsed.has(key);
  }

  public len(): number {
    return this.parsed.size;
  }

  public isEmpty(): boolean {
    return this.parsed.size === 0;
  }
}

/** Immutable language-native equivalent of Rust's `ToolResult` constructors. */
export class ToolResultValue implements ToolResultData {
  public readonly kind: ToolResultKind;
  public readonly output: string;
  public readonly success: boolean;
  public readonly truncated: boolean;
  public readonly artifact?: ToolResultData["artifact"];
  public readonly data?: WireValue | null;
  public readonly error?: string | null;
  public readonly failure?: ToolFailure | null;
  public readonly metadata?: Readonly<Record<string, string>>;
  public readonly mime_type?: string | null;
  public readonly model_content?: readonly ToolResultContent[];

  private constructor(state: ToolResultData) {
    this.kind = state.kind;
    this.output = state.output;
    this.success = state.success;
    this.truncated = state.truncated;
    this.artifact = state.artifact;
    this.data = state.data;
    this.error = state.error;
    this.failure = state.failure;
    this.metadata = state.metadata;
    this.mime_type = state.mime_type;
    this.model_content = state.model_content;
  }

  public static success(output: string): ToolResultValue {
    return ToolResultValue.create({ kind: { kind: "text" }, output, success: true });
  }

  public static successJson(data: JsonValue): ToolResultValue {
    return ToolResultValue.create({
      kind: { kind: "json" },
      output: JSON.stringify(data) ?? "",
      success: true,
      data: jsonToWireValue(data),
    });
  }

  public static successWithKind(kind: ToolResultKind, output: string): ToolResultValue {
    validateToolResultKind(kind);
    return ToolResultValue.create({ kind, output, success: true });
  }

  public static error(error: string): ToolResultValue {
    return ToolResultValue.create({
      kind: { kind: "structured_error", error_code: "tool_error" },
      output: "",
      success: false,
      error,
      failure: { category: "permanent", recovery: "stop", side_effect: "none" },
    });
  }

  public static invalidArguments(error: string): ToolResultValue {
    return ToolResultValue.error(error).withFailure({
      category: "invalid_arguments",
      recovery: "correct_arguments",
      side_effect: "none",
    });
  }

  public static failure(category: ToolFailureCategory, error: string): ToolResultValue {
    if (!FAILURE_CATEGORIES.has(category)) throw new TypeError(`unknown tool failure category: ${category}`);
    const recovery = category === "invalid_arguments"
      ? "correct_arguments"
      : category === "unavailable"
        ? "restore_then_retry"
        : category === "timeout" || category === "partial_side_effect"
          ? "verify_then_retry"
          : category === "transient"
            ? "retry"
            : "stop";
    const side_effect = category === "partial_side_effect" ? "possible" : "none";
    return ToolResultValue.error(error).withFailure({ category, recovery, side_effect });
  }

  public withOutput(output: string): ToolResultValue {
    return this.copy({ output });
  }

  public withError(error: string): ToolResultValue {
    return this.copy({
      success: false,
      error,
      failure: this.failure ?? { category: "permanent", recovery: "stop", side_effect: "none" },
    });
  }

  public withFailure(failure: ToolFailure): ToolResultValue {
    validateFailure(failure);
    return this.copy({ success: false, failure: { ...failure } });
  }

  public withData(data: JsonValue): ToolResultValue {
    return this.copy({ data: jsonToWireValue(data) });
  }

  public withTruncated(truncated: boolean): ToolResultValue {
    return this.copy({ truncated });
  }

  public withMimeType(mimeType: string): ToolResultValue {
    return this.copy({ mime_type: mimeType });
  }

  public withArtifact(artifact: NonNullable<ToolResultData["artifact"]>): ToolResultValue {
    return this.copy({ artifact });
  }

  public withMeta(key: string, value: string): ToolResultValue {
    return this.copy({ metadata: { ...(this.metadata ?? {}), [key]: value } });
  }

  public withMetadata(metadata: Readonly<Record<string, string>>): ToolResultValue {
    return this.copy({ metadata: { ...metadata } });
  }

  public withModelContent(content: ToolResultContent): ToolResultValue {
    return this.copy({ model_content: [...(this.model_content ?? []), { ...content }] });
  }

  public toJSON(): ToolResultData {
    return {
      kind: this.kind,
      output: this.output,
      success: this.success,
      truncated: this.truncated,
      artifact: this.artifact,
      data: this.data,
      error: this.error,
      failure: this.failure,
      metadata: this.metadata,
      mime_type: this.mime_type,
      model_content: this.model_content,
    };
  }

  private static create(state: Partial<ToolResultData>): ToolResultValue {
    return new ToolResultValue({
      kind: state.kind ?? { kind: "text" },
      output: state.output ?? "",
      success: state.success ?? false,
      truncated: state.truncated ?? false,
      artifact: state.artifact,
      data: state.data,
      error: state.error,
      failure: state.failure,
      metadata: state.metadata,
      mime_type: state.mime_type,
      model_content: state.model_content,
    });
  }

  private copy(changes: Partial<ToolResultData>): ToolResultValue {
    return ToolResultValue.create({
      ...this.toJSON(),
      ...changes,
    });
  }
}

// Keep the Rust type name available while the structural shape remains
// exported as `ToolResultData` for callback result unions.
export const ToolResult = ToolResultValue;

function isJsonObject(value: unknown): value is Record<string, JsonValue> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function paramTypeName(value: ParamValue): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (typeof value === "object") return "object";
  if (typeof value === "number") return "number";
  if (typeof value === "boolean") return "bool";
  return "string";
}

function validateFailure(failure: ToolFailure): void {
  if (failure === null || typeof failure !== "object") throw new TypeError("tool failure must be an object");
  if (!FAILURE_CATEGORIES.has(failure.category)) {
    throw new TypeError(`unknown tool failure category: ${failure.category}`);
  }
  if (!RECOVERY_ACTIONS.has(failure.recovery)) {
    throw new TypeError(`unknown tool recovery action: ${failure.recovery}`);
  }
  if (!SIDE_EFFECTS.has(failure.side_effect)) {
    throw new TypeError(`unknown tool side effect: ${failure.side_effect}`);
  }
  if (failure.retry_after_ms != null) wireU64(failure.retry_after_ms);
  for (const field of ["idempotency_key", "postcondition"] as const) {
    const value = failure[field];
    if (value != null && typeof value !== "string") {
      throw new TypeError(`tool failure ${field} must be text`);
    }
  }
}

function validateToolResultKind(kind: ToolResultKind): void {
  if (kind === null || typeof kind !== "object" || typeof kind.kind !== "string") {
    throw new TypeError("tool result kind must contain a textual discriminator");
  }
  switch (kind.kind) {
    case "text":
    case "json":
      return;
    case "image":
      if (typeof kind.mime_type !== "string") throw new TypeError("image result kind requires mime_type");
      return;
    case "table":
      if (!Array.isArray(kind.columns) || !Array.isArray(kind.rows)) throw new TypeError("table result kind requires columns and rows");
      return;
    case "diff":
      if (typeof kind.unified_diff !== "string") throw new TypeError("diff result kind requires unified_diff");
      return;
    case "file_reference":
      if (typeof kind.path !== "string") throw new TypeError("file_reference result kind requires path");
      return;
    case "command_output":
      if (kind.exit_code !== undefined && kind.exit_code !== null && !Number.isInteger(kind.exit_code)) {
        throw new TypeError("command_output result kind requires an integer exit_code");
      }
      return;
    case "skill_activation":
      if (typeof kind.name !== "string") throw new TypeError("skill_activation result kind requires name");
      return;
    case "structured_error":
      if (typeof kind.error_code !== "string") throw new TypeError("structured_error result kind requires error_code");
      return;
    default:
      throw new TypeError(`unknown tool result kind: ${(kind as { readonly kind: string }).kind}`);
  }
}

/** Encode ordinary JSON as a WireValue map, even when it has kind/value keys. */
function jsonToWireValue(value: JsonValue): WireValue {
  if (Array.isArray(value)) return { kind: "list", value: value.map(jsonToWireValue) };
  if (value !== null && typeof value === "object") {
    return {
      kind: "map",
      value: Object.entries(value).map(([key, entry]) => ({
        key: { kind: "string", value: key },
        value: jsonToWireValue(entry),
      })),
    };
  }
  return toWireValue(value);
}

function normalizeMaxChunkBytes(value: number): number {
  // Rust's usize input is integral; keep the TypeScript boundary total for
  // JavaScript callers while retaining its max(1) behavior for zero/negative
  // values.
  if (!Number.isFinite(value)) return 1;
  return Math.max(1, Math.trunc(value));
}

/**
 * Split UTF-8 text into chunks capped by encoded byte length.
 *
 * A chunk is never split in the middle of a Unicode scalar value. The limit
 * is clamped to one byte, matching the Rust helper's `max_chunk_bytes.max(1)`.
 */
export function splitUtf8Chunks(text: string, maxChunkBytes: number): string[] {
  if (typeof text !== "string") throw new TypeError("text must be a string");
  if (text.length === 0) return [];

  const maxBytes = normalizeMaxChunkBytes(maxChunkBytes);
  if (UTF8_ENCODER.encode(text).byteLength <= maxBytes) return [text];

  const chunks: string[] = [];
  let current = "";
  let currentBytes = 0;
  for (const character of text) {
    const characterBytes = UTF8_ENCODER.encode(character).byteLength;
    if (current.length > 0 && currentBytes + characterBytes > maxBytes) {
      chunks.push(current);
      current = "";
      currentBytes = 0;
    }
    current += character;
    currentBytes += characterBytes;
  }
  if (current.length > 0) chunks.push(current);
  return chunks;
}

/**
 * Stateful UTF-8 decoder for byte streams whose read boundaries may split a
 * multi-byte scalar value.
 *
 * `TextDecoder` with streaming enabled preserves incomplete suffixes between
 * pushes and replaces malformed sequences with U+FFFD. `finish` flushes an
 * incomplete suffix and resets the decoder for a subsequent stream.
 */
export class IncrementalUtf8Decoder {
  private readonly maxChunkBytes: number;
  private decoder: TextDecoder;

  public constructor(maxChunkBytes = DEFAULT_STREAM_CHUNK_BYTES) {
    this.maxChunkBytes = normalizeMaxChunkBytes(maxChunkBytes);
    // Rust's UTF-8 decoder preserves an initial BOM; TextDecoder strips it
    // unless `ignoreBOM` is enabled.
    this.decoder = new TextDecoder("utf-8", { fatal: false, ignoreBOM: true });
  }

  public push(bytes: Uint8Array): string[] {
    if (!(bytes instanceof Uint8Array)) throw new TypeError("bytes must be a Uint8Array");
    const output = this.decoder.decode(bytes, { stream: true });
    return splitUtf8Chunks(output, this.maxChunkBytes);
  }

  public finish(): string | undefined {
    const output = this.decoder.decode();
    return output.length === 0 ? undefined : output;
  }
}

/** Extract JSON from a fenced markdown block or bare text. */
export function extractJsonFromMarkdown(content: string): string {
  if (typeof content !== "string") throw new TypeError("content must be a string");
  const jsonStart = content.indexOf("```json");
  if (jsonStart >= 0) {
    const rest = content.slice(jsonStart + 7);
    const end = rest.indexOf("```");
    if (end >= 0) return trimRustWhitespace(rest.slice(0, end));
  }

  const blockStart = content.indexOf("```");
  if (blockStart >= 0) {
    const rest = content.slice(blockStart + 3);
    const end = rest.indexOf("```");
    if (end >= 0) return trimRustWhitespace(rest.slice(0, end));
  }
  return trimRustWhitespace(content);
}

/** Remove trailing commas before `}` or `]`, outside quoted strings. */
export function cleanJson(value: string): string {
  if (typeof value !== "string") throw new TypeError("value must be a string");
  const characters = Array.from(value);
  let cleaned = "";
  let inString = false;
  let escaped = false;

  for (let index = 0; index < characters.length; index += 1) {
    const character = characters[index];
    if (character === undefined) continue;

    if (inString) {
      cleaned += character;
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === '"') {
        inString = false;
      }
      continue;
    }

    if (character === '"') {
      inString = true;
      cleaned += character;
      continue;
    }

    if (character === ",") {
      let lookahead = index + 1;
      while (lookahead < characters.length && isWhitespace(characters[lookahead])) {
        lookahead += 1;
      }
      const next = characters[lookahead];
      if (next === "}" || next === "]") continue;
    }
    cleaned += character;
  }
  return cleaned;
}

function isWhitespace(character: string | undefined): boolean {
  const codePoint = character?.codePointAt(0);
  return codePoint !== undefined && (
    (codePoint >= 0x0009 && codePoint <= 0x000d)
    || codePoint === 0x0020
    || codePoint === 0x0085
    || codePoint === 0x00a0
    || codePoint === 0x1680
    || (codePoint >= 0x2000 && codePoint <= 0x200a)
    || codePoint === 0x2028
    || codePoint === 0x2029
    || codePoint === 0x202f
    || codePoint === 0x205f
    || codePoint === 0x3000
  );
}

function trimRustWhitespace(value: string): string {
  const characters = Array.from(value);
  let start = 0;
  let end = characters.length;
  while (start < end && isWhitespace(characters[start])) start += 1;
  while (end > start && isWhitespace(characters[end - 1])) end -= 1;
  return characters.slice(start, end).join("");
}
