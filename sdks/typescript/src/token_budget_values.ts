/** Local token budget policy values; model execution remains Rust-owned. */
export type TokenAllocation = Readonly<{
  systemFits: boolean;
  toolDefsFit: boolean;
  conversationFits: boolean;
  outputFits: boolean;
  conversationExcess: number;
  usagePct: number;
}> & {
  ok(): boolean;
  needsCompression(): boolean;
};

export type BudgetReport = Readonly<{
  totalWindow: number;
  systemPrompt: number;
  systemPromptBudget: number;
  toolDefinitions: number;
  toolDefinitionsBudget: number;
  conversation: number;
  conversationBudget: number;
  estimatedOutput: number;
  outputBudget: number;
  usagePct: number;
  needsCompression: boolean;
}>;

function finiteFraction(value: number): number {
  if (!Number.isFinite(value) || value < 0 || value > 1) {
    throw new RangeError("token budget allocation must be finite and in [0, 1]");
  }
  return value;
}

export class TokenBudget {
  private constructor(
    readonly totalWindow: number,
    private readonly systemPct: number,
    private readonly toolPct: number,
    private readonly outputPct: number,
    private readonly safetyPct: number,
  ) {}

  static new(totalWindow: number): TokenBudget {
    if (!Number.isSafeInteger(totalWindow) || totalWindow <= 0) {
      throw new RangeError("token budget total window must be greater than zero");
    }
    return new TokenBudget(totalWindow, 0.1, 0.05, 0.1, 0.1);
  }

  static default(): TokenBudget {
    return new TokenBudget(128_000, 0.1, 0.05, 0.1, 0.1);
  }

  withAllocations(systemPct: number, toolPct: number, outputPct: number, safetyPct: number): TokenBudget {
    const system = finiteFraction(systemPct);
    const tool = finiteFraction(toolPct);
    const output = finiteFraction(outputPct);
    const safety = finiteFraction(safetyPct);
    if (system + tool + output + safety > 1) {
      throw new RangeError("token budget allocations exceed 1.0");
    }
    return new TokenBudget(this.totalWindow, system, tool, output, safety);
  }

  systemPromptBudget(): number { return Math.round(this.totalWindow * this.systemPct); }
  toolDefinitionsBudget(): number { return Math.round(this.totalWindow * this.toolPct); }
  outputBudget(): number { return Math.round(this.totalWindow * this.outputPct); }
  safetyBudget(): number { return Math.round(this.totalWindow * this.safetyPct); }

  conversationBudget(): number {
    return Math.round(this.totalWindow * Math.max(0, 1 - this.systemPct - this.toolPct - this.outputPct - this.safetyPct));
  }

  allocate(systemSize: number, toolDefsSize: number, conversationSize: number): TokenAllocation {
    const effective = Math.max(0, this.totalWindow - this.outputBudget() - this.safetyBudget() - systemSize - toolDefsSize);
    const allocation = {
      systemFits: systemSize <= this.systemPromptBudget(),
      toolDefsFit: toolDefsSize <= this.toolDefinitionsBudget(),
      conversationFits: conversationSize <= effective,
      outputFits: this.outputBudget() > 0,
      conversationExcess: Math.max(0, conversationSize - effective),
      usagePct: ((systemSize + toolDefsSize + conversationSize) / this.totalWindow) * 100,
    } as TokenAllocation;
    allocation.ok = () => allocation.systemFits && allocation.toolDefsFit && allocation.conversationFits && allocation.outputFits;
    allocation.needsCompression = () => allocation.conversationExcess > 0;
    return Object.freeze(allocation);
  }

  report(systemSize: number, toolDefsSize: number, conversationSize: number, estimatedOutput: number): BudgetReport {
    const allocation = this.allocate(systemSize, toolDefsSize, conversationSize);
    return Object.freeze({
      totalWindow: this.totalWindow,
      systemPrompt: systemSize,
      systemPromptBudget: this.systemPromptBudget(),
      toolDefinitions: toolDefsSize,
      toolDefinitionsBudget: this.toolDefinitionsBudget(),
      conversation: conversationSize,
      conversationBudget: this.conversationBudget(),
      estimatedOutput,
      outputBudget: this.outputBudget(),
      usagePct: allocation.usagePct,
      needsCompression: allocation.needsCompression(),
    });
  }
}

export class TokenBudgetConfig {
  private constructor(
    readonly totalWindow: number | undefined,
    readonly systemPct: number,
    readonly toolPct: number,
    readonly outputPct: number,
    readonly safetyPct: number,
    readonly enabled: boolean,
  ) {}

  static enabled(): TokenBudgetConfig { return new TokenBudgetConfig(undefined, 0.1, 0.05, 0.1, 0.1, true); }
  static disabled(): TokenBudgetConfig { return new TokenBudgetConfig(undefined, 0.1, 0.05, 0.1, 0.1, false); }

  withTotalWindow(window: number): TokenBudgetConfig {
    if (!Number.isSafeInteger(window) || window <= 0) throw new RangeError("token budget total window must be greater than zero");
    return new TokenBudgetConfig(window, this.systemPct, this.toolPct, this.outputPct, this.safetyPct, this.enabled);
  }

  build(fallbackWindow: number): TokenBudget {
    return TokenBudget.new(this.totalWindow ?? fallbackWindow).withAllocations(this.systemPct, this.toolPct, this.outputPct, this.safetyPct);
  }
}

/** Timeout policy in milliseconds; zero disables a boundary like Rust Duration. */
export class LlmTimeouts {
  private constructor(
    private readonly request: number | undefined,
    private readonly firstChunk: number | undefined,
    private readonly idle: number | undefined,
    private readonly overall: number | undefined,
  ) {}

  static default(): LlmTimeouts { return new LlmTimeouts(60_000, 30_000, 30_000, undefined); }
  requestTimeout(): number | undefined { return this.request; }
  firstChunkTimeout(): number | undefined { return this.firstChunk; }
  idleTimeout(): number | undefined { return this.idle; }
  overallTimeout(): number | undefined { return this.overall; }
  withRequestTimeout(timeoutMs: number): LlmTimeouts { return new LlmTimeouts(timeoutMs > 0 ? timeoutMs : undefined, this.firstChunk, this.idle, this.overall); }
  withoutRequestTimeout(): LlmTimeouts { return new LlmTimeouts(undefined, this.firstChunk, this.idle, this.overall); }
  withFirstChunkTimeout(timeoutMs: number): LlmTimeouts { return new LlmTimeouts(this.request, timeoutMs > 0 ? timeoutMs : undefined, this.idle, this.overall); }
  withoutFirstChunkTimeout(): LlmTimeouts { return new LlmTimeouts(this.request, undefined, this.idle, this.overall); }
  withIdleTimeout(timeoutMs: number): LlmTimeouts { return new LlmTimeouts(this.request, this.firstChunk, timeoutMs > 0 ? timeoutMs : undefined, this.overall); }
  withoutIdleTimeout(): LlmTimeouts { return new LlmTimeouts(this.request, this.firstChunk, undefined, this.overall); }
  withOverallTimeout(timeoutMs: number): LlmTimeouts { return new LlmTimeouts(this.request, this.firstChunk, this.idle, timeoutMs > 0 ? timeoutMs : undefined); }
  withoutOverallTimeout(): LlmTimeouts { return new LlmTimeouts(this.request, this.firstChunk, this.idle, undefined); }
}
