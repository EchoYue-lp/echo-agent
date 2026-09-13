/** Typed memory classifications and source policies projected from Rust. */
export const MemoryType = Object.freeze({
  UserPreference: "user_preference",
  ProjectFact: "project_fact",
  ArchitectureDecision: "architecture_decision",
  DebuggingLesson: "debugging_lesson",
  ErrorResolution: "error_resolution",
  CommandPattern: "command_pattern",
  ToolUsage: "tool_usage",
  WorkflowPattern: "workflow_pattern",
  SkillCandidate: "skill_candidate",
  DeprecatedNote: "deprecated_note",
} as const);

export type MemoryType = (typeof MemoryType)[keyof typeof MemoryType];

export const MemorySource = Object.freeze({
  UserCorrection: "user_correction",
  ErrorResolution: "error_resolution",
  RepeatedWorkflow: "repeated_workflow",
  ExplicitSave: "explicit_save",
  AutoExtracted: "auto_extracted",
  L3Promotion: "l3_promotion",
} as const);

export type MemorySource = (typeof MemorySource)[keyof typeof MemorySource];

const TYPE_STABILITY: Readonly<Record<MemoryType, number>> = Object.freeze({
  user_preference: 0.85,
  project_fact: 0.6,
  architecture_decision: 0.8,
  debugging_lesson: 0.55,
  error_resolution: 0.5,
  command_pattern: 0.4,
  tool_usage: 0.5,
  workflow_pattern: 0.6,
  skill_candidate: 0.55,
  deprecated_note: 0.1,
});
const SOURCE_CONFIDENCE: Readonly<Record<MemorySource, number>> = Object.freeze({
  user_correction: 0.9,
  error_resolution: 0.85,
  repeated_workflow: 0.75,
  explicit_save: 1,
  auto_extracted: 0.6,
  l3_promotion: 0.5,
});
const SOURCE_RECALL_WEIGHT: Readonly<Record<MemorySource, number>> = Object.freeze({
  user_correction: 0.9,
  error_resolution: 0.7,
  repeated_workflow: 0.6,
  explicit_save: 0.8,
  auto_extracted: 0.4,
  l3_promotion: 0.4,
});
const MEMORY_TYPES: ReadonlySet<string> = new Set(Object.values(MemoryType));
const MEMORY_SOURCES: ReadonlySet<string> = new Set(Object.values(MemorySource));

export function memoryTypeDefaultStability(memoryType: MemoryType): number {
  validate(memoryType, MEMORY_TYPES, "memory type");
  return TYPE_STABILITY[memoryType];
}

export function memoryTypeIsSkillEligible(memoryType: MemoryType): boolean {
  validate(memoryType, MEMORY_TYPES, "memory type");
  return memoryType === MemoryType.WorkflowPattern
    || memoryType === MemoryType.SkillCandidate
    || memoryType === MemoryType.DebuggingLesson;
}

export function memoryTypeIsRuleEligible(memoryType: MemoryType): boolean {
  validate(memoryType, MEMORY_TYPES, "memory type");
  return memoryType === MemoryType.UserPreference
    || memoryType === MemoryType.ArchitectureDecision
    || memoryType === MemoryType.ProjectFact;
}

export function memorySourceDefaultConfidence(source: MemorySource): number {
  validate(source, MEMORY_SOURCES, "memory source");
  return SOURCE_CONFIDENCE[source];
}

export function memorySourceDefaultRecallWeight(source: MemorySource): number {
  validate(source, MEMORY_SOURCES, "memory source");
  return SOURCE_RECALL_WEIGHT[source];
}

function validate(value: unknown, values: ReadonlySet<string>, name: string): asserts value is string {
  if (typeof value !== "string" || !values.has(value)) throw new TypeError(`unknown ${name}: ${String(value)}`);
}
