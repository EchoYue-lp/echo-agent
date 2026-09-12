/** Subagent context inheritance values projected without owning context state. */
export type ContextInheritanceMode = "sync" | "fork" | "teammate" | "team";

export interface ContextInheritance {
  readonly inheritTools: readonly string[] | null;
  readonly inheritHistory: bigint | null;
  readonly inheritMemory: boolean;
  readonly injectMetadata: Readonly<Record<string, string>>;
}

export function contextInheritanceSyncDefault(): ContextInheritance {
  return freezeContext(null, null, false, {});
}

export function contextInheritanceFreshDefault(): ContextInheritance {
  return contextInheritanceSyncDefault();
}

export function contextInheritanceForkDefault(): ContextInheritance {
  return freezeContext(null, 2n, true, {});
}

export function contextInheritanceTeammateDefault(): ContextInheritance {
  return freezeContext([], 2n, false, {});
}

export function contextInheritanceForMode(mode: ContextInheritanceMode): ContextInheritance {
  switch (mode) {
    case "sync": return contextInheritanceSyncDefault();
    case "fork": return contextInheritanceForkDefault();
    case "teammate":
    case "team": return contextInheritanceTeammateDefault();
  }
}

function freezeContext(
  inheritTools: readonly string[] | null,
  inheritHistory: bigint | null,
  inheritMemory: boolean,
  injectMetadata: Readonly<Record<string, string>>,
): ContextInheritance {
  if (inheritHistory !== null && (inheritHistory < 0n || typeof inheritHistory !== "bigint")) {
    throw new RangeError("inherit history must be a non-negative bigint");
  }
  return Object.freeze({
    inheritTools: inheritTools === null ? null : Object.freeze([...inheritTools]),
    inheritHistory,
    inheritMemory,
    injectMetadata: Object.freeze({ ...injectMetadata }),
  });
}
