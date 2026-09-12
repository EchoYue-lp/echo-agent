/** Permission mode identifiers and policy helpers projected without evaluation. */
export const PermissionMode = Object.freeze({
  Default: "default",
  Plan: "plan",
  AcceptEdits: "auto-edit",
  BypassPermissions: "full-auto",
  Auto: "auto",
  Bubble: "bubble",
  DontAsk: "dont-ask",
  StrictConfirm: "strict",
} as const);

export type PermissionMode = (typeof PermissionMode)[keyof typeof PermissionMode];

export function permissionModeParse(value: string): PermissionMode {
  if (typeof value !== "string") throw new TypeError("permission mode must be text");
  const aliases: Readonly<Record<string, PermissionMode>> = {
    default: PermissionMode.Default,
    ask: PermissionMode.Default,
    plan: PermissionMode.Plan,
    "auto-edit": PermissionMode.AcceptEdits,
    autoedit: PermissionMode.AcceptEdits,
    "accept-edits": PermissionMode.AcceptEdits,
    acceptedits: PermissionMode.AcceptEdits,
    "full-auto": PermissionMode.BypassPermissions,
    fullauto: PermissionMode.BypassPermissions,
    bypass: PermissionMode.BypassPermissions,
    "bypass-permissions": PermissionMode.BypassPermissions,
    bypasspermissions: PermissionMode.BypassPermissions,
    auto: PermissionMode.Auto,
    bubble: PermissionMode.Bubble,
    "dont-ask": PermissionMode.DontAsk,
    dontask: PermissionMode.DontAsk,
    strict: PermissionMode.StrictConfirm,
    "strict-confirm": PermissionMode.StrictConfirm,
    "strict-confirmation": PermissionMode.StrictConfirm,
  };
  const mode = aliases[value.trim().toLowerCase()];
  if (mode === undefined) throw new Error(`invalid permission mode '${value}'`);
  return mode;
}

export function permissionModeAllowsWrite(mode: PermissionMode): boolean {
  return mode === PermissionMode.BypassPermissions || mode === PermissionMode.AcceptEdits;
}

export function permissionModeRequiresInteraction(mode: PermissionMode): boolean {
  return mode !== PermissionMode.BypassPermissions
    && mode !== PermissionMode.Auto
    && mode !== PermissionMode.DontAsk
    && mode !== PermissionMode.AcceptEdits;
}

export function permissionModeUsesClassifier(mode: PermissionMode): boolean {
  return mode === PermissionMode.Auto;
}
