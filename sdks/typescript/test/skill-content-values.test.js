import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SkillContent } from "../dist/index.js";

test("skill content preserves the structured prompt block", () => {
  const content = new SkillContent("demo", "/tmp/demo", "  Do the thing.  ", ["read"], [{ kind: "script", relativePath: "scripts/run.sh" }]);
  const block = content.toPromptBlock();
  assert.match(block, /<skill_content name="demo">/);
  assert.match(block, /<allowed_tools>/);
  assert.match(block, /<file kind="script">scripts\/run\.sh<\/file>/);
});

test("skill content mappings are complete", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../../../contracts/sdk/parity-manifest.json", import.meta.url), "utf8"),
  );
  const entries = manifest.entries.filter(
    (entry) => entry.canonical && entry.languages.typescript.contract_test.endsWith("/skill_content_values"),
  );
  assert.equal(entries.length, 2);
  for (const entry of entries) assert.equal(entry.languages.typescript.status, "done", entry.path);
});
