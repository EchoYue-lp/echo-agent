/** Command-cell terminal/artifact status values projected without ownership. */
export const CommandCellTerminalCause = Object.freeze({
  Exited: "exited",
  TimedOut: "timed_out",
  Cancelled: "cancelled",
  LaunchFailed: "launch_failed",
  WaitFailed: "wait_failed",
  OutputDrainFailed: "output_drain_failed",
} as const);

export type CommandCellTerminalCause =
  (typeof CommandCellTerminalCause)[keyof typeof CommandCellTerminalCause];

export function commandCellTerminalCauseAsStr(cause: CommandCellTerminalCause): string {
  if (!Object.values(CommandCellTerminalCause).includes(cause)) {
    throw new TypeError("invalid command cell terminal cause");
  }
  return cause;
}

export const CommandCellArtifactStatus = Object.freeze({
  NotRequested: "not_requested",
  Writing: "writing",
  BelowThreshold: "below_threshold",
  Available: "available",
  Failed: "failed",
} as const);

export type CommandCellArtifactStatus =
  (typeof CommandCellArtifactStatus)[keyof typeof CommandCellArtifactStatus];

export function commandCellArtifactStatusAsStr(status: CommandCellArtifactStatus): string {
  if (!Object.values(CommandCellArtifactStatus).includes(status)) {
    throw new TypeError("invalid command cell artifact status");
  }
  return status;
}
