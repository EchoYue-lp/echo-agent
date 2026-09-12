/** Hook action configuration values without executing hooks. */
export type HookAction =
  | { readonly type: "command"; readonly command: string; readonly shell?: string | null; readonly timeout: bigint }
  | { readonly type: "prompt"; readonly prompt: string }
  | { readonly type: "permission"; readonly decision: string; readonly reason?: string | null; readonly suggestions: readonly string[] }
  | { readonly type: "http"; readonly url: string; readonly method?: string | null; readonly headers?: Readonly<Record<string, string>> | null; readonly timeout: bigint }
  | { readonly type: "mcp_tool"; readonly server: string; readonly tool: string; readonly arguments?: unknown | null; readonly timeout: bigint }
  | { readonly type: "subagent"; readonly name: string; readonly task?: string | null; readonly timeout: bigint }
  | { readonly type: "activate_skill"; readonly skill: string; readonly reason: string };

const freeze = <T extends HookAction>(action: T): T => Object.freeze(action);

export const hookCommand = (command: string, shell: string | null = null, timeout = 300n): HookAction =>
  freeze({ type: "command", command, shell, timeout });
export const hookPrompt = (prompt: string): HookAction => freeze({ type: "prompt", prompt });
export const hookPermission = (decision: string, reason: string | null = null, suggestions: readonly string[] = []): HookAction =>
  freeze({ type: "permission", decision, reason, suggestions: Object.freeze([...suggestions]) });
export const hookHttp = (url: string, method: string | null = null, headers: Readonly<Record<string, string>> | null = null, timeout = 300n): HookAction =>
  freeze({ type: "http", url, method, headers: headers ? Object.freeze({ ...headers }) : null, timeout });
export const hookMcpTool = (server: string, tool: string, args: unknown | null = null, timeout = 300n): HookAction =>
  freeze({ type: "mcp_tool", server, tool, arguments: args, timeout });
export const hookSubagent = (name: string, task: string | null = null, timeout = 300n): HookAction =>
  freeze({ type: "subagent", name, task, timeout });
export const hookActivateSkill = (skill: string, reason = ""): HookAction => freeze({ type: "activate_skill", skill, reason });

export function hookActionKind(action: HookAction): HookAction["type"] { return action.type; }

export function hookActionValidate(action: HookAction): void {
  if (action.type === "command") {
    if (!action.command) throw new Error("Command hook has empty command string");
    if (Array.from(action.command).length > 32 * 1024) throw new Error("Command hook exceeds max length");
    if (action.timeout > 3600n) throw new Error("Command hook timeout exceeds maximum");
  } else if (action.type === "prompt") {
    if (!action.prompt) throw new Error("Prompt hook has empty prompt string");
  } else if (action.type === "permission") {
    if (!["allow", "deny", "ask"].includes(action.decision)) throw new Error("Permission hook has invalid decision");
  } else if (action.type === "http") {
    if (!action.url) throw new Error("Http hook has empty url");
    const parsed = new URL(action.url);
    if (parsed.protocol !== "https:" && !(parsed.protocol === "http:" && ["localhost", "127.0.0.1", "::1"].includes(parsed.hostname))) throw new Error("Http hook must use https unless it targets a local address");
    if (action.timeout > 3600n) throw new Error("Http hook timeout exceeds maximum");
  } else if (action.type === "mcp_tool") {
    if (!action.server) throw new Error("McpTool hook has empty server name");
    if (!action.tool) throw new Error("McpTool hook has empty tool name");
    if (action.timeout > 3600n) throw new Error("McpTool hook timeout exceeds maximum");
  } else if (action.type === "subagent") {
    if (!action.name) throw new Error("Subagent hook has empty subagent name");
    if (action.timeout > 3600n) throw new Error("Subagent hook timeout exceeds maximum");
  } else if (!action.skill) {
    throw new Error("ActivateSkill hook has empty skill name");
  }
}
