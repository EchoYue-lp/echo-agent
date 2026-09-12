/** Steering lifecycle values projected without owning an Agent turn. */
export const AgentSteerPhase = Object.freeze({
  Accepted: "accepted",
  Drained: "drained",
  TurnSettled: "turn_settled",
} as const);

export type AgentSteerPhase = (typeof AgentSteerPhase)[keyof typeof AgentSteerPhase];

export const AgentSteerTurnOutcome = Object.freeze({
  Completed: "completed",
  Cancelled: "cancelled",
  Failed: "failed",
  Dropped: "dropped",
} as const);

export type AgentSteerTurnOutcome =
  (typeof AgentSteerTurnOutcome)[keyof typeof AgentSteerTurnOutcome];

export type AgentSteerState =
  | { readonly kind: "accepted" }
  | { readonly kind: "drained" }
  | {
      readonly kind: "turn_settled";
      readonly outcome: AgentSteerTurnOutcome;
      readonly drained: boolean;
    };

export function agentSteerStateAccepted(): AgentSteerState {
  return Object.freeze({ kind: AgentSteerPhase.Accepted });
}

export function agentSteerStateDrained(): AgentSteerState {
  return Object.freeze({ kind: AgentSteerPhase.Drained });
}

export function agentSteerStateTurnSettled(
  outcome: AgentSteerTurnOutcome,
  drained: boolean,
): AgentSteerState {
  if (!Object.values(AgentSteerTurnOutcome).includes(outcome)) {
    throw new TypeError("invalid steering turn outcome");
  }
  if (typeof drained !== "boolean") throw new TypeError("drained must be boolean");
  return Object.freeze({ kind: AgentSteerPhase.TurnSettled, outcome, drained });
}

export function agentSteerStatePhase(state: AgentSteerState): AgentSteerPhase {
  return state.kind;
}

export function agentSteerStateWasDrained(state: AgentSteerState): boolean {
  return state.kind === AgentSteerPhase.Drained
    || (state.kind === AgentSteerPhase.TurnSettled && state.drained);
}

export function agentSteerTurnOutcomeAsStr(outcome: AgentSteerTurnOutcome): string {
  if (!Object.values(AgentSteerTurnOutcome).includes(outcome)) {
    throw new TypeError("invalid steering turn outcome");
  }
  return outcome;
}

export function agentSteerTurnOutcomeParse(value: string): AgentSteerTurnOutcome | undefined {
  if (typeof value !== "string") return undefined;
  return Object.values(AgentSteerTurnOutcome).find((outcome) => outcome === value);
}
