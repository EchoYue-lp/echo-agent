export class EchoAgentError extends Error {
  readonly code: string;
  readonly retryable: string;
  readonly operation?: string;
  readonly details?: unknown;

  constructor(
    code: string,
    message: string,
    retryable = "never",
    operation?: string,
    details?: unknown,
  ) {
    super(message);
    this.name = "EchoAgentError";
    this.code = code;
    this.retryable = retryable;
    this.operation = operation;
    this.details = details;
  }

  static fromUnknown(error: unknown, operation?: string): EchoAgentError {
    if (error instanceof EchoAgentError) return error;
    const candidate = error as { data?: unknown; message?: unknown } | null;
    const data = candidate && typeof candidate.data === "object" && candidate.data !== null
      ? candidate.data as Record<string, unknown>
      : undefined;
    const code = typeof data?.code === "string" ? data.code : "transport_error";
    const message = typeof data?.message === "string"
      ? data.message
      : typeof candidate?.message === "string" ? candidate.message : String(error);
    const retryable = typeof data?.retryable === "string" ? data.retryable : "never";
    const resolvedOperation = typeof data?.operation === "string" ? data.operation : operation;
    return new EchoAgentError(code, message, retryable, resolvedOperation, data?.details);
  }
}
