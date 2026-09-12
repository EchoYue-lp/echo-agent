/** Cumulative Subagent LLM usage values without owning provider execution. */
export class LlmUsageStats {
  public model = "";
  public promptTokens = 0n;
  public completionTokens = 0n;
  public totalTokens = 0n;
  public cachedPromptTokens = 0n;
  public cacheCreationPromptTokens = 0n;
  public usageReported = false;
  public callCount = 0n;

  public record(model: string, promptTokens: bigint, completionTokens: bigint, totalTokens: bigint,
    cachedPromptTokens: bigint, cacheCreationPromptTokens: bigint, usageReported: boolean): void {
    this.model = model;
    this.promptTokens += promptTokens;
    this.completionTokens += completionTokens;
    this.totalTokens += totalTokens;
    this.cachedPromptTokens += cachedPromptTokens;
    this.cacheCreationPromptTokens += cacheCreationPromptTokens;
    this.usageReported ||= usageReported;
    this.callCount += 1n;
  }

  public toPayload(sessionId: string): Record<string, string | boolean> {
    return {
      session_id: sessionId,
      model: this.model || "unknown",
      prompt_tokens: this.promptTokens.toString(),
      completion_tokens: this.completionTokens.toString(),
      total_tokens: this.totalTokens.toString(),
      cached_prompt_tokens: this.cachedPromptTokens.toString(),
      cache_creation_prompt_tokens: this.cacheCreationPromptTokens.toString(),
      usage_reported: this.usageReported,
      call_count: this.callCount.toString(),
    };
  }
}
