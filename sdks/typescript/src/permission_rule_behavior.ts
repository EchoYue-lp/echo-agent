/** Permission rule behavior values projected without owning evaluation. */
export type RuleBehavior =
  | { readonly kind: "allow" }
  | { readonly kind: "deny"; readonly reason: string }
  | { readonly kind: "ask"; readonly suggestions: readonly string[] };

export type RuleDecision =
  | { readonly kind: "allow" }
  | { readonly kind: "deny"; readonly reason: string }
  | { readonly kind: "ask"; readonly suggestions: readonly string[] };

export function ruleBehaviorAllow(): RuleBehavior {
  return Object.freeze({ kind: "allow" });
}

export function ruleBehaviorDeny(reason: string): RuleBehavior {
  if (typeof reason !== "string") throw new TypeError("deny reason must be text");
  return Object.freeze({ kind: "deny", reason });
}

export function ruleBehaviorAsk(suggestions: readonly string[]): RuleBehavior {
  return Object.freeze({ kind: "ask", suggestions: Object.freeze([...suggestions]) });
}

export function ruleBehaviorParse(value: string): RuleBehavior {
  if (typeof value !== "string") throw new TypeError("rule behavior must be text");
  switch (value) {
    case "allow": return ruleBehaviorAllow();
    case "deny": return ruleBehaviorDeny("denied by rule");
    case "ask": return ruleBehaviorAsk(["allow", "deny"]);
    default: throw new Error(`unknown permission rule behavior: ${value}`);
  }
}

export function ruleBehaviorToDecision(behavior: RuleBehavior): RuleDecision {
  if (behavior.kind === "allow") return Object.freeze({ kind: "allow" });
  if (behavior.kind === "deny") return Object.freeze({ kind: "deny", reason: behavior.reason });
  return Object.freeze({ kind: "ask", suggestions: Object.freeze([...behavior.suggestions]) });
}
