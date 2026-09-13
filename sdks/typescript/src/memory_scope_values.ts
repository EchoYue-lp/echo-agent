/** Memory lifetime scopes with Rust-compatible wire names and priority. */
export const MemoryScope = Object.freeze({
  User: "user",
  Project: "project",
  Repo: "repo",
  Task: "task",
  Session: "session",
  Run: "run",
} as const);

export type MemoryScope = (typeof MemoryScope)[keyof typeof MemoryScope];

const MEMORY_SCOPE_ALL: readonly MemoryScope[] = Object.freeze([
  MemoryScope.User,
  MemoryScope.Project,
  MemoryScope.Repo,
  MemoryScope.Task,
  MemoryScope.Session,
  MemoryScope.Run,
]);
const MEMORY_SCOPES: ReadonlySet<string> = new Set(MEMORY_SCOPE_ALL);

export function memoryScopeAll(): readonly MemoryScope[] {
  return MEMORY_SCOPE_ALL;
}

export function memoryScopeName(scope: MemoryScope): string {
  validateMemoryScope(scope);
  return scope;
}

export function memoryScopePriority(scope: MemoryScope): number {
  validateMemoryScope(scope);
  return MEMORY_SCOPE_ALL.indexOf(scope);
}

export function memoryScopeIsPersistent(scope: MemoryScope): boolean {
  validateMemoryScope(scope);
  return scope === MemoryScope.User || scope === MemoryScope.Project || scope === MemoryScope.Repo;
}

export function parseMemoryScope(value: string): MemoryScope | null {
  if (typeof value !== "string") return null;
  switch (value.trim().toLowerCase()) {
    case "user": return MemoryScope.User;
    case "project": case "proj": return MemoryScope.Project;
    case "repo": return MemoryScope.Repo;
    case "task": return MemoryScope.Task;
    case "session": case "sess": return MemoryScope.Session;
    case "run": return MemoryScope.Run;
    default: return null;
  }
}

function validateMemoryScope(scope: unknown): asserts scope is MemoryScope {
  if (typeof scope !== "string" || !MEMORY_SCOPES.has(scope)) {
    throw new TypeError(`unknown memory scope: ${String(scope)}`);
  }
}
