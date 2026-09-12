/** Skill dependency/source values projected without probing or loading. */
export const DependencyKind = Object.freeze({
  Binary: "binary",
  PythonPkg: "python_pkg",
  NodeModule: "node_module",
} as const);

export type DependencyKind = (typeof DependencyKind)[keyof typeof DependencyKind];

export const SkillSource = Object.freeze({
  Local: "local",
  Mcp: "mcp",
} as const);

export type SkillSource = (typeof SkillSource)[keyof typeof SkillSource];
