/** Permission matcher values and pure matching helpers. */
import type { ToolPermission } from "./types.js";

export const RuleMatcherPermission = Object.freeze({
  Read: "read",
  Write: "write",
  Network: "network",
  Execute: "execute",
  Sensitive: "sensitive",
} as const);

export type RuleMatcher =
  | { readonly kind: "tool"; readonly name: string }
  | { readonly kind: "pattern"; readonly pattern: string }
  | { readonly kind: "permission"; readonly permission: ToolPermission }
  | { readonly kind: "all" };

export function ruleMatcherParse(value: string): RuleMatcher {
  if (typeof value !== "string") throw new TypeError("permission matcher must be text");
  if (value === "*" || value === "all") return Object.freeze({ kind: "all" });
  if (value.startsWith("tool:")) {
    const name = value.slice(5);
    if (!name) throw new Error("tool permission matcher requires a name");
    return Object.freeze({ kind: "tool", name });
  }
  if (value.startsWith("pattern:")) {
    const pattern = value.slice(8);
    if (!pattern) throw new Error("pattern permission matcher cannot be empty");
    return Object.freeze({ kind: "pattern", pattern });
  }
  const flag = value.startsWith("perm:") ? value.slice(5) : value.startsWith("permission:") ? value.slice(11) : undefined;
  if (flag !== undefined && Object.values(RuleMatcherPermission).includes(flag as ToolPermission)) {
    return Object.freeze({ kind: "permission", permission: flag as ToolPermission });
  }
  if (flag !== undefined) throw new Error(`unknown permission matcher: ${flag}`);
  throw new Error(`unsupported permission matcher: ${value}`);
}

export function ruleMatcherDisplay(matcher: RuleMatcher): string {
  switch (matcher.kind) {
    case "tool": return `tool:${matcher.name}`;
    case "pattern": return `pattern:${matcher.pattern}`;
    case "permission": return `permission:${matcher.permission}`;
    case "all": return "all";
  }
}

export function ruleMatcherMatchesMatcherStr(matcher: RuleMatcher, value: string): boolean {
  if (matcher.kind === "tool") return matcher.name === value;
  if (matcher.kind === "pattern") return matcher.pattern === value;
  return matcher.kind === "all" && (value === "*" || value === "all");
}

export function ruleMatcherMatches(matcher: RuleMatcher, toolName: string, permissions: readonly ToolPermission[]): boolean {
  switch (matcher.kind) {
    case "tool": return toolName === matcher.name;
    case "permission": return permissions.includes(matcher.permission);
    case "all": return true;
    case "pattern":
      if (toolName === matcher.pattern) return true;
      if (globMatches(matcher.pattern, toolName)) return true;
      if (matcher.pattern.endsWith("*)") && toolName.startsWith(matcher.pattern.slice(0, -2))) return true;
      return toolName.startsWith(matcher.pattern)
        && toolName.length > matcher.pattern.length
        && toolName[matcher.pattern.length] === "(";
  }
}

function globMatches(pattern: string, value: string): boolean {
  let p = 0;
  let v = 0;
  let star = -1;
  let mark = -1;
  while (v < value.length) {
    if (p < pattern.length && (pattern[p] === "?" || pattern[p] === value[v])) {
      p += 1;
      v += 1;
    } else if (p < pattern.length && pattern[p] === "*") {
      star = p++;
      mark = v;
    } else if (star !== -1) {
      p = star + 1;
      v = ++mark;
    } else {
      return false;
    }
  }
  while (p < pattern.length && pattern[p] === "*") p += 1;
  return p === pattern.length;
}
