import test from "node:test";
import assert from "node:assert/strict";
import {
  cleanJson,
  extractJsonFromMarkdown,
  IncrementalUtf8Decoder,
  splitUtf8Chunks,
} from "../dist/index.js";

test("splitUtf8Chunks caps encoded bytes without splitting scalar values", () => {
  assert.deepEqual(splitUtf8Chunks("中文ab", 5), ["中", "文ab"]);
  assert.deepEqual(splitUtf8Chunks("🙂🙂", 4), ["🙂", "🙂"]);
  assert.deepEqual(splitUtf8Chunks("中文", 0), ["中", "文"]);
});

test("IncrementalUtf8Decoder preserves scalars split across pushes", () => {
  const decoder = new IncrementalUtf8Decoder(16);
  assert.deepEqual(decoder.push(new Uint8Array([0xe4, 0xb8])), []);
  assert.deepEqual(decoder.push(new Uint8Array([0xad])), ["中"]);
  assert.equal(decoder.finish(), undefined);
});

test("IncrementalUtf8Decoder replaces malformed bytes and flushes suffixes", () => {
  const decoder = new IncrementalUtf8Decoder(16);
  assert.deepEqual(decoder.push(new Uint8Array([0x61, 0xff])), ["a\ufffd"]);
  assert.deepEqual(decoder.push(new Uint8Array([0xf0, 0x9f])), []);
  assert.equal(decoder.finish(), "\ufffd");
  assert.equal(decoder.finish(), undefined);
});

test("IncrementalUtf8Decoder preserves a UTF-8 BOM like Rust", () => {
  const decoder = new IncrementalUtf8Decoder();
  assert.deepEqual(decoder.push(new Uint8Array([0xef, 0xbb, 0xbf, 0x61])), ["\ufeffa"]);
  assert.equal(decoder.finish(), undefined);
});

test("extractJsonFromMarkdown handles fenced and bare JSON", () => {
  assert.equal(
    extractJsonFromMarkdown("Here:\n```json\n{\"key\": \"value\"}\n```\nDone."),
    '{"key": "value"}',
  );
  assert.equal(extractJsonFromMarkdown("```\n{\"key\": \"value\"}\n```"), '{"key": "value"}');
  assert.equal(extractJsonFromMarkdown("  {\"key\": true}  "), '{"key": true}');
  assert.equal(extractJsonFromMarkdown("\u0085{\"key\": true}\u0085"), '{"key": true}');
  assert.equal(extractJsonFromMarkdown("\ufeff{\"key\": true}\ufeff"), '\ufeff{"key": true}\ufeff');
});

test("cleanJson removes structural trailing commas only", () => {
  assert.equal(cleanJson('{"a": 1,}'), '{"a": 1}');
  assert.equal(cleanJson('[1, 2,]'), '[1, 2]');
  assert.equal(cleanJson('{"text":"keep ,} and ,] literal","escaped":"\\\\\\\" ,}",}'),
    '{"text":"keep ,} and ,] literal","escaped":"\\\\\\\" ,}"}',
  );
  assert.equal(cleanJson('{"中文": 1, \n}'), '{"中文": 1 \n}');
  assert.equal(cleanJson('{"a": 1,\u0085}'), '{"a": 1\u0085}');
  assert.equal(cleanJson('{"a": 1,\ufeff}'), '{"a": 1,\ufeff}');
  assert.equal(cleanJson("{'a': 'don\\'t'}"), "{'a': 'don\\'t'}");
});

test("text helpers reject non-text and non-byte inputs", () => {
  assert.throws(() => cleanJson(null), TypeError);
  assert.throws(() => extractJsonFromMarkdown(1), TypeError);
  assert.throws(() => splitUtf8Chunks(true, 1), TypeError);
  assert.throws(() => new IncrementalUtf8Decoder().push("abc"), TypeError);
});
