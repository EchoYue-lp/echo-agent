/** Hook event names and matcher semantics projected as local immutable values. */
export const HookEventCategory = Object.freeze({
  Tool: "Tool",
  Lifecycle: "Lifecycle",
  Subagent: "Subagent",
  Task: "Task",
  Error: "Error",
  Evolution: "Evolution",
} as const);

export type HookEventCategory = (typeof HookEventCategory)[keyof typeof HookEventCategory];

const EVENT_NAMES = [
  "PreToolUse",
  "PostToolUse",
  "PostToolUseFailure",
  "PermissionRequest",
  "PermissionDenied",
  "SessionStart",
  "SessionEnd",
  "Stop",
  "Notification",
  "UserPromptSubmit",
  "PreCompact",
  "PostCompact",
  "ConfigChange",
  "InstructionsLoaded",
  "PostToolBatch",
  "SubagentStart",
  "SubagentStop",
  "TaskCreated",
  "TaskStarted",
  "TaskCompleted",
  "StopFailure",
  "PluginLoaded",
  "PluginDisabled",
  "PostMemoryWrite",
  "MemoryLayerChange",
  "SkillCandidateDetected",
  "SkillLifecycleTransition",
  "SkillHealthCheck",
  "SkillPatchApplied",
  "SkillMergeApplied",
  "RulePromoted",
] as const;

export const HookEvent = Object.freeze({
  PreToolUse: "PreToolUse",
  PostToolUse: "PostToolUse",
  PostToolUseFailure: "PostToolUseFailure",
  PermissionRequest: "PermissionRequest",
  PermissionDenied: "PermissionDenied",
  SessionStart: "SessionStart",
  SessionEnd: "SessionEnd",
  Stop: "Stop",
  Notification: "Notification",
  UserPromptSubmit: "UserPromptSubmit",
  PreCompact: "PreCompact",
  PostCompact: "PostCompact",
  ConfigChange: "ConfigChange",
  InstructionsLoaded: "InstructionsLoaded",
  PostToolBatch: "PostToolBatch",
  SubagentStart: "SubagentStart",
  SubagentStop: "SubagentStop",
  TaskCreated: "TaskCreated",
  TaskStarted: "TaskStarted",
  TaskCompleted: "TaskCompleted",
  StopFailure: "StopFailure",
  PluginLoaded: "PluginLoaded",
  PluginDisabled: "PluginDisabled",
  PostMemoryWrite: "PostMemoryWrite",
  MemoryLayerChange: "MemoryLayerChange",
  SkillCandidateDetected: "SkillCandidateDetected",
  SkillLifecycleTransition: "SkillLifecycleTransition",
  SkillHealthCheck: "SkillHealthCheck",
  SkillPatchApplied: "SkillPatchApplied",
  SkillMergeApplied: "SkillMergeApplied",
  RulePromoted: "RulePromoted",
  ALL: EVENT_NAMES,
} as const);

export type HookEvent = (typeof HookEvent)[Exclude<keyof typeof HookEvent, "ALL">];

const TOOL_EVENTS: ReadonlySet<HookEvent> = new Set([
  HookEvent.PreToolUse,
  HookEvent.PostToolUse,
  HookEvent.PostToolUseFailure,
  HookEvent.PermissionRequest,
  HookEvent.PermissionDenied,
]);
const SUBAGENT_EVENTS: ReadonlySet<HookEvent> = new Set([
  HookEvent.SubagentStart,
  HookEvent.SubagentStop,
]);
const TASK_EVENTS: ReadonlySet<HookEvent> = new Set([
  HookEvent.TaskCreated,
  HookEvent.TaskStarted,
  HookEvent.TaskCompleted,
]);
const ERROR_EVENTS: ReadonlySet<HookEvent> = new Set([HookEvent.StopFailure]);
const EVOLUTION_EVENTS: ReadonlySet<HookEvent> = new Set([
  HookEvent.PostMemoryWrite,
  HookEvent.MemoryLayerChange,
  HookEvent.SkillCandidateDetected,
  HookEvent.SkillLifecycleTransition,
  HookEvent.SkillHealthCheck,
  HookEvent.SkillPatchApplied,
  HookEvent.SkillMergeApplied,
  HookEvent.RulePromoted,
]);

function assertEvent(event: string): asserts event is HookEvent {
  if (!(EVENT_NAMES as readonly string[]).includes(event)) {
    throw new TypeError(`invalid hook event: ${event}`);
  }
}

export function hookEventCategory(event: HookEvent): HookEventCategory {
  assertEvent(event);
  if (TOOL_EVENTS.has(event)) return HookEventCategory.Tool;
  if (SUBAGENT_EVENTS.has(event)) return HookEventCategory.Subagent;
  if (TASK_EVENTS.has(event)) return HookEventCategory.Task;
  if (ERROR_EVENTS.has(event)) return HookEventCategory.Error;
  if (EVOLUTION_EVENTS.has(event)) return HookEventCategory.Evolution;
  return HookEventCategory.Lifecycle;
}

export function hookEventAsStr(event: HookEvent): string {
  assertEvent(event);
  return event;
}

export function hookEventFromName(name: string): HookEvent | undefined {
  return (EVENT_NAMES as readonly string[]).includes(name) ? (name as HookEvent) : undefined;
}

export function hookEventIsToolEvent(event: HookEvent): boolean {
  return TOOL_EVENTS.has(event);
}

export function hookEventSupportsMatcher(event: HookEvent): boolean {
  assertEvent(event);
  return true;
}
