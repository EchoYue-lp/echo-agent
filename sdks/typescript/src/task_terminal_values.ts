/** Task terminal status values projected without owning task execution. */
export const TaskTerminalStatus = Object.freeze({
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
  TimedOut: "timed_out",
  Skipped: "skipped",
} as const);

export type TaskTerminalStatus =
  (typeof TaskTerminalStatus)[keyof typeof TaskTerminalStatus];

export function taskTerminalStatusAsStr(status: TaskTerminalStatus): string {
  if (!Object.values(TaskTerminalStatus).includes(status)) {
    throw new TypeError("invalid task terminal status");
  }
  return status;
}
