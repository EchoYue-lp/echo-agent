const DEFAULT_STREAM_CHUNK_BYTES = 16 * 1024;
const UTF8_ENCODER = new TextEncoder();

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
