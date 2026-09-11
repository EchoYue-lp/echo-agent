/** Closed A2A task-state values exposed by the Rust facade. */
export const TaskState = Object.freeze({
  Submitted: "submitted",
  Working: "working",
  InputRequired: "input-required",
  Completed: "completed",
  Failed: "failed",
  Canceled: "canceled",
} as const);

export type TaskState = (typeof TaskState)[keyof typeof TaskState];

const TASK_STATES: ReadonlySet<string> = new Set(Object.values(TaskState));

export function taskStateIsTerminal(state: TaskState): boolean {
  validateTaskState(state);
  return state === TaskState.Completed || state === TaskState.Failed || state === TaskState.Canceled;
}

export function taskStateCanTransitionTo(current: TaskState, next: TaskState): boolean {
  validateTaskState(current);
  validateTaskState(next);
  if (taskStateIsTerminal(current)) return false;
  return (current === TaskState.Submitted
      && (next === TaskState.Working || next === TaskState.Canceled))
    || (current === TaskState.Working
      && (next === TaskState.Completed
        || next === TaskState.Failed
        || next === TaskState.InputRequired
        || next === TaskState.Canceled))
    || (current === TaskState.InputRequired
      && (next === TaskState.Working || next === TaskState.Canceled));
}

export type A2APart =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "file"; readonly mimeType: string; readonly data: string };

export class A2AMessage {
  public readonly role: string;
  public readonly parts: readonly A2APart[];

  private constructor(role: string, parts: readonly A2APart[]) {
    this.role = role;
    this.parts = Object.freeze(parts.map((part) => Object.freeze({ ...part })));
    Object.freeze(this);
  }

  public static userText(text: string): A2AMessage {
    if (typeof text !== "string") throw new TypeError("message text must be text");
    return new A2AMessage("user", [{ type: "text", text }]);
  }

  public static agentText(text: string): A2AMessage {
    if (typeof text !== "string") throw new TypeError("message text must be text");
    return new A2AMessage("agent", [{ type: "text", text }]);
  }

  public textContent(): string {
    return this.parts
      .filter((part): part is Extract<A2APart, { readonly type: "text" }> => part.type === "text")
      .map((part) => part.text)
      .join("\n");
  }
}

export class A2ATaskStatus {
  public readonly state: TaskState;
  public readonly message?: A2AMessage;
  public readonly timestamp: string;

  private constructor(state: TaskState, message?: A2AMessage) {
    validateTaskState(state);
    this.state = state;
    this.message = message;
    this.timestamp = new Date().toISOString();
    Object.freeze(this);
  }

  public static new(state: TaskState): A2ATaskStatus {
    return new A2ATaskStatus(state);
  }

  public static withMessage(state: TaskState, message: A2AMessage): A2ATaskStatus {
    if (!(message instanceof A2AMessage)) throw new TypeError("A2A task status message must be an A2AMessage");
    return new A2ATaskStatus(state, message);
  }
}

export class AgentProvider {
  public readonly organization: string;
  public readonly url?: string;

  private constructor(organization: string, url?: string) {
    this.organization = organization;
    this.url = url;
    Object.freeze(this);
  }

  public static new(organization: string): AgentProvider {
    if (typeof organization !== "string") throw new TypeError("organization must be text");
    return new AgentProvider(organization);
  }

  public withUrl(url: string): AgentProvider {
    if (typeof url !== "string") throw new TypeError("provider url must be text");
    return new AgentProvider(this.organization, url);
  }
}

export class AgentSkill {
  public readonly id: string;
  public readonly name: string;
  public readonly description?: string;
  public readonly examples: readonly string[];
  public readonly inputModes: readonly string[];
  public readonly outputModes: readonly string[];
  public readonly tags: readonly string[];

  private constructor(
    name: string,
    description: string,
    examples: readonly string[] = [],
    tags: readonly string[] = [],
  ) {
    this.id = name;
    this.name = name;
    this.description = description;
    this.examples = Object.freeze([...examples]);
    this.inputModes = Object.freeze([]);
    this.outputModes = Object.freeze([]);
    this.tags = Object.freeze([...tags]);
    Object.freeze(this);
  }

  public static new(name: string, description: string): AgentSkill {
    if (typeof name !== "string" || typeof description !== "string") {
      throw new TypeError("skill name and description must be text");
    }
    return new AgentSkill(name, description);
  }

  public withExamples(examples: readonly string[]): AgentSkill {
    return this.copy({ examples: textList(examples, "examples") });
  }

  public withTags(tags: readonly string[]): AgentSkill {
    return this.copy({ tags: textList(tags, "tags") });
  }

  private copy(changes: Partial<AgentSkill>): AgentSkill {
    return new AgentSkill(
      this.name,
      this.description ?? "",
      changes.examples ?? this.examples,
      changes.tags ?? this.tags,
    );
  }
}

function validateTaskState(state: string): asserts state is TaskState {
  if (!TASK_STATES.has(state)) throw new TypeError(`unknown A2A task state: ${state}`);
}

function textList(values: readonly string[], field: string): readonly string[] {
  if (!Array.isArray(values) || values.some((value) => typeof value !== "string")) {
    throw new TypeError(`${field} must be a string array`);
  }
  return [...values];
}
