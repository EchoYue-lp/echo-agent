/** Team collaboration strategies projected without owning Team execution. */
export type TeamStrategy =
  | { readonly kind: "manager_subagent" }
  | { readonly kind: "pipeline"; readonly members: readonly string[] }
  | { readonly kind: "debate"; readonly judge: string; readonly debaters: readonly string[] }
  | { readonly kind: "swarm"; readonly reducer: string };

export function teamStrategyManager(): TeamStrategy {
  return Object.freeze({ kind: "manager_subagent" });
}

export function teamStrategyPipeline(members: readonly string[]): TeamStrategy {
  return Object.freeze({ kind: "pipeline", members: Object.freeze([...members]) });
}

export function teamStrategyDebate(judge: string, debaters: readonly string[]): TeamStrategy {
  return Object.freeze({ kind: "debate", judge, debaters: Object.freeze([...debaters]) });
}

export function teamStrategySwarm(reducer: string): TeamStrategy {
  return Object.freeze({ kind: "swarm", reducer });
}

export function teamStrategyName(strategy: TeamStrategy): string {
  return strategy.kind;
}

export function teamStrategyDescription(strategy: TeamStrategy): string {
  switch (strategy.kind) {
    case "manager_subagent": return "Manager plans typed tasks, Subagents execute them, and the manager synthesizes";
    case "pipeline": return "Subagents execute in sequence";
    case "debate": return "Debaters propose independently and a judge synthesizes";
    case "swarm": return "Subagents inspect independently and a reducer synthesizes";
  }
}
