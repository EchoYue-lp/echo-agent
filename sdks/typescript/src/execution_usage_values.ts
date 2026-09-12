import type { ExecutionUsage } from "./types.js";

/** Return duration in milliseconds, matching Rust's absent-to-zero helper. */
export function executionUsageDurationMillis(usage: ExecutionUsage): bigint {
  return BigInt(usage.duration_ms ?? "0");
}
