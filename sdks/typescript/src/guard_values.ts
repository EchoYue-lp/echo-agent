/** Guard decisions projected without owning guard execution. */
export type GuardDecision =
  | { readonly kind: "pass" }
  | { readonly kind: "block"; readonly reason: string }
  | { readonly kind: "warn"; readonly reasons: readonly string[] }
  | { readonly kind: "transform"; readonly content: string; readonly reasons: readonly string[] };

export function guardDecisionPass(): GuardDecision {
  return Object.freeze({ kind: "pass" });
}

export function guardDecisionBlock(reason: string): GuardDecision {
  if (typeof reason !== "string") throw new TypeError("block reason must be text");
  return Object.freeze({ kind: "block", reason });
}

export function guardDecisionWarn(reasons: readonly string[]): GuardDecision {
  return Object.freeze({ kind: "warn", reasons: Object.freeze([...reasons]) });
}

export function guardDecisionTransform(
  content: string,
  reasons: readonly string[],
): GuardDecision {
  if (typeof content !== "string") throw new TypeError("transformed content must be text");
  return Object.freeze({ kind: "transform", content, reasons: Object.freeze([...reasons]) });
}

export function guardDecisionIsBlocked(decision: GuardDecision): boolean {
  return decision.kind === "block";
}
