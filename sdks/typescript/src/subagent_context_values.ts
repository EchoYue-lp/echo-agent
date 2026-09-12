/** Subagent context snapshot values without owning tools, messages, or stores. */
export class SubagentContext {
  public constructor(
    public readonly toolDefinitions: readonly unknown[] = [],
    public readonly messages: readonly unknown[] = [],
    public readonly storePresent: boolean = false,
    public readonly parentGoal: string | null = null,
    public readonly allowedTools: readonly string[] | null = null,
  ) {}

  public static empty(): SubagentContext {
    return new SubagentContext();
  }

  public hasContent(): boolean {
    return this.toolDefinitions.length > 0
      || this.messages.length > 0
      || this.storePresent
      || this.parentGoal !== null
      || this.allowedTools !== null;
  }
}
