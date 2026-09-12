import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { PageInfo, ToolResultValue } from "../dist/index.js";

test("page info applies metadata, truncation and continuation", () => {
  const result = new PageInfo("cursor-2", true, true, 4n, 2n)
    .applyTo(ToolResultValue.success("items"));
  assert.equal(result.truncated, true);
  assert.equal(result.metadata?.["page.next_cursor"], "cursor-2");
  assert.match(result.output, /\[page\]/);
});

test("page info mappings are complete", () => {
  const manifest = JSON.parse(readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"));
  const entries = manifest.entries.filter((entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/page_info_values"));
  assert.equal(entries.length, 2);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
