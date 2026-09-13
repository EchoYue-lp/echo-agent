import test from "node:test";
import assert from "node:assert/strict";
import {
  MemoryScope,
  memoryScopeAll,
  memoryScopeIsPersistent,
  memoryScopeName,
  memoryScopePriority,
  parseMemoryScope,
} from "../dist/index.js";

test("memory scopes preserve Rust order, names and persistence", () => {
  assert.deepEqual(memoryScopeAll(), [
    MemoryScope.User,
    MemoryScope.Project,
    MemoryScope.Repo,
    MemoryScope.Task,
    MemoryScope.Session,
    MemoryScope.Run,
  ]);
  assert.equal(memoryScopeName(MemoryScope.Project), "project");
  assert.equal(memoryScopePriority(MemoryScope.Run), 5);
  assert.equal(memoryScopeIsPersistent(MemoryScope.User), true);
  assert.equal(memoryScopeIsPersistent(MemoryScope.Task), false);
});

test("memory scope parser preserves aliases and rejects unknown values", () => {
  assert.equal(parseMemoryScope(" proj "), MemoryScope.Project);
  assert.equal(parseMemoryScope("SESS"), MemoryScope.Session);
  assert.equal(parseMemoryScope("unknown"), null);
  assert.equal(parseMemoryScope(42), null);
});
