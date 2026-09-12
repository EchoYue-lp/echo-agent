/** Durable Subagent command identities without owning live-control state. */
export class SubagentAttemptIdentity {
  public constructor(
    public readonly taskId: string,
    public readonly executionId: string,
    public readonly attempt: number,
  ) {
    if (!taskId.trim()) throw new Error("invalid identity: task_id");
    if (!executionId.trim()) throw new Error("invalid identity: execution_id");
    if (!Number.isInteger(attempt) || attempt < 0 || attempt > 0xffffffff) {
      throw new Error("invalid identity: attempt");
    }
    Object.freeze(this);
  }
}

export class SubagentCommandIdentity {
  public constructor(
    public readonly runId: string,
    public readonly taskId: string,
    public readonly executionId: string,
    public readonly planRevision: bigint,
    public readonly attempt: number,
    public readonly commandId: string,
  ) {
    this.validate();
    Object.freeze(this);
  }

  public validate(): void {
    if (!this.runId.trim()) throw new Error("invalid identity: run_id");
    if (this.planRevision <= 0n) throw new Error("invalid identity: plan_revision");
    if (!this.commandId.trim()) throw new Error("invalid identity: command_id");
    new SubagentAttemptIdentity(this.taskId, this.executionId, this.attempt);
  }

  public attemptIdentity(): SubagentAttemptIdentity {
    return new SubagentAttemptIdentity(this.taskId, this.executionId, this.attempt);
  }
}
