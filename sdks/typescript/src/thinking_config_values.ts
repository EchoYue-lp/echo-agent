import { ThinkingLevel, thinkingLevelParse } from "./thinking.js";

export type ThinkingConfig =
  | Readonly<{ kind: "disabled" }>
  | Readonly<{ kind: "level"; level: ThinkingLevel }>
  | Readonly<{ kind: "budget_tokens"; budgetTokens: number }>;

function effortForBudget(tokens: number, glm: boolean): string {
  if (tokens < 4_000) return "low";
  if (tokens < 12_000) return "medium";
  if (tokens < 24_000) return "high";
  if (glm) return "max";
  if (tokens < 48_000) return "xhigh";
  return "max";
}

function levelEffort(level: ThinkingLevel): string | undefined {
  return level === ThinkingLevel.None ? "none" : level;
}

/** Provider-neutral thinking configuration and pure dialect projections. */
export const ThinkingConfig = Object.freeze({
  Disabled: Object.freeze({ kind: "disabled" }) as ThinkingConfig,
  Level(level: ThinkingLevel): ThinkingConfig {
    return Object.freeze({ kind: "level", level });
  },
  BudgetTokens(budgetTokens: number): ThinkingConfig {
    if (!Number.isSafeInteger(budgetTokens) || budgetTokens < 0) throw new RangeError("thinking budget must be non-negative");
    return Object.freeze({ kind: "budget_tokens", budgetTokens });
  },
  medium(): ThinkingConfig {
    return Object.freeze({ kind: "level", level: ThinkingLevel.Medium });
  },
  parseSpec(spec: string): ThinkingConfig | undefined {
    const trimmed = spec.trim().toLowerCase();
    if (trimmed === "" || trimmed === "auto" || trimmed === "default") return undefined;
    if (trimmed === "disabled" || trimmed === "off" || trimmed === "false") return this.Disabled;
    if (/^\d+$/.test(trimmed)) return this.BudgetTokens(Number(trimmed));
    const level = thinkingLevelParse(trimmed);
    if (level !== undefined) return this.Level(level);
    throw new TypeError(`unrecognized thinking spec: '${spec}'`);
  },
  toReasoningEffort(config: ThinkingConfig): string | undefined {
    if (config.kind === "disabled") return "minimal";
    if (config.kind === "budget_tokens") return effortForBudget(config.budgetTokens, false);
    return levelEffort(config.level);
  },
  toAnthropicEffort(config: ThinkingConfig): string | undefined {
    if (config.kind === "disabled" || config.kind === "level" && (config.level === ThinkingLevel.None || config.level === ThinkingLevel.Minimal)) return undefined;
    if (config.kind === "budget_tokens") return effortForBudget(config.budgetTokens, false);
    return config.level === ThinkingLevel.Minimal || config.level === ThinkingLevel.None ? undefined : config.level;
  },
  toAnthropicBudget(config: ThinkingConfig, maxTokens: number): number | undefined {
    if (maxTokens <= 1 || config.kind === "disabled" || config.kind === "level" && (config.level === ThinkingLevel.None || config.level === ThinkingLevel.Minimal)) return undefined;
    let budget: number;
    if (config.kind === "budget_tokens") budget = config.budgetTokens;
    else {
      const fraction: Record<ThinkingLevel, number> = {
        none: 0, minimal: 0, low: 0.25, medium: 0.5, high: 0.8, xhigh: 0.95, max: 0.98,
      };
      budget = Math.round(maxTokens * fraction[config.level]);
    }
    return Math.min(budget, maxTokens - 1);
  },
  toEnableThinking(config: ThinkingConfig): boolean {
    return config.kind === "budget_tokens"
      || config.kind === "level" && config.level !== ThinkingLevel.None && config.level !== ThinkingLevel.Minimal;
  },
  toGlmThinkingType(config: ThinkingConfig): "enabled" | "disabled" {
    return this.toEnableThinking(config) ? "enabled" : "disabled";
  },
  toGlmReasoningEffort(config: ThinkingConfig): string | undefined {
    if (config.kind === "disabled") return "none";
    if (config.kind === "budget_tokens") return effortForBudget(config.budgetTokens, true);
    return config.level;
  },
});
