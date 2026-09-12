/** Subagent command/status values projected without owning dispatch state. */
export const SubagentCommandPhase = Object.freeze({
  Persisted: "persisted",
  MailboxAccepted: "mailbox_accepted",
  Drained: "drained",
  TurnSettled: "turn_settled",
} as const);

export type SubagentCommandPhase =
  (typeof SubagentCommandPhase)[keyof typeof SubagentCommandPhase];

export function subagentCommandPhaseAsStr(phase: SubagentCommandPhase): string {
  if (!Object.values(SubagentCommandPhase).includes(phase)) {
    throw new TypeError("invalid subagent command phase");
  }
  return phase;
}

export function subagentCommandPhaseParse(value: string): SubagentCommandPhase | undefined {
  if (typeof value !== "string") return undefined;
  return Object.values(SubagentCommandPhase).find((phase) => phase === value);
}

export const SubagentStatus = Object.freeze({
  Running: "running",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
  TimedOut: "timed_out",
} as const);

export type SubagentStatus = (typeof SubagentStatus)[keyof typeof SubagentStatus];

export function subagentStatusAsStr(status: SubagentStatus): string {
  if (!Object.values(SubagentStatus).includes(status)) {
    throw new TypeError("invalid subagent status");
  }
  return status;
}

export function subagentStatusParse(value: string): SubagentStatus {
  if (typeof value !== "string") throw new TypeError("subagent status must be text");
  const status = Object.values(SubagentStatus).find((candidate) => candidate === value);
  if (status === undefined) throw new Error(`unknown Subagent status: ${value}`);
  return status;
}
