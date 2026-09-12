/** ACP runtime values projected without owning connection or ledger state. */
export const ConnectionMode = Object.freeze({
  Standard: "standard",
  Extended: "extended",
} as const);

export type ConnectionMode = (typeof ConnectionMode)[keyof typeof ConnectionMode];

export const ExtensionSettlement = Object.freeze({
  Answered: "answered",
  TimedOut: "timed_out",
  Cancelled: "cancelled",
  Disconnected: "disconnected",
} as const);

export type ExtensionSettlement =
  (typeof ExtensionSettlement)[keyof typeof ExtensionSettlement];

export function extensionSettlementIsAnswered(settlement: ExtensionSettlement): boolean {
  if (!Object.values(ExtensionSettlement).includes(settlement)) {
    throw new TypeError("invalid extension settlement");
  }
  return settlement === ExtensionSettlement.Answered;
}

export interface AcpLedgerLimits {
  readonly maxEvents: bigint;
  readonly maxBytes: bigint;
}

export function acpLedgerLimits(
  maxEvents: bigint,
  maxBytes: bigint,
): AcpLedgerLimits {
  for (const [name, value] of [["maxEvents", maxEvents], ["maxBytes", maxBytes]] as const) {
    if (typeof value !== "bigint" || value < 0n) {
      throw new RangeError(`${name} must be a non-negative bigint`);
    }
  }
  return Object.freeze({ maxEvents, maxBytes });
}

export function acpLedgerLimitsDefault(): AcpLedgerLimits {
  return acpLedgerLimits(10_000n, 8_388_608n);
}

export interface AcpAdapterConfig {
  readonly name: string;
  readonly title: string;
  readonly version: string;
  readonly maxSessions: bigint;
  readonly maxPromptChars: bigint;
  readonly maxUpdateChars: bigint;
  readonly maxUpdatesPerTurn: bigint;
  readonly maxTotalUpdateChars: bigint;
  readonly maxExtensionConcurrency: bigint;
  readonly shutdownTimeout: WireDuration;
}

export function acpAdapterConfigDefault(version = "0.2.0"): AcpAdapterConfig {
  return Object.freeze({
    name: "echo-agent",
    title: "echo-agent",
    version,
    maxSessions: 128n,
    maxPromptChars: 1_000_000n,
    maxUpdateChars: 1_000_000n,
    maxUpdatesPerTurn: 10_000n,
    maxTotalUpdateChars: 8_000_000n,
    maxExtensionConcurrency: 8n,
    shutdownTimeout: Object.freeze({ seconds: "5", nanos: 0 }),
  });
}

export function acpAdapterConfigValidate(config: AcpAdapterConfig): void {
  if (config === null || typeof config !== "object") {
    throw new TypeError("ACP adapter config must be an object");
  }
  if (!config.name.trim() || !config.title.trim() || !config.version.trim()) {
    throw new Error("ACP adapter name, title, and version must not be empty");
  }
  for (const [name, value] of [
    ["maxSessions", config.maxSessions],
    ["maxPromptChars", config.maxPromptChars],
    ["maxUpdateChars", config.maxUpdateChars],
    ["maxUpdatesPerTurn", config.maxUpdatesPerTurn],
    ["maxTotalUpdateChars", config.maxTotalUpdateChars],
    ["maxExtensionConcurrency", config.maxExtensionConcurrency],
  ] as const) {
    if (typeof value !== "bigint" || value <= 0n) {
      throw new Error("ACP adapter resource limits must be positive");
    }
  }
  if (
    !config.shutdownTimeout
    || typeof config.shutdownTimeout.seconds !== "string"
    || !/^\d+$/.test(config.shutdownTimeout.seconds)
    || !Number.isInteger(config.shutdownTimeout.nanos)
    || config.shutdownTimeout.nanos < 0
    || config.shutdownTimeout.nanos >= 1_000_000_000
    || (config.shutdownTimeout.seconds === "0" && config.shutdownTimeout.nanos === 0)
  ) {
    throw new Error("ACP adapter shutdown timeout must be positive");
  }
}

export const ExtensionLeaseError = Object.freeze({
  AdmissionClosed: "extension admission is closed",
  ConcurrencyLimit: "extension concurrency limit reached",
  ExclusiveConflict: "extension is already executing an exclusive invocation",
} as const);

export type ExtensionLeaseError =
  (typeof ExtensionLeaseError)[keyof typeof ExtensionLeaseError];

export function extensionLeaseErrorAsStr(error: ExtensionLeaseError): string {
  if (!Object.values(ExtensionLeaseError).includes(error)) {
    throw new TypeError("invalid extension lease error");
  }
  return error;
}
import type { WireDuration } from "./types.js";
