import test from "node:test";
import assert from "node:assert/strict";
import {
  MemorySource,
  MemoryType,
  memorySourceDefaultConfidence,
  memorySourceDefaultRecallWeight,
  memoryTypeDefaultStability,
  memoryTypeIsRuleEligible,
  memoryTypeIsSkillEligible,
} from "../dist/index.js";

test("memory type policies preserve Rust stability and promotion eligibility", () => {
  assert.equal(memoryTypeDefaultStability(MemoryType.UserPreference), 0.85);
  assert.equal(memoryTypeDefaultStability(MemoryType.DeprecatedNote), 0.1);
  assert.equal(memoryTypeIsRuleEligible(MemoryType.ProjectFact), true);
  assert.equal(memoryTypeIsRuleEligible(MemoryType.WorkflowPattern), false);
  assert.equal(memoryTypeIsSkillEligible(MemoryType.DebuggingLesson), true);
  assert.equal(memoryTypeIsSkillEligible(MemoryType.UserPreference), false);
});

test("memory source policies preserve confidence and recall weights", () => {
  assert.equal(memorySourceDefaultConfidence(MemorySource.ExplicitSave), 1);
  assert.equal(memorySourceDefaultConfidence(MemorySource.AutoExtracted), 0.6);
  assert.equal(memorySourceDefaultRecallWeight(MemorySource.UserCorrection), 0.9);
  assert.equal(memorySourceDefaultRecallWeight(MemorySource.L3Promotion), 0.4);
});
