import type { LlmMessage, LlmToolDefinition, WireValue } from "./types.js";
import { SegmentRange } from "./segment_range_values.js";

/** Message-cache segment indexes without owning cache state. */
export class SegmentRanges {
  public constructor(
    public readonly system: SegmentRange = new SegmentRange(),
    public readonly canonical: SegmentRange = new SegmentRange(),
    public readonly history: SegmentRange = new SegmentRange(),
    public readonly runtimeContext: SegmentRange = new SegmentRange(),
  ) {
    Object.freeze(this);
  }
}

/** Read-only prompt layout projection; cache storage and provider placement stay in Rust. */
export class PromptCacheLayout {
  public readonly system: readonly LlmMessage[];
  public readonly canonical: readonly LlmMessage[];
  public readonly history: readonly LlmMessage[];
  public readonly runtimeContext: readonly LlmMessage[];
  public readonly tools: readonly LlmToolDefinition[];

  private constructor(
    system: readonly LlmMessage[],
    canonical: readonly LlmMessage[],
    history: readonly LlmMessage[],
    runtimeContext: readonly LlmMessage[],
    tools: readonly LlmToolDefinition[],
  ) {
    this.system = Object.freeze([...system]);
    this.canonical = Object.freeze([...canonical]);
    this.history = Object.freeze([...history]);
    this.runtimeContext = Object.freeze([...runtimeContext]);
    this.tools = Object.freeze([...tools]);
    Object.freeze(this);
  }

  public static fromMessages(
    messages: readonly LlmMessage[],
    tools: readonly LlmToolDefinition[],
  ): PromptCacheLayout {
    if (!Array.isArray(messages) || !Array.isArray(tools)) {
      throw new TypeError("messages and tools must be arrays");
    }
    const systemEnd = messages.findIndex((message) => message.role !== "system");
    const sysEnd = systemEnd < 0 ? messages.length : systemEnd;
    const canonicalIndex = messages
      .slice(0, sysEnd)
      .findIndex((message) => messageText(message)?.includes("Canonical context") ?? false);
    const canonicalStart = canonicalIndex < 0 ? sysEnd : canonicalIndex;

    let runtimeStart = messages.length;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const candidate = messages[index];
      if (candidate && (messageText(candidate)?.trimStart().startsWith("[runtime_context:") ?? false)) {
        runtimeStart = index;
        while (runtimeStart > sysEnd) {
          const previous = messages[runtimeStart - 1];
          if (!previous || !(messageText(previous)?.trimStart().startsWith("[runtime_context:") ?? false)) break;
          runtimeStart -= 1;
        }
        break;
      }
    }

    return new PromptCacheLayout(
      messages.slice(0, Math.min(canonicalStart, sysEnd)),
      canonicalStart < sysEnd ? messages.slice(canonicalStart, sysEnd) : [],
      messages.slice(sysEnd, runtimeStart),
      messages.slice(runtimeStart),
      tools,
    );
  }

  public segmentRanges(): SegmentRanges {
    const system = BigInt(this.system.length);
    const canonical = BigInt(this.canonical.length);
    const history = BigInt(this.history.length);
    const runtimeContext = BigInt(this.runtimeContext.length);
    return new SegmentRanges(
      new SegmentRange(0n, system),
      new SegmentRange(system, system + canonical),
      new SegmentRange(system + canonical, system + canonical + history),
      new SegmentRange(system + canonical + history, system + canonical + history + runtimeContext),
    );
  }
}

function messageText(message: LlmMessage): string | null {
  const content: unknown = message.content;
  if (typeof content === "string") return content;
  if (isWireString(content)) return content.value;
  return null;
}

function isWireString(value: unknown): value is Extract<WireValue, { readonly kind: "string" }> {
  return typeof value === "object"
    && value !== null
    && !Array.isArray(value)
    && (value as { readonly kind?: unknown }).kind === "string"
    && typeof (value as { readonly value?: unknown }).value === "string";
}
