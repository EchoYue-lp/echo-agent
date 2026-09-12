/** Terminal Subagent hook status values projected without owning hooks. */
export const SubagentStopStatus = Object.freeze({
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
  TimedOut: "timed_out",
} as const);

export type SubagentStopStatus =
  (typeof SubagentStopStatus)[keyof typeof SubagentStopStatus];

export function subagentStopStatusAsStr(status: SubagentStopStatus): string {
  if (!Object.values(SubagentStopStatus).includes(status)) {
    throw new TypeError("invalid Subagent stop status");
  }
  return status;
}
