/** User-facing reasoning effort levels with Rust-compatible parsing. */
export const ThinkingLevel = Object.freeze({
  None: "none",
  Minimal: "minimal",
  Low: "low",
  Medium: "medium",
  High: "high",
  Xhigh: "xhigh",
  Max: "max",
} as const);

export type ThinkingLevel = (typeof ThinkingLevel)[keyof typeof ThinkingLevel];

export function thinkingLevelParse(value: string): ThinkingLevel | undefined {
  if (typeof value !== "string") return undefined;
  switch (value.trim().toLowerCase()) {
    case "none":
    case "off":
      return ThinkingLevel.None;
    case "minimal":
    case "min":
      return ThinkingLevel.Minimal;
    case "low":
      return ThinkingLevel.Low;
    case "medium":
    case "med":
    case "normal":
      return ThinkingLevel.Medium;
    case "high":
      return ThinkingLevel.High;
    case "xhigh":
      return ThinkingLevel.Xhigh;
    case "max":
      return ThinkingLevel.Max;
    default:
      return undefined;
  }
}
