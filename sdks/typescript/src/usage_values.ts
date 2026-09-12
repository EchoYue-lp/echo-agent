/** Provider-normalized LLM usage values without owning provider execution. */
export interface TokenUsageDetails {
  readonly cachedTokens?: bigint | null;
  readonly cacheWriteTokens?: bigint | null;
  readonly reasoningTokens?: bigint | null;
}

const MAX_U32 = 0xffffffffn;
const sat = (value: bigint): bigint => value > MAX_U32 ? MAX_U32 : value;

export interface UsageInput {
  readonly promptTokens?: bigint | null;
  readonly completionTokens?: bigint | null;
  readonly totalTokens?: bigint | null;
  readonly promptTokensDetails?: TokenUsageDetails | null;
  readonly inputTokensDetails?: TokenUsageDetails | null;
  readonly outputTokensDetails?: TokenUsageDetails | null;
  readonly cacheCreationInputTokens?: bigint | null;
  readonly cacheReadInputTokens?: bigint | null;
  readonly promptCacheHitTokens?: bigint | null;
  readonly promptCacheMissTokens?: bigint | null;
}

export class Usage {
  public constructor(public readonly input: Readonly<UsageInput> = {}) {
    validateUsage(input);
  }

  public cachedPromptTokens(): bigint {
    return this.input.promptTokensDetails?.cachedTokens
      ?? this.input.inputTokensDetails?.cachedTokens
      ?? this.input.cacheReadInputTokens
      ?? this.input.promptCacheHitTokens
      ?? 0n;
  }

  public cacheCreationPromptTokens(): bigint {
    return this.input.promptTokensDetails?.cacheWriteTokens
      ?? this.input.inputTokensDetails?.cacheWriteTokens
      ?? this.input.cacheCreationInputTokens
      ?? 0n;
  }

  public effectivePromptTokens(): bigint {
    const prompt = this.input.promptTokens ?? 0n;
    if (this.input.cacheReadInputTokens != null || this.input.cacheCreationInputTokens != null) {
      return sat(prompt + this.cachedPromptTokens() + this.cacheCreationPromptTokens());
    }
    return prompt;
  }

  public effectiveTotalTokens(): bigint {
    const completion = this.input.completionTokens ?? 0n;
    if (this.input.cacheReadInputTokens != null || this.input.cacheCreationInputTokens != null) {
      return sat(this.effectivePromptTokens() + completion);
    }
    return this.input.totalTokens ?? sat(this.effectivePromptTokens() + completion);
  }

  public cacheHitRate(): number | undefined {
    const total = this.effectivePromptTokens();
    return total === 0n ? undefined : Number(this.cachedPromptTokens()) / Number(total);
  }
}

function validateUsage(input: UsageInput): void {
  const values = [
    input.promptTokens, input.completionTokens, input.totalTokens,
    input.cacheCreationInputTokens, input.cacheReadInputTokens,
    input.promptCacheHitTokens, input.promptCacheMissTokens,
    input.promptTokensDetails?.cachedTokens, input.promptTokensDetails?.cacheWriteTokens,
    input.promptTokensDetails?.reasoningTokens, input.inputTokensDetails?.cachedTokens,
    input.inputTokensDetails?.cacheWriteTokens, input.inputTokensDetails?.reasoningTokens,
    input.outputTokensDetails?.cachedTokens, input.outputTokensDetails?.cacheWriteTokens,
    input.outputTokensDetails?.reasoningTokens,
  ];
  if (values.some((value) => value != null && (typeof value !== "bigint" || value < 0n || value > MAX_U32))) {
    throw new RangeError("usage token values must fit u32");
  }
}
