/** MCP JSON-RPC request/notification values without owning transport. */
export interface JsonRpcRequest {
  readonly jsonrpc: "2.0";
  readonly id: unknown | null;
  readonly method: string;
  readonly params: unknown | null;
}

export interface JsonRpcNotification {
  readonly jsonrpc: "2.0";
  readonly method: string;
  readonly params: unknown | null;
}

export function jsonRpcRequest(method: string, params: unknown | null = null): JsonRpcRequest {
  return Object.freeze({ jsonrpc: "2.0" as const, id: null, method, params });
}

export function jsonRpcNotification(method: string, params: unknown | null = null): JsonRpcNotification {
  return Object.freeze({ jsonrpc: "2.0" as const, method, params });
}
