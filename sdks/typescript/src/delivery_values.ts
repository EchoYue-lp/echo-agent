/** Durable delivery outcome/phase values projected without owning the ledger. */
export const DeliveryOutcome = Object.freeze({
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
  Dropped: "dropped",
  OutcomeUnknown: "outcome_unknown",
} as const);

export type DeliveryOutcome = (typeof DeliveryOutcome)[keyof typeof DeliveryOutcome];

export function deliveryOutcomeAsStr(outcome: DeliveryOutcome): string {
  if (!Object.values(DeliveryOutcome).includes(outcome)) {
    throw new TypeError("invalid delivery outcome");
  }
  return outcome;
}

export const DeliveryPhase = Object.freeze({
  Persisted: "persisted",
  Claimed: "claimed",
  EffectStarted: "effect_started",
  MailboxAccepted: "mailbox_accepted",
  Drained: "drained",
  Deferred: "deferred",
  TurnSettled: "turn_settled",
} as const);

export type DeliveryPhase = (typeof DeliveryPhase)[keyof typeof DeliveryPhase];

export function deliveryPhaseAsStr(phase: DeliveryPhase): string {
  if (!Object.values(DeliveryPhase).includes(phase)) {
    throw new TypeError("invalid delivery phase");
  }
  return phase;
}
