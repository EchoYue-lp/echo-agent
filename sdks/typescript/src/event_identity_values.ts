import { randomUUID } from "node:crypto";

function nonEmpty(value: string, label: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new TypeError(`${label} must not be empty`);
  }
  return value;
}

/** Validated event stream identity values. */
export class StreamId {
  readonly value: string;

  private constructor(value: string) {
    this.value = nonEmpty(value, "event stream_id");
    Object.freeze(this);
  }

  static new(value: string): StreamId {
    return new StreamId(value);
  }

  asStr(): string {
    return this.value;
  }

  toString(): string {
    return this.value;
  }
}

/** Validated event identity values. */
export class EventId {
  readonly value: string;

  private constructor(value: string) {
    this.value = nonEmpty(value, "event_id");
    Object.freeze(this);
  }

  static new(value: string): EventId {
    return new EventId(value);
  }

  asStr(): string {
    return this.value;
  }

  toString(): string {
    return this.value;
  }
}

export type EventRuntimeContext = {
  conversation_id?: string | null;
  run_id?: string | null;
  turn_id?: string | null;
  message_id?: string | null;
  execution_id?: string | null;
  parent_event_id?: string | null;
  subagent_lineage?: { parent_event_id?: string | null } | null;
};

/** Immutable transport identity projection without owning a live stream. */
export class EventIdentity {
  readonly streamId: StreamId;
  readonly conversationId: string | undefined;
  readonly runId: string | undefined;
  readonly turnId: string;
  readonly messageId: string | undefined;
  readonly executionId: string | undefined;
  readonly parentEventId: string | undefined;

  private constructor(fields: {
    streamId: StreamId;
    conversationId?: string;
    runId?: string;
    turnId: string;
    messageId?: string;
    executionId?: string;
    parentEventId?: string;
  }) {
    this.streamId = fields.streamId;
    this.conversationId = fields.conversationId;
    this.runId = fields.runId;
    this.turnId = nonEmpty(fields.turnId, "turn_id");
    this.messageId = fields.messageId;
    this.executionId = fields.executionId;
    this.parentEventId = fields.parentEventId;
    this.validate();
    Object.freeze(this);
  }

  static new(streamId: string, turnId: string): EventIdentity {
    return new EventIdentity({ streamId: StreamId.new(streamId), turnId });
  }

  static forRun(runId: string): EventIdentity {
    const id = nonEmpty(runId, "run_id");
    return new EventIdentity({
      streamId: StreamId.new(randomUUID()),
      runId: id,
      turnId: id,
      executionId: id,
    });
  }

  static forChat(
    conversationId: string | undefined,
    turnId: string,
    messageId: string,
    runId?: string,
  ): EventIdentity {
    return new EventIdentity({
      streamId: StreamId.new(randomUUID()),
      conversationId: conversationId === undefined ? undefined : nonEmpty(conversationId, "conversation_id"),
      runId: runId === undefined ? undefined : nonEmpty(runId, "run_id"),
      turnId,
      messageId: nonEmpty(messageId, "message_id"),
    });
  }

  static fromInvocation(invocation: { runtime?: EventRuntimeContext | null } | null | undefined): EventIdentity {
    return EventIdentity.fromRuntimeContext(invocation?.runtime);
  }

  static fromRuntimeContext(runtime?: EventRuntimeContext | null): EventIdentity {
    const runId = runtime?.run_id ?? undefined;
    const executionId = runtime?.execution_id ?? undefined;
    const turnId = runtime?.turn_id ?? executionId ?? runId ?? randomUUID();
    const parentEventId = runtime?.parent_event_id ?? runtime?.subagent_lineage?.parent_event_id ?? undefined;
    return new EventIdentity({
      streamId: StreamId.new(randomUUID()),
      conversationId: runtime?.conversation_id ?? undefined,
      runId,
      turnId,
      messageId: runtime?.message_id ?? undefined,
      executionId,
      parentEventId,
    });
  }

  validate(): void {
    StreamId.new(this.streamId.asStr());
    nonEmpty(this.turnId, "turn_id");
    for (const [value, label] of [
      [this.conversationId, "conversation_id"],
      [this.runId, "run_id"],
      [this.messageId, "message_id"],
      [this.executionId, "execution_id"],
      [this.parentEventId, "event_id"],
    ] as const) {
      if (value !== undefined) nonEmpty(value, label);
    }
  }

  withConversationId(value: string): EventIdentity {
    return this.copy({ conversationId: nonEmpty(value, "conversation_id") });
  }

  withRunId(value: string): EventIdentity {
    return this.copy({ runId: nonEmpty(value, "run_id") });
  }

  withMessageId(value: string): EventIdentity {
    return this.copy({ messageId: nonEmpty(value, "message_id") });
  }

  withExecutionId(value: string): EventIdentity {
    return this.copy({ executionId: nonEmpty(value, "execution_id") });
  }

  withParentEventId(value: string): EventIdentity {
    return this.copy({ parentEventId: nonEmpty(value, "event_id") });
  }

  private copy(overrides: Partial<Omit<EventIdentity, "copy" | "validate">>): EventIdentity {
    return new EventIdentity({
      streamId: overrides.streamId ?? this.streamId,
      conversationId: overrides.conversationId ?? this.conversationId,
      runId: overrides.runId ?? this.runId,
      turnId: overrides.turnId ?? this.turnId,
      messageId: overrides.messageId ?? this.messageId,
      executionId: overrides.executionId ?? this.executionId,
      parentEventId: overrides.parentEventId ?? this.parentEventId,
    });
  }
}
